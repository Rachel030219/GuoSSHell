# GuoSSHell

一个 **纯 SSH 客户端**的 iOS / iPadOS 版（iPad 优先）。业务与终端语义跑在 **Rust**
（上游 [rsHell](https://github.com/hugefiver/rsHell) 的内核，作为 pin 住 rev 的 git 依赖），
只有渲染与交互用 **Flutter** 重写。Dart 不写业务逻辑。

> **实现期的唯一参考是 [`PLAN.md`](PLAN.md)。** 动代码之前先读它。

## 目录

```
GuoSSHell/
├── PLAN.md              实现规划：铁律 / 架构 / 实测事实 / 性能预算 / M0–M4 / 复用清单 / 陷阱
├── docs/                调研报告（可行性 + 里程碑与 M0 交接单）
├── rust/                本仓库的 Rust 代码（**不含上游源码**）
│   ├── Cargo.toml       独立 workspace 根；上游走 git 依赖，rev 精确 pin
│   ├── UPSTREAM.md      上游来源、为什么用 git 依赖、许可
│   ├── Cargo.lock       进版本控制（App 仓库 + git 依赖，复现性靠它）
│   ├── LICENSES/        上游 rsHell / portable-pty-psmux 的 MIT 许可副本
│   ├── src/lib.rs       M0：SSH 直连 + C ABI + 引擎/帧前置验证
│   └── examples/        m0.rs · m0_loopback.rs · bench_frame.rs
├── ios-host/            M0b 的最小 Swift 宿主 + 命令行链接检查
│   └── link-check/      「不开 Xcode 也能验链接」用的 C 入口
└── scripts/
    ├── setup.sh         幂等环境准备
    └── link-check.sh    M0b 起飞前检查（构建两个切片 → 链接 → 符号审计）
```

Flutter app 之后落在仓库根（`lib/`、`ios/`、`macos/`…）。Rust 一律放 `rust/`，
**不要用 `ios/` 这个名字**——那是 Flutter 自己占用的。

## 快速开始

```bash
./scripts/setup.sh

# M0a：SSH 端到端，不需要外部服务器、不需要 Xcode、不需要真机
cd rust && cargo run --example m0_loopback

# 打真实服务器
cargo run --example m0 -- <host> <port> <user> <password>

# 性能基准（PLAN.md §4 的全部数字）
cargo run --release --example bench_frame

# M0b 起飞前检查：两个切片都能链进 iOS 可执行文件，且 PTY/fork 符号为 0
cd .. && ./scripts/link-check.sh
```

M0b 的 Xcode 工程建法见 [`ios-host/README.md`](ios-host/README.md)。

## 上游与许可

上游 `hugefiver/rsHell` @ `b2ab8656079225dc2c920c24f5d9e0124f4f83e1`，MIT。
**上游源码零改动** —— 所以才能用 git 依赖而不是 fork。原因与代价见
[`rust/UPSTREAM.md`](rust/UPSTREAM.md)。

我们只依赖 4 个内核 crate（`rshell-core` / `rshell-session` / `rshell-platform` /
`rshell-storage`），**不用** `rshell-ui`（22,812 行 GTK4/Relm4 界面层，正是要用 Flutter 替掉的那层）。
