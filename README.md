# GuoSSHell

一个 **纯 SSH 客户端**的 iOS / iPadOS 版。业务与终端语义跑在 **Rust**（从 [rsHell](https://github.com/hugefiver/rsHell)
vendor 进来的内核），只有渲染与交互用 **Flutter** 重写。Dart 不写业务逻辑。

> **实现期的唯一参考是 [`PLAN.md`](PLAN.md)。** 动代码之前先读它。

## 目录

```
GuoSSHell/
├── PLAN.md              实现规划：铁律 / 架构 / 实测事实 / 性能预算 / M0–M4 / 复用清单 / 陷阱
├── docs/                调研报告（可行性 + 里程碑与 M0 交接单）
├── rust/                全部 Rust 代码
│   ├── Cargo.toml       独立 workspace 根；当前是我们的 M0 crate
│   ├── src/lib.rs       SSH 直连 + C ABI + 引擎/帧前置验证
│   ├── examples/        m0.rs · m0_loopback.rs · bench_frame.rs
│   └── upstream/        vendored 的上游 rsHell 内核（见 upstream/PROVENANCE.md）
│       ├── crates/      rshell-core / rshell-session / rshell-platform / rshell-storage
│       └── third_party/ portable-pty-psmux（上游的 fork）
├── ios-host/            临时 Swift 宿主，用来在真机验证 M0b（M1 换成 Flutter 后废弃）
└── scripts/setup.sh     幂等环境准备
```

Flutter app 之后落在仓库根（`lib/`、`ios/`、`macos/`…）。Rust 一律放 `rust/`，
**不要用 `ios/` 这个名字**——那是 Flutter 自己占用的。

`rust/upstream/` 是**被 vendor 进来的源码**，不是 git 依赖，也不与上游保持同步——
我们可以直接改它，改动处用 `[GuoSSHell 改动]` 标注。唯一已做的改动是
`rust/upstream/crates/rshell-storage/Cargo.toml` 里给 iOS 打开 keyring 的 `protected` feature。

## 快速开始

```bash
./scripts/setup.sh

# M0a：SSH 端到端，不需要外部服务器、不需要 Xcode、不需要真机
cd rust && cargo run --example m0_loopback

# 打真实服务器
cargo run --example m0 -- <host> <port> <user> <password>

# 性能基准（PLAN.md §4 的全部数字）
cargo run --release --example bench_frame

# iOS 静态库
cargo build --release --lib --target aarch64-apple-ios
```

真机验证（M0b）见 [`ios-host/README.md`](ios-host/README.md)。

## 上游来源

`hugefiver/rsHell` @ `b2ab8656079225dc2c920c24f5d9e0124f4f83e1`（2026-09-14），MIT。
许可与出处说明见 [`rust/upstream/PROVENANCE.md`](rust/upstream/PROVENANCE.md)。

未 vendor 的部分：`rshell-ui`（22,812 行 GTK4/Relm4 界面层，正是要用 Flutter 替掉的那一层）、
GTK 根 crate（`src/`、`tests/`、`resources/`）以及 `docs/`。
