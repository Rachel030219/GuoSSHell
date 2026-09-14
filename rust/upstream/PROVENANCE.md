# PROVENANCE — 上游来源与许可

`upstream/` 目录里的源码来自 **rsHell**，**已 vendor 进本仓库**（不是 git 依赖、不是 submodule）。

| 项 | 值 |
|---|---|
| 仓库 | https://github.com/hugefiver/rsHell |
| 基线 commit | `b2ab8656079225dc2c920c24f5d9e0124f4f83e1` |
| commit 日期 | 2026-09-14 |
| 许可证 | MIT（见 `LICENSE`） |

## 为什么要 vendor

上游是「业务内核 + GTK 界面」两层。本项目的目标是**换掉界面层、保留业务内核**，
这必然要改动上游代码，因此选择 fork-and-own，而不是把它当外部依赖。
明确不与上游保持同步（这是项目决定，见 `PLAN.md` §8）。

## vendor 了什么

| 路径 | 说明 |
|---|---|
| `crates/rshell-core` | 领域模型、连接目录、终端设置、渲染帧类型、可序列化的命令/事件协议 |
| `crates/rshell-session` | SSH 传输、alacritty 适配、终端引擎、actor、会话管理 |
| `crates/rshell-platform` | 平台适配（iOS 上需要新分支） |
| `crates/rshell-storage` | SQLite 持久化与凭证仓库 |
| `third_party/portable-pty-psmux` | 上游对 portable-pty 0.9 的 fork（去掉会死锁的 Windows INHERIT_CURSOR DSR）。本项目在 iOS 上不用本地 PTY，但保留以免补丁声明失配 |
| `LICENSE` `README.md` `DESIGN.md` `AGENTS.md` `Cargo.lock` | 许可、原文档、原设计权威、依赖锁定快照 |

## 没有 vendor 什么，以及为什么

| 路径 | 原因 |
|---|---|
| `crates/rshell-ui`（22,812 行） | GTK4/Relm4 界面层，正是要用 Flutter 替掉的那一层 |
| 根 `src/`（4,215 行） | GTK 主循环与 App 装配，同上 |
| `tests/`、`resources/`、`scripts/` | 围绕 GTK 应用目标的构建与测试脚手架，本项目不构建它 |
| `docs/` | 上游的内部设计文档，与本题无关 |

## 本仓库对上游做的改动

改动处一律用 `// [GuoSSHell 改动]`（Rust）或 `# [GuoSSHell 改动]`（TOML/SQL）标注。

1. `crates/rshell-storage/Cargo.toml` —— 增加

   ```toml
   [target.'cfg(target_os = "ios")'.dependencies]
   apple-native-keyring-store = { version = "1.0.1", features = ["protected"] }
   ```

   原因：`keyring 4.1.5` 的 `v1` feature 在 iOS 上只启用
   `apple-native-keyring-store/keychain`，而 iOS 没有 legacy keychain，只有 protected data store。
   不补会直接编译失败：`error: The 'protected' feature is required on iOS`。

截至 2026-09-14，这是唯一一处改动。
