#!/usr/bin/env bash
# Xcode "Run Script" 构建阶段：把 rshell-m0 编成当前架构的静态库。
#
# 用法：Target → Build Phases → + → New Run Script Phase
#   位置放在 "Compile Sources" 之前（把输出拖到最上面）
#   脚本内容： <本仓库>/ios-host/build-rust.sh 的绝对路径
#             （Xcode 工程不一定和 rust/ 放在同一棵树里，用绝对路径最省事）
#   取消勾选 "Based on dependency analysis"（否则增量构建可能跳过它）
#
#   ⚠ 还要在这个 phase 的 "Output Files" 里声明（否则会被 User Script Sandboxing
#     拦住 cp，报 `Sandbox: cp(...) deny(1) file-write-create`）：
#       $(BUILT_PRODUCTS_DIR)/librshell_m0.a
#
# 然后在 Build Settings 里设置：
#   LIBRARY_SEARCH_PATHS = $(BUILT_PRODUCTS_DIR)
#   OTHER_LDFLAGS        = -lrshell_m0 -liconv
#   SWIFT_OBJC_BRIDGING_HEADER = <相对 SRCROOT 的整条路径，例如 GuoSSHell/GuoSSHell-Bridging-Header.h>
#
# 为什么需要 -liconv：rusqlite 的 bundled SQLite 与 ring 在 Apple 平台上会引用
# /usr/lib/libiconv.2.dylib（`otool -L` 实测可见），Xcode 不会自动带上。

set -euo pipefail

# Xcode 的构建环境里没有 ~/.cargo/bin。
export PATH="${HOME}/.cargo/bin:${PATH}"

# ⚠ 本脚本由 Xcode 用 /bin/sh 调起，实际解释器是 macOS 自带的 bash **3.2**，
#   不是你交互式 shell 里那个 bash 5。所以只能写 bash 3.2 能吃的语法：
#   空数组展开 "${arr[@]}" 在 3.2 下配合 set -u 会报 "unbound variable"，
#   大小写展开 "${var,,}" 是 4.0+ 的，会报 "bad substitution"。
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../rust" && pwd)"

case "${PLATFORM_NAME:-iphoneos}" in
  iphoneos)
    TARGET="aarch64-apple-ios"
    ;;
  iphonesimulator)
    if [[ "${CURRENT_ARCH:-}" == "x86_64" ]]; then
      TARGET="x86_64-apple-ios"
    else
      TARGET="aarch64-apple-ios-sim"
    fi
    ;;
  *)
    echo "error: M0 只处理 iphoneos / iphonesimulator，收到 ${PLATFORM_NAME}" >&2
    exit 1
    ;;
esac

PROFILE="${CONFIGURATION:-Debug}"
if [[ "${PROFILE}" == "Release" ]]; then
  CARGO_FLAGS="--release"
else
  CARGO_FLAGS=""
fi
PROFILE_LC="$(printf '%s' "${PROFILE}" | tr '[:upper:]' '[:lower:]')"

echo "note: 构建 rshell-m0 ${TARGET} / ${PROFILE}"
# shellcheck disable=SC2086  # 故意不加引号：CARGO_FLAGS 可能为空，也可能是一个标志
cargo build \
  --manifest-path "${CRATE_DIR}/Cargo.toml" \
  --lib \
  --target "${TARGET}" \
  ${CARGO_FLAGS}

ARTIFACT="${CRATE_DIR}/target/${TARGET}/${PROFILE_LC}/librshell_m0.a"
if [[ ! -f "${ARTIFACT}" ]]; then
  echo "error: 没有产出 ${ARTIFACT}" >&2
  exit 1
fi

# 放到 BUILT_PRODUCTS_DIR，配合 LIBRARY_SEARCH_PATHS + -lrshell_m0 使用。
DESTINATION="${BUILT_PRODUCTS_DIR}/librshell_m0.a"
cp -f "${ARTIFACT}" "${DESTINATION}"
echo "note: 已放入 ${DESTINATION} ($(du -h "${DESTINATION}" | cut -f1))"
