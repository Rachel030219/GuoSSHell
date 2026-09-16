# GuoSSHell

一个 **纯 SSH 客户端**的 iOS / iPadOS 版（iPad 优先）。业务与终端语义跑在 **Rust**
（上游 [rsHell](https://github.com/hugefiver/rsHell) 的内核，作为 pin 住 rev 的 git 依赖），
只有渲染与交互用 **Flutter** 重写。Dart 不写业务逻辑。

> **实现期的唯一参考是 [`PLAN.md`](PLAN.md)。** 动代码之前先读它。

## 目录

```
GuoSSHell/
├── PLAN.md              实现规划：铁律 / 架构 / 实测事实 / 性能预算 / M0–M5 / 复用清单 / 陷阱
├── docs/                调研报告（可行性 + 里程碑与 M0 交接单）
├── lib/                 Flutter app（M1 起）
│   ├── main.dart        入口：initializeRust + MaterialApp
│   └── src/
│       ├── bindings/    rinf gen 生成的 Dart 绑定（**不手改**；rinf 默认不入库）
│       └── terminal/    帧解码 + 画布 painter + 会话页（纯展示，无终端状态）
├── native/hub/          rinf 的 Rust 侧（信号层 + 会话 actor + run 压缩编码器）
│   └── src/             lib.rs · signals/ · frame_codec.rs · session.rs
├── Cargo.toml           根 workspace（members = native/*）+ release profile
├── rust/                rshell-m0：M0 的 C ABI 探针 crate（**独立 workspace**，M0b 壳仍依赖它）
│   ├── Cargo.toml       上游走 git 依赖，rev 精确 pin
│   ├── UPSTREAM.md      上游来源、为什么用 git 依赖、许可
│   └── examples/        m0.rs · m0_loopback.rs · bench_frame.rs · demo_server.rs
├── ios/                 Flutter 的 iOS 宿主（Runner）
├── ios-host/            M0b 的最小 Swift 宿主 + 命令行链接检查（M1 绿了再删）
│   └── link-check/      「不开 Xcode 也能验链接」用的 C 入口
└── scripts/
    ├── setup.sh         幂等环境准备
    └── link-check.sh    M0b 起飞前检查（构建两个切片 → 链接 → 符号审计）
```

`native/hub` 与 `rust/` 是两个独立 workspace：M0b 的 `ios-host/build-rust.sh` 与
`scripts/link-check.sh` 依赖 `rust/target/` 的独立产物，所以在 M1 验收绿之前不能合并。
代价：上游 rev 在两处 Cargo.toml 各 pin 一次，**升级时要一起改**（见 UPSTREAM.md）。

## 快速开始

```bash
./scripts/setup.sh

# M0a：SSH 端到端，不需要外部服务器、不需要 Xcode、不需要真机
cd rust && cargo run --example m0_loopback

# 打真实服务器
cargo run --example m0 -- <host> <port> <user> <password>

# 性能基准（PLAN.md §4 的全部数字）
cargo run --release --example bench_frame

# M1：起一个无凭证的环回 SSH 服务器（probe / probe，密钥稳定不换）
cargo run --example demo_server -- 2222

# App（另开一个终端；iOS 模拟器可达宿主 127.0.0.1）
cd ..
flutter pub get
rinf gen                      # 改过 native/hub 的信号结构后要重跑
flutter run -d <模拟器id> \
  --dart-define=GUOSH_HOST=127.0.0.1 --dart-define=GUOSH_PORT=2222 \
  --dart-define=GUOSH_USER=probe --dart-define=GUOSH_PASS=probe
# 不带 GUOSH_* 时是正常的连接表单；
# GUOSH_CMD=top 可选——exec 模式（连上直接执行命令，M1 帧率实测用，无需键盘）。
```

M0b 的 Xcode 工程建法见 [`ios-host/README.md`](ios-host/README.md)。

## 上游与许可

上游 `hugefiver/rsHell` @ `b2ab8656079225dc2c920c24f5d9e0124f4f83e1`，MIT。
**上游源码零改动** —— 所以才能用 git 依赖而不是 fork。原因与代价见
[`rust/UPSTREAM.md`](rust/UPSTREAM.md)。

我们只依赖 4 个内核 crate（`rshell-core` / `rshell-session` / `rshell-platform` /
`rshell-storage`），**不用** `rshell-ui`（22,812 行 GTK4/Relm4 界面层，正是要用 Flutter 替掉的那层）。
