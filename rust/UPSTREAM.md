# UPSTREAM.md —— 上游来源、依赖方式与许可

本项目是**基于 rsHell 改出来的独立仓库**。上游内核不在本仓库里，而是作为
**git 依赖**被 pin 到一个 commit。

| 项 | 值 |
|---|---|
| 仓库 | https://github.com/hugefiver/rsHell |
| 基线 commit | `b2ab8656079225dc2c920c24f5d9e0124f4f83e1` |
| 许可证 | MIT，Copyright (c) 2026 hugefiver（副本见 `LICENSES/rsHell-MIT.txt`） |

依赖声明在 `Cargo.toml`：

```toml
rshell-core = { git = "https://github.com/hugefiver/rsHell", rev = "b2ab8656079225dc2c920c24f5d9e0124f4f83e1" }
rshell-session = { git = "https://github.com/hugefiver/rsHell", rev = "b2ab8656079225dc2c920c24f5d9e0124f4f83e1" }
```

## 为什么是 git 依赖，而不是 vendor 进仓库

一开始是把上游四个 crate 的源码 vendor 进 `rust/upstream/`（211 个文件 / 1.8 MB）。
改用 git 依赖的原因：

- 仓库干净：本仓库只装**我们自己的**代码。
- provenance 就写在一行 `rev = "..."` 里，比一个副本目录更难说谎。
- **前提是上游源码零改动**——这一点是实测确认的，见下。

代价（必须知道）：

1. **全新环境首次构建需要网络。** cargo 要把仓库 clone 进 `~/.cargo/git/`。
   离线机器要先 `cargo fetch`。
2. **上游代码只读。** 一旦需要改上游（哪怕一行），就必须先 fork，
   然后把 `rev=` 换成自己 fork 的 commit。这不是理论风险，见下面第 3 条。

## 上游源码是零改动的 —— 以及为此做的三件事

### 1. iOS 的 keyring `protected` feature：不需要改上游

上游 `rshell-storage` 用 `keyring 4.1.5`，其 `v1` feature 在 iOS 上只启用
`apple-native-keyring-store/keychain`，而 iOS 没有 legacy keychain，只有
protected data store，于是直接编译失败：

```
error: The `protected` feature is required on iOS
```

之前为了过这一关去改了上游的 `Cargo.toml`。**现在不用了**：Cargo 的 feature 是
按**包**统一（unification）的，只要依赖图里**任何一个人**打开了它，
`rshell-storage` 那条路径也会看到。所以在我们自己的 `Cargo.toml` 里加：

```toml
[target.'cfg(target_os = "ios")'.dependencies]
apple-native-keyring-store = { version = "1.0.1", features = ["protected"] }
```

实测有效：`cargo check --target aarch64-apple-ios` 通过，上游零改动。

### 2. `[patch.crates-io] portable-pty-psmux`：我们不需要

上游根 crate 有一个指向 `third_party/portable-pty-psmux` 的 patch。
读了那份目录里的 `README.rshell-patch.md` 才知道：rsHell 对它的改动**全部在
`src/win/*`**（Windows ConPTY 的 Job handle 处理）加一个只给 dev-dependencies 用的
`containment-test-support` feature。**我们的目标是 iOS / macOS，这些文件根本不参与编译。**

而且这里还藏着一个必然性：`rshell-session` 的 `[dev-dependencies]` 里引用了
`containment-test-support` 这个 feature，而它在 crates.io 上的
`portable-pty-psmux 0.9.6` 里不存在。**cargo 会解析 path 依赖的 dev-dependencies**，
所以只要 `rshell-session` 是 path 依赖，去掉 patch 就会直接报：

```
package `rshell-session` depends on `portable-pty-psmux` with feature
`containment-test-support` but `portable-pty-psmux` does not have that feature.
```

换成 **git 依赖后这个问题自己消失了**（cargo 不解析 git 依赖的 dev-dependencies）。
实测：去掉 patch、纯 git 依赖，`cargo check --target aarch64-apple-ios` 通过。
补丁说明留档在 `LICENSES/portable-pty-psmux-PATCH-NOTES.md`。

### 3. 已知的上游改动需求（还没做，是 git 依赖的第一个真实代价）

链接出来的 iOS 可执行文件**会导入** `_openpty` / `_login_tty` / `_fork` /
`_posix_spawnp` 等符号——它们来自 `rshell-session` 里那三个 iOS 不可用的传输
（`local` / `pty` / `system_ssh`），我们从不调用，但**符号被保留下来了**。

**暂时的结论是这不影响交付**：加 `-Wl,-dead_strip`（Xcode 的
`DEAD_CODE_STRIPPING = YES`，Release 默认开）后这些导入**全部消失**，
二进制从 13 MB 降到 3.7 MB。实测见 `PLAN.md` §3.2。

**但如果将来需要把这些传输从编译图里彻底摘掉**（而不是靠链接器裁），
就必须 fork 上游把 `transport/local.rs` + `pty.rs` 用 feature gate 关掉——
那就是「改上游」，就得按上面第 2 点先 fork。

## 顺带：`LICENSES/`

| 文件 | 内容 |
|---|---|
| `rsHell-MIT.txt` | 上游 rsHell 的 MIT 许可（Copyright (c) 2026 hugefiver） |
| `portable-pty-psmux-MIT.md` | `portable-pty-psmux` 的 MIT 许可（Wez Furlong） |
| `portable-pty-psmux-PATCH-NOTES.md` | 上游那份 patch 的说明，留档——我们现在不应用它了 |

MIT 要求「许可声明随软件或其重要部分一起分发」。App Store 提交时需要一份
第三方许可清单，这三个文件就是它的起点。
