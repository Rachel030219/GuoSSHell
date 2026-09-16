#!/usr/bin/env bash
# GuoSSHell 环境准备。幂等，可重复执行。
#
# 上游内核是 git 依赖（pin 到具体 commit，见 rust/UPSTREAM.md），
# 所以这里不需要克隆任何东西，但首次需要联网让 cargo 把它拉下来。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="$(cd "${HERE}/.." && pwd)"
RUST="${PROJECT}/rust"

# cargo / rustup / rinf 通常装在 ~/.cargo/bin，但非登录 shell 常常没有这个 PATH。
if [ -x "${HOME}/.cargo/bin/cargo" ]; then
  export PATH="${HOME}/.cargo/bin:${PATH}"
fi

CARGO=""
for candidate in "$(command -v cargo 2>/dev/null || true)" "${HOME}/.cargo/bin/cargo"; do
  if [ -n "${candidate}" ] && [ -x "${candidate}" ]; then CARGO="${candidate}"; break; fi
done

RUSTUP=""
for candidate in "$(command -v rustup 2>/dev/null || true)" "${HOME}/.cargo/bin/rustup"; do
  if [ -n "${candidate}" ] && [ -x "${candidate}" ]; then RUSTUP="${candidate}"; break; fi
done

echo "== 1/3 检查 git 依赖 =="
grep -q 'git = "https://github.com/hugefiver/rsHell", rev = "b2ab865' "${RUST}/Cargo.toml" \
  || { echo "error: rust/Cargo.toml 里找不到 pin 住的上游 git 依赖" >&2; exit 1; }
if [ -n "${CARGO}" ] && [ -f "${RUST}/Cargo.lock" ]; then
  # 离线也要能过；拉不到只是提示，不算失败。
  (cd "${RUST}" && "${CARGO}" fetch --locked) || \
    echo "   警告：cargo fetch 失败（可能是网络）。首次构建需要能访问 GitHub。"
fi
echo "   ok"

echo "== 2/3 Rust iOS 编译目标 =="
if [ -z "${RUSTUP}" ]; then
  echo "   警告：找不到 rustup。装上 Rust 后重跑本脚本。" >&2
  echo "         https://rustup.rs" >&2
else
  "${RUSTUP}" target add aarch64-apple-ios aarch64-apple-ios-sim
  echo "   已装目标："
  "${RUSTUP}" target list --installed | sed 's/^/     /'
fi

echo "== 3/3 工具链 =="
if [ -n "${CARGO}" ]; then
  "${CARGO}" --version | sed 's/^/   /'
  for tool in rinf cargo-ndk protoc-gen-prost; do
    [ -x "${HOME}/.cargo/bin/${tool}" ] && echo "   已装：${tool}"
  done
fi

if command -v flutter >/dev/null 2>&1; then
  flutter --version | head -1
else
  # Flutter 常由 fvm 之类的版本管理器安装，不在默认 PATH 里，顺手探测一下。
  found=""
  for candidate in "${HOME}/fvm/default/bin" "${HOME}/fvm/versions"/*/bin; do
    if [ -x "${candidate}/flutter" ]; then found="${candidate}"; break; fi
  done
  if [ -n "${found}" ]; then
    echo "   flutter 在 ${found}（不在 PATH，用前 export PATH=\"${found}:\$PATH\"）"
    "${found}/flutter" --version | head -1
  else
    echo "   警告：没找到 flutter"
  fi
fi

if ! xcrun simctl list runtimes 2>/dev/null | grep -q "^iOS"; then
  echo "   提示：没有安装任何 iOS 模拟器 runtime（不影响 My Mac (Designed for iPad)）。"
  echo "         需要模拟器时再补：Xcode → Settings → Components，或 xcodebuild -downloadPlatform iOS"
fi

echo
echo "完成。接着跑："
echo "  cd ${RUST}"
echo "  cargo run --example m0_loopback                        # 不需要外部服务器"
echo "  cargo run --release --example bench_frame              # 性能基准"
echo "  cargo build --release --lib --target aarch64-apple-ios # iOS 静态库"
