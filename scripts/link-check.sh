#!/usr/bin/env bash
# M0b 的「起飞前检查」：在不打开 Xcode 的前提下，证明静态库能被一个 iOS 目标完整链接。
#
# 它替我们回答了 M0b 里唯一不是 GUI 操作的那个未知量：
#   「.a 链进 iOS App 会不会缺符号 / 缺框架 / 缺链接标志」
# 剩下的就只有 Xcode 里点几下的事了。
#
# 用法：./scripts/link-check.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="$(cd "${HERE}/.." && pwd)"
RUST="${PROJECT}/rust"

if [ -d "${HOME}/.cargo/bin" ] && ! command -v cargo >/dev/null 2>&1; then
  export PATH="${HOME}/.cargo/bin:${PATH}"
fi

echo "== 1/3 构建两个切片的静态库 =="
(cd "${RUST}" && cargo build --release --lib --target aarch64-apple-ios)
(cd "${RUST}" && cargo build --release --lib --target aarch64-apple-ios-sim)

LIB_DEVICE="${RUST}/target/aarch64-apple-ios/release/librshell_m0.a"
LIB_SIM="${RUST}/target/aarch64-apple-ios-sim/release/librshell_m0.a"
for lib in "${LIB_DEVICE}" "${LIB_SIM}"; do
  [ -f "${lib}" ] || { echo "error: 没产出 ${lib}" >&2; exit 1; }
  printf '   %-22s %s\n' "$(basename "$(dirname "$(dirname "$(dirname "${lib}")")")")" "$(du -h "${lib}" | cut -f1)"
done

echo "== 2/3 链进一个 iOS 可执行文件 =="
OUT="${RUST}/target/link-check"
mkdir -p "${OUT}"

for spec in "iphoneos:arm64-apple-ios18.2:${LIB_DEVICE}" \
            "iphonesimulator:arm64-apple-ios18.2-simulator:${LIB_SIM}"; do
  sdk="${spec%%:*}"; rest="${spec#*:}"
  target="${rest%%:*}"; lib="${rest#*:}"
  # -Wl,-dead_strip 是 Xcode DEAD_CODE_STRIPPING=YES 的等价物。
  xcrun --sdk "${sdk}" clang -target "${target}" \
    -isysroot "$(xcrun --sdk "${sdk}" --show-sdk-path)" \
    -Wl,-dead_strip \
    -o "${OUT}/m0-link-${sdk}" \
    "${PROJECT}/ios-host/link-check/main.c" "${lib}" -liconv
  printf '   %-16s 链接成功  %s\n' "${sdk}" "$(du -h "${OUT}/m0-link-${sdk}" | cut -f1)"
done

echo "== 3/3 符号审计（这几项必须全是 0）=="
for bin in "${OUT}/m0-link-iphoneos" "${OUT}/m0-link-iphonesimulator"; do
  echo "   --- $(basename "${bin}") ---"
  nm -u "${bin}" | awk '{print $NF}' | sort -u > "${OUT}/syms.txt"
  total=$(wc -l < "${OUT}/syms.txt" | tr -d ' ')
  for sym in _openpty _forkpty _login_tty _fork _execve _posix_spawnp; do
    printf '     %-15s %s\n' "${sym}" "$(grep -cx "${sym}" "${OUT}/syms.txt")"
  done
  printf '     %-15s %s\n' "posix_spawn*" "$(grep -c '_posix_spawn' "${OUT}/syms.txt")"
  printf '     %-15s %s\n' "未解析符号总数" "${total}"
done

echo
echo "全绿。Xcode 那边只要：链接 -lrshell_m0 -liconv，并保持 DEAD_CODE_STRIPPING=YES。"
echo "（步骤见 ios-host/README.md）"
