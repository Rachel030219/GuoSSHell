# PLAN.md — GuoSSHell

> 把 rsHell 的 **Rust 业务内核**原样搬到一个 Flutter 前端的 iOS / iPadOS 应用上。
> 这份文件是实现期的唯一参考。所有结论都标注了证据来源；标「实测」的都是本机跑出来的，可复现。

- 上游基线：`hugefiver/rsHell` @ `b2ab8656079225dc2c920c24f5d9e0124f4f83e1`（2026-09-14，MIT），
  已 **vendor 进本仓库**在 `rust/upstream/`（见 `rust/upstream/PROVENANCE.md`）
- 应用名：**GuoSSHell**
- 开发环境：macOS（Apple Silicon）· Xcode 16+ · Flutter 3.x（**本机具体版本号、工具绝对路径一律不入库**，见 §7）
- 相关文档：
  - `docs/feasibility-2026-09-14.html`（可行性：alacritty 四层判定、rinf 事实纠正）
  - `docs/mvp-plan-2026-09-14.html`（里程碑与 M0 交接单）
  - `rust/`（可运行的 M0 代码 + `bench_frame` 性能基准）、`ios-host/`（Swift 真机宿主）

---

## 0. 铁律

1. **业务与终端语义全部跑在 Rust**，能不改就不改。Dart **不写业务逻辑**。
2. **只有渲染、交互这些必不可少的部分用 Flutter 重写。**
3. **优先复用，绝不造轮子。** 具体顺序是：
   1. pub.dev / crates.io 有能用的 → 直接用；
   2. 有接近的但不够好 → **在已有开源项目上 fork**，不要从零写；
   3. 只有确认前两条都不成立才自己写，并且要在本文件里写下「为什么没有可复用的」。
   这条对每一层都适用：渲染、输入、网络发现、连接存储，全都先找现成的。
4. **Rust 是终端状态的唯一权威。** Dart 侧不得持有第二份终端状态机——这条是选技术方案的硬约束，
   任何让 Dart 也解析一遍 ANSI 的方案（例如直接用 `xterm.dart` 的 `Terminal`）都直接排除。
5. **所有产物都放在 `GuoSSHell/` 里。** 不要在存放多个项目的容器目录里留文件。
   仓库结构：Flutter app 在根，Rust 一律在 `rust/`（**不要占用 `ios/`**，那是 Flutter 自己的目录）。
   临时文件（探针、半成品克隆）用 `/tmp`，用完即走。

---

## 1. 项目定义

**GuoSSHell = 一个纯 SSH 客户端的 iOS / iPadOS 版本。**

明确不做的事：**不做本地 shell 面板**。上游 rsHell 的「本地终端」是一等公民，
但 iOS 上不可能（没有 `fork`、没有 `openpty`、沙箱禁止 spawn 进程），所以 iOS 版
把产品模型收窄为「远端 SSH 终端」。Rust 侧零改动——只要 Flutter 侧不暴露
`UiCommand::NewLocalTab` / `UiCommand::StartLocal` 就行。

上游是 MIT，fork 与改名合规，但**必须保留原始版权与许可声明**。

---

## 2. 架构

三层，边界已经存在于上游代码里，**不需要重新设计**。

```
┌──────────────────────────────────────────────────────────────┐
│ Flutter（全量重写，约 2.7 万行规模）                            │
│  终端画布 / 选择 / IME / 软键盘 / 连接表单 / 侧栏 / 标签 / 分屏  │
└──────────────┬───────────────────────────────────────────────┘
               │  rinf：单向信号流（Dart → Rust 命令，Rust → Dart 事件）
               │  ⚠ 边界上的类型全部来自 rshell-core::protocol，不自己定义
┌──────────────┴───────────────────────────────────────────────┐
│ Rust（保留，约 1.1 万行）                                       │
│  rshell-core     4,240 行  领域模型 / 连接目录 / 设置 / 渲染帧类型 │
│  rshell-session  7,036 行  SSH 传输 / alacritty 适配 / 引擎 / actor│
│  ─────────────────────────────────────────────────────────────│
│  rshell-platform 1,306 行  → 需要 iOS 分支                      │
│  rshell-storage  4,785 行  → SQLite 可用；keyring 走 protected  │
└──────────────┬───────────────────────────────────────────────┘
               │  russh + tokio TCP
┌──────────────┴───────────────────────────────────────────────┐
│ 远端主机（PTY 与 shell 都在这一侧）                              │
└──────────────────────────────────────────────────────────────┘
```

### 2.1 边界已经存在（这是整件事能成立的原因）

`rshell-core::protocol` 里已经有一整套可序列化的命令/事件：

| 层级 | 类型 | 用途 | serde |
|---|---|---|---|
| 应用级 | `UiCommand` / `AppEvent` | 连接目录、搜索、设置、导入、分屏、标签、PaneId | 已有 |
| 会话级 | `SessionUiCommand` / `SessionUiEvent` | 输入、鼠标、粘贴、缩放、滚动、搜索、选择、交互应答 | 已有 |
| 交互 | `InteractionRequest` / `InteractionResponse` / `HostKeyPrompt` | 主机密钥确认、密码、私钥口令、keyboard-interactive | 请求已有；**响应刻意没有**（见下） |
| 渲染 | `RenderFrame` / `RenderRow` / `RenderCell` / `TerminalSize` / `Viewport` | 帧数据 | 已有 |

上游的 GTK 层（`rshell-ui`，22,812 行）就是靠这套类型跟内核说话的。**换掉 GTK，边界不用动。**

`SessionClient` 的形状也天然对齐 rinf：

```rust
pub struct SessionClient {
    pub commands: mpsc::Sender<SessionCommand>,          // → DartSignal
    pub events: broadcast::Receiver<SessionEvent>,        // → RustSignal
    pub frames: watch::Receiver<Option<Arc<RenderFrame>>>,// → 只留最新，正好是丢帧背压
}
```

### 2.2 两个边界上的硬约束

1. **秘密不能序列化。** `SessionUiCommand::Paste(SecretString)` 与
   `InteractionResponse::Secret(SecretString)` 刻意**没有** serde 派生。
   Dart 侧只能传明文，Rust 侧在接收处自己包成 `SecretString`。
   这意味着这一层协议要手写，不能全 `derive`。
2. **`serde` 必须开 `rc` feature。** `RenderFrame.rows` 与 `RenderRow.cells` 的类型是
   `Arc<[..]>`，serde 默认不给 `Arc` 实现序列化。（实测：不开就在 `encode` 处报 trait 不满足。）

---

## 3. 关键事实清单（实测，不是推测）

### 3.1 传输层在 iOS 上的可达性

| 传输 | 位置 | iOS | 原因 |
|---|---|---|---|
| `native_ssh`（russh） | `transport/native_ssh.rs` | <span>✅ 可用</span> | 纯 `russh` + `tokio::net::TcpStream`。PTY 开在**远端**，本地零 `fork` |
| `system_ssh` | `transport/system_ssh.rs` | ❌ 不可用 | 靠 `build_system_ssh_argv` 启动系统 `ssh`。沙箱禁止 spawn |
| `local` | `transport/local.rs` | ❌ 不可用 | portable-pty，需要 `fork` + `openpty` |
| `pty` | `transport/pty.rs` | ❌ 不可用 | 同上，是 `local` 的底层 |

**关键点**：这三个不可用的传输都在 `rshell-session` 内部，**不需要改动或删除**。
Flutter 侧不暴露入口即可。

### 3.2 编译与链接

- `alacritty_terminal 0.26.0` 全链为 `aarch64-apple-ios` **零改动编译通过**。
- `rshell-core` / `rshell-platform` / `rshell-storage` / `rshell-session` 为 iOS 编译通过。
- **唯一硬阻塞**：`rshell-storage/Cargo.toml` 需要追加

  ```toml
  [target.'cfg(target_os = "ios")'.dependencies]
  apple-native-keyring-store = { version = "1.0.1", features = ["protected"] }
  ```

  原因：`keyring 4.1.5` 的 `v1` feature 在 iOS 上只启用
  `apple-native-keyring-store/keychain`，而 iOS 没有 legacy keychain，只有 protected data store。
  不补会直接 `error: The 'protected' feature is required on iOS`。

- 为 iOS 完整链接出的 dylib，未解析符号里
  `openpty` / `forkpty` / `login_tty` / `fork` / `posix_spawn` / `execve` **命中数 0**。
  只剩 libSystem 的 POSIX（socket / connect / getaddrinfo / kqueue / pthread / malloc）
  与 CommonCrypto 的 `_CCRandomGenerateBytes`。
  静态库 `.a` 里残留 7 处 PTY 引用，但最终链接被裁掉——**死代码，不卡人**。

- 体积（iOS device）：

  | 产物 | 大小 |
  |---|---|
  | `librshell_m0.a`（release，未 strip） | 43.8 MB |
  | `librshell_m0.dylib`（release，已 strip） | **3.72 MB** ← Rust 内核真实增量 |
  | `librshell_m0.dylib`（debug，已 strip） | 9.06 MB |

### 3.3 M0 端到端（不需要任何外部服务器）

`rshell-ios-m0/examples/m0_loopback.rs` 在同进程里起一个最小 russh 服务端，
用 rsHell 自己的 `NativeSshTransport` 走完整路径。实测 transcript：

```
PTY-ACK term=xterm-256color cols=80 rows=24 px=0x0
M0-LOOPBACK-SHELL-OK
$ echo M0-ECHO
```

证明：TCP + 握手 + 主机密钥 TOFU 落盘 + 密码认证 + `request_pty(真实参数)` +
`request_shell` + **写路径** + 读路径，全部通。

### 3.4 代码规模

| 位置 | 行数 | iOS 处置 |
|---|---|---|
| `rshell-core` | 4,240 | 全量保留 |
| `rshell-session` | 7,036 | 保留（去掉三个不可用传输） |
| `rshell-platform` | 1,306 | 需要 iOS 分支 |
| `rshell-storage` | 4,785 | 基本保留 |
| **`rshell-ui`** | **22,812** | **全部重写** |
| 根 `src/` | 4,215 | **全部重写**（业务逻辑在 `rshell-core::application`，1,853 行） |
| 合计 | 44,394 | 保留 11,276（25%），重写 27,027（61%） |

> **要重写的 Flutter 层是它对话的 Rust 内核的 2.4 倍。** 这是这个项目的真实形状。

---

## 4. 性能预算（回答「整帧传 RenderFrame 会不会有瓶颈」）

实测命令：`cd rust && cargo run --release --example bench_frame`

### 4.1 iPad 横屏 120×40（4,800 cells/帧，主循环预算 16.67 ms）

| 负载 | `render()` | run/行 | run 总数 | serde_json | bincode | run 打包 | 脏 3 行 |
|---|---|---|---|---|---|---|---|
| `ls --color` | 0.14 ms | 5.0 | **200** | 836.5 KB | 52.9 KB | **7.2 KB** | **0.5 KB** |
| `git diff` | 0.11 ms | 1.8 | **73** | 837.1 KB | 52.6 KB | **5.8 KB** | **0.4 KB** |
| TUI 满屏（最坏） | 0.11 ms | 120.0 | **4,800** | 848.5 KB | 56.4 KB | **61.2 KB** | 4.6 KB |
| CJK 日志 | 0.10 ms | 2.0 | **80** | 699.1 KB | 44.8 KB | **6.6 KB** | 0.5 KB |

编码耗时：bincode 0.06–0.09 ms · run 打包 0.03–0.13 ms。

### 4.2 iPhone 竖屏 40×22（880 cells/帧）

| 负载 | `render()` | run 总数 | serde_json | bincode | run 打包 | 脏 3 行 |
|---|---|---|---|---|---|---|
| `ls --color` | 0.02 ms | 26 | 154.3 KB | 9.6 KB | 1.3 KB | 0.2 KB |
| TUI 满屏 | 0.02 ms | 880 | 156.6 KB | 10.4 KB | 11.3 KB | 1.5 KB |
| CJK 日志 | 0.02 ms | 43 | 109.7 KB | 7.3 KB | 1.7 KB | 0.2 KB |

### 4.3 结论

**瓶颈存在，但不在你担心的位置。**

1. **Rust 侧不是瓶颈。** `render()` + 编码总计 **< 0.3 ms**，预算 16.67 ms，余量 50× 以上。
   即使 `RenderCell.text: String` 会给每个 cell 分配一次堆内存（4,800 次/帧），也不够看。
2. **`serde_json` 是禁区。** 700–850 KB/帧，因为每个空格都要写成 `" "` 加字段名。
   60 fps 就是 50 MB/s。**绝对不要用它做帧传输。**
3. **bincode 能用但浪费。** 固定 ~50 KB/帧，因为它是无压缩的逐 cell 编码，
   `RenderCell` 的每个字段（含 `String` 长度前缀）都要过一遍线。60 fps ≈ 3.2 MB/s。
4. **真正的成本在 Dart 侧的对象图。** 逐 cell 传 4,800 个对象，Dart 每帧要建
   ~4,800 个小对象 + 做 ~4,800 次文本布局。60 fps 下是 **28.8 万对象/秒**，
   GC 和文本布局会被打爆。**这才是「整帧传会不会有瓶颈」的正确答案。**
5. **解法：run 压缩 + 脏行增量。** 把相邻同属性单元格合并成一个 run 再打包：

   | | 典型 shell 输出 | TUI 满屏（最坏） |
   |---|---|---|
   | Dart 侧要建的对象数 | 4,800 → **73–200**（**24×–65×**） | 4,800（无法压缩） |
   | 每帧字节数 | 52.9 KB → **5.8–7.2 KB**（**7×–9×**） | 61.2 KB |
   | 再加脏行增量 | **0.4–0.5 KB** | 4.6 KB |

   TUI 满屏无法压缩是物理事实（每个 cell 独立底色），但它也几乎不会以 60 fps 刷新
   ——`htop` 默认 1.5 s 一次。真正的 60 fps 场景是 `tail -f`、编译日志、
   `ls` 之类，恰好都是 run 压缩效果最好的形状。

6. **落地方案：结构化帧 + run 压缩 + 脏行增量，走 rinf 的 `RustSignalBinary` 原始字节通道。**

   - 不改 `rshell-core` 的 `RenderFrame` 契约（它是上游的核心资产）。
   - 在 iOS 侧的适配层加一个「`RenderFrame` → 紧凑 run 字节流」编码器。
   - **不需要自己设计序列化框架**：`RustSignalBinary` 就是 rinf 为这种情况准备的逃生舱。
   - 位图方案（Rust 侧渲染成像素）**不采用**：一旦 rinf 丢帧就是花屏（必须带
     generation 号重传），文本不可选，字体要内嵌进 Rust，DPI 变化要重渲。
     run 压缩后已经落在 7 KB/帧量级，没有理由付这些代价。

---

## 5. 里程碑

每个里程碑只增加**一类**未知量。前一个是绿的前提下才做下一个。**验收顺序：iPad 先通，再谈 iPhone / iOS。**

### M0 — SSH 直连

写死地址和密码，先证明 SSH 本身在 iOS 上可行。无渲染、无输入、无 UI。

- **M0a（macOS 命令行，不需要 Xcode）** —— <span>已完成</span>
  `cargo run --example m0_loopback`（不需要外部服务器）
  `cargo run --example m0 -- <host> <port> <user> <password>`（打真实服务器）
- **M0b（在 iOS App 里跑起来）** —— 宿主用 `ios-host/` 里的 Swift 壳。
  产物是 `librshell_m0.a`，**它不是 App，必须链进一个 iOS App 目标才能跑**。
  两种跑法，按代价从低到高：
  1. **"My Mac (Designed for iPad)"**（M1 可用，**零下载**）——把 iOS App 目标直接跑在 macOS 上，
     走的是 iPad 的二进制与 iOS 沙箱，不是 macOS 版。
  2. **iPad 模拟器** —— 需要先补装模拟器 runtime（本机当前为空，见 §7）。
  3. 真机（iPhone 已注册两台，可选、非阻塞）。

  > 诚实标注：模拟器与 "Designed for iPad" 跑在 macOS 用户态，**不能**用来证明
  > 「本地 PTY 在 iOS 上不工作」这类结论（会给出错误答案，见 §10.1）。
  > 但它们**可以**证明「iOS App 沙箱内、russh 出站 TCP 全链路能通」——
  > 因为这里唯一涉及平台差异的就是 socket 与 App 沙箱。
  > 真机与模拟器的差异（蜂窝、后台挂起、内存上限）留到 M1 与 M4 处理。

**验收**：App 内拿到远端提示符；失败时返回明确的 `SessionFailure` 分类
（`Network` / `Authentication` / `HostKey` / `SshChannel`）；写路径回显成功；
`known_hosts` 在 App 沙箱内落盘，第二次连接不再触发主机密钥交互。

**子项 M0-c：本地网络连接（内网）**

- 公网机优先做，但内网连接是迟早要有的。
- 需要 `NSLocalNetworkUsageDescription`（`NSBonjourServices` 只在用 Bonjour 发现时必需）。
- **iOS 没有「主动申请本地网络权限」的 API**，权限只能被第一次实际访问触发；
  未授权时 `connect()` **静默超时而不是报错**，所以代码必须把超时当作
  「可能没授权」处理，并提供跳转设置页的引导。
- **本期范围：手工填内网地址 + 权限处理 + 失败提示，不做自动发现**（见 §9）。
  权限被拒后「跳系统设置页」的引导要找现成实现复用，**不要自己写**（见 §6.3）。

### M1 — 一帧终端画面

字节 → `DefaultTerminalEngine::advance()` → `render(viewport, selection)` →
`RenderFrame` → run 压缩 → rinf 信号 → Flutter 画出来。**只读**，不处理输入。

> **起手式**：用 `rinf template` 在仓库根铺 Flutter 骨架（本机 `rinf` CLI 已装，见 §7），
> 再用 `rinf gen` 从带属性的 Rust 结构生成 Dart 侧类型。**不要手写桥接样板**。

**验收**：
- iPad 上出现完整着色的终端画面，`ls --color=always` / `git log --color` 颜色正确。
- CJK 按 2 列宽渲染、emoji 与零宽字符不错位。
- 旋转 / 改窗口 → `SessionUiCommand::Resize` → 远端 `WindowChange` → 远端 `stty size` 变化。
- `top` / `htop` 连续刷新下掉帧可接受；rinf 丢帧表现为**晚一帧**而不是花屏。
- **必须在这一步把帧率压到目标值**，不能留到 M4——这是整条路线唯一需要提前验证的技术风险。

**子项 M1-a：iPad 验证（**不需要真实 iPad 设备**）**

两条路，都不用买平板：

1. **"My Mac (Designed for iPad)"** —— 在 M1 上跑 **iPad 版**（不是 macOS 版）。
   用来做高频迭代：改一行看一眼，秒级反馈。
   注意目标名必须选 `Designed for iPad` 那个，**不能**选 My Mac 的 macOS 目标——
   后者是 macOS 二进制，跑通了也证明不了任何 iOS 的事。
2. **iPad 模拟器** —— 更接近真机的运行时环境（真实 iOS 系统库、真实 UIKit/IME 栈）。

验收标准按「iPad 版二进制在 iOS 运行时上跑通」来定，不按「macOS 上跑通」来定。
两者都不覆盖：jetsam 内存上限、真实触摸/IME 时序、App 审核沙箱限制——那些留给 M4 与真机。

**子项 M1-b：iPhone 适配（iPad 通了之后再做）。** 小屏 + 软键盘是两套独立问题，单独立项。
本机已有两台 iPhone（13 mini / 12 mini）可用；先用模拟器，真机可选。

### M2 — 输入闭环

Flutter 的输入 → `TerminalInput::{CommittedText, Key{code, modifiers}}` →
Rust `encode_input()`（alacritty 键编码，含 Kitty / CSI-u 协商）→ `transport.write()`。

**验收**：iPad 上能跑通 `vim` 并保存；`Ctrl+C` 精确发一个 `ETX`；
中文 IME 的 **preedit 绝不外发**；`Ctrl`/`Esc`/`Tab`/方向键在软键盘上可达。

### M3 — 连接管理与凭证

Flutter 表单做连接编辑，凭证进 iOS Keychain。
Rust 侧接口现成：`AuthPlan::from_profile(&profile, &vault)` + `CredentialVault`。

**验收**：连接增删改 + 密码进 Keychain，杀进程重启仍在；
主机密钥确认走真实 UI（`HostKeyPrompt{sha256, changed}`），
`changed=true` 时给显式警告而不是静默接受。

**⚠ 还债点**：M0 用的是 TOFU 自动接受，**必须在 M3 换成真实确认**。

### M4 — 产品化与合规

- 多标签 / 分屏（`PaneTree` / `SplitAxis` / `WorkspaceState` / `UiCommand::Split` 都已在 Rust 侧）
- **滚动回看的分级上界**：桌面契约是 `scrollback_lines` 上限 `1_000_000` 行，
  手机上会触发 jetsam 直接杀进程。必须另设平台分级上界。
- App 生命周期：进后台后 SSH 连接怎么处理（`SessionUiCommand::Reconnect` 已在协议里）
- 分发合规：App Review 2.5.2 对远程 shell 类应用有限制，定位与描述要提前想清楚。

---

## 6. 复用清单（铁律 3 的落地）

### 6.1 已找到、优先复用

| 需求 | 候选 | 状态 | 备注 |
|---|---|---|---|
| **终端渲染层**（M1 核心） | [`terminal_view`](https://pub.dev/packages/terminal_view) | pub.dev v0.1.x，MIT，fork 自 xterm.dart 4.0.0 | **首选 fork 对象。** 移动端优先，changelog 明确写了「把相邻同风格单元格合并成一个 paragraph、相邻背景合并成一个 rect、把不再变化的行录成 Picture 重放、光标在 render object 里闪烁」——**正好就是 §4.3 测出来的那套优化**，而且已经实现了。目标就是「mid-range Android 上把忙碌的 `tail -f` 压到 60fps」 |
| 同上（备选） | [`lollipopkit/xterm.dart`](https://github.com/lollipopkit/xterm.dart) | MIT，活跃 | 有 Unicode 16 宽度表、grapheme cluster、**字形图集（glyph atlas）**、`CharMetricsCache` |
| 同上（备选） | [`dart_xterm`](https://github.com/cdrury526/dart_xterm) | MIT，标称 production-grade | xterm.dart 的另一个维护分支 |
| **mDNS / Bonjour 发现**（暂不实施） | [`nsd`](https://pub.dev/packages/nsd) | pub.dev 5.0.1 | iOS 13+，多平台，使用平台原生 API。**已决定 M0-c 不做自动发现，此条备查** |
| 同上（备选） | [`bonsoir`](https://pub.dev/packages/bonsoir) | pub.dev，iOS 13+ | 基于 `NWBrowser`，API 更现代（`eventStream`） |
| 同上（不推荐） | `flutter_nsd` | 原包与 AGP 8 不兼容，有第三方 fork | 生态较乱，优先上面两个 |

**⚠️ 复用 `terminal_view` / `xterm.dart` 的边界（对应铁律 4）：**
它们的 `Terminal` 是**完整的 VT100/xterm 解析器 + 缓冲区**。
我们**只复用它的绘制层**（`TerminalView` / render object / 字形缓存 /
选择手柄 / IME 接入 / 主题 / 鼠标 / 超链接 / 搜索 / OSC 52），
把数据源换成来自 Rust 的 `RenderFrame`。
**绝不使用它的解析器与缓冲区**——那会造出第二份终端状态权威。

### 6.2 明确不用

| 东西 | 为什么不用 |
|---|---|
| `dartssh2`（纯 Dart SSH） | 会把 SSH 搬到 Dart，直接违反铁律 1 |
| `xterm.dart` 的 `Terminal` 状态机 | 第二份状态权威，违反铁律 4 |
| `flutter_rust_bridge` | 已定用 rinf（统一信号规约，且单向信号流能防止业务滑向 Dart） |
| Rust 侧自渲染成位图 | 见 §4.3 第 6 条 |

### 6.3 还没找的（做之前必须先找）

- iOS 侧「跳转到系统设置页」的现成实现（本地网络权限被拒后的引导）
- Flutter 侧的键位条 / 快捷键行（有大量现成实现，如终端类 App 的开源方案）
- 连接导入（`ssh_config` / 其他客户端迁移）——上游 `rshell-core::protocol::imports` 已有 Rust 侧逻辑

---

## 7. 环境前置（任意机器需满足什么）

> 本节只写**要求**。本机的版本号、工具绝对路径、真机清单等**一律不入库**。

| 项 | 要求 | 需要做的事 |
|---|---|---|
| Mac | Apple Silicon | 这类机器上 "My Mac (Designed for iPad)" 可用，**iPad 验证的第一条路不依赖模拟器** |
| Xcode | 16+，iOS SDK 与 Xcode 配套 | — |
| Rust iOS 目标 | `aarch64-apple-ios` / `aarch64-apple-ios-sim` | — |
| **Rust 工具链** | `cargo` 1.89+ / `rustup`，常装在 `~/.cargo/bin` 而**不在 PATH** | 用前 `export PATH="$HOME/.cargo/bin:$PATH"`（`scripts/setup.sh` 会提示） |
| **`rinf` CLI** | 8.x。子命令：`config` / `template` / `gen` / `wasm` / `server` | 起手直接用 `rinf template` 铺 Flutter 骨架、`rinf gen` 生成 Dart 侧类型，**不手写桥接样板** |
| 其他 CLI | `cargo-ndk`、`protoc-gen-prost` 可选 | 本项目用不上（不做 Android、不用 protobuf 走码流） |
| **iOS 模拟器 runtime** | 随 iOS platform 一起装 | 需要模拟器时再补：Xcode → Settings → Components，或 `xcodebuild -downloadPlatform iOS` |
| **Flutter / Dart** | 3.4x，常由 **fvm** 之类的版本管理器托管 | 用前自行 export PATH；`scripts/setup.sh` 会探测并提示 |
| 已注册真机 | iPhone 13 mini / 12 mini | **本机设备名不入库**；M1-b 才用得上，非阻塞 |
| **iPad** | **不需要真机**。验证走 "Designed for iPad" + iPad 模拟器（按需补装 runtime） | — |

---

## 8. 已定型的技术决定

| 决定 | 理由 |
|---|---|
| 桥用 **rinf**，不用 flutter_rust_bridge | 统一信号规约；单向信号流天然防止业务滑向 Dart。注意 rinf v8 已改用 serde + bincode（不再是 protobuf）；需要 protobuf 语义时用 `prost` 编码后塞进 `RustSignalBinary.binary` |
| 边界类型**沿用** `rshell-core::protocol` | 上游已有完整可序列化协议，重建等于在 Dart 写业务模型（违反铁律 1） |
| **不做本地 shell 面板** | iOS 不可能；Rust 侧零改动，Flutter 侧不暴露入口即可 |
| 帧格式：**结构化帧 + run 压缩 + 脏行增量** | §4.3 实测 |
| 度量权威归 **Flutter** | 字体度量只有一个权威，就是实际画字的那一方。`TerminalSize{cols,rows,pixel_width,pixel_height,dpi}` 由 Flutter 测完回传 |
| 状态权威归 **Rust** | alacritty 网格只在 Rust 侧 |
| M0 宿主用 **Swift 而不是 Flutter** | 把「SSH 通不通」与「Flutter 构建集成」两个未知量分开；`.a` 在两种宿主下通用，M1 换宿主零返工 |
| M0 主机密钥策略：**TOFU 自动接受并落盘** | M0 前提是「不接 UI」，走完整确认会自相矛盾。**明确的技术债，M3 还清** |
| release profile **不要设 `panic = "abort"`** | 上游 actor 靠 `catch_unwind` 把 panic 转成 `SessionEvent::Crashed`（有 `actor_panic_gtk_survival_macos` 测试守着），abort 会毁掉这条韧性设计 |
| iPad 优先，iPhone 作为子项 | iPad 有大屏 + 硬件键盘 + 指针支持，能把最难的 IME/软键盘问题推到 M1-b |
| **仓库形态：新建仓库 + vendor 上游源码** | 不 fork 上游（不做 PR 回上游的压力）、也不用 git 依赖（要能随手改内核）。上游只作为**基线**存在于 `rust/upstream/`，**不追求与上游保持同步**。改动处打 `[GuoSSHell 改动]` 标注，出处记在 `rust/upstream/PROVENANCE.md` |
| **连接方式：手工填写，不做自动发现** | 范围收窄，M0-c 只做「手填内网地址能连 + 权限处理 + 清晰失败提示」。mDNS/Bonjour 插件候选留在 §6.1 备查，不作为本期依赖 |
| **iPad 验证靠 "Designed for iPad" + 模拟器** | 不需要真实 iPad 设备。但**必须**是 iPad 版二进制跑在 iOS 运行时上，不能拿 macOS 目标冒充（见 §5 M1-a） |

---

## 9. 待定项

### 9.1 已定（原待定项，2026-09-14 拍板）

| 原问题 | 决定 | 落地位置 |
|---|---|---|
| 仓库形态：fork 改名，还是新建仓库？ | **新建仓库**，上游源码 **vendor** 进 `rust/upstream/`，不与上游保持同步 | §8、`rust/upstream/PROVENANCE.md` |
| 内网子项要不要做 mDNS 自动发现？ | **不做**。只做手填内网地址 + 权限处理 + 失败提示 | §5 M0-c |
| iPad 设备怎么准备？ | **不需要真机**：先 "My Mac (Designed for iPad)"，再 iPad 模拟器，iPad 通了才做 iPhone | §5 M1-a / M1-b、§7 |

### 9.2 仍然开着（不阻塞 M0/M1，先记着）

1. **iPad 上的字号与列数默认值**：`TerminalSize` 由 Flutter 量完回传（§8），
   但 120×40 只是基准值。真实 iPad 上的默认字号/行列数要等 M1 画出来才好定。
2. **M3 的凭证 UI 形态**：钥匙串存密码「每次连接都读」还是「读一次缓存在内存」，
   影响 Face ID / 自动填充的介入点。等 M3 再定。
3. **自动发现（mDNS）是否作为 M5**：本期明确不做，但如果 M0-c 的手填体验在局域网里
   太差，可以把它拉回来作为独立里程碑。候选插件已在 §6.1。

---

## 10. 陷阱清单

按踩坑概率排序。前三条是"不知道就会浪费一天"级别的。

1. **别用模拟器（或 "Designed for iPad"）验收 iOS 特有问题。** 它们跑在 macOS 用户态，
   `fork` / `openpty` 都在。拿它们测「本地 PTY 在 iOS 上不工作」会得出**错误结论**（会显示能跑）；
   反过来也会掩盖真正的沙箱问题。
   **但它们能验收「出站 TCP 在 iOS 沙箱里能不能连」**——这恰好是 M0b 要问的问题，
   因为这里唯一的平台差异就是 socket 与 App 沙箱。
2. **M0 的读循环必须有截止时间。** 真实远端 shell 既不 `Eof` 也不给 `ExitStatus`，
   会一直等下一条输入。只有 `exec` 模式（`ConnectionProfile::remote_command` 非空）
   才有退出码。「连上 → 读 → 等 Eof → 打印」这个最自然的写法在真机上永久挂住。
   （实测中我第一版就这么挂了。）
3. **内网连接的权限失败是静默超时，不是报错。** 见 §5 M0-c。
4. **`serde` 不开 `rc` feature 就序列化不了 `RenderFrame`。** 见 §2.2。
5. **`serde_json` 不能用于帧传输。** 700–850 KB/帧。见 §4.3。
6. **secret 类型不可序列化，协议层要手写转换。** 见 §2.2。
7. **不要在 release 里开 `panic = "abort"`。** 见 §8。
8. **锁屏/后台会挂起 SSH 连接。** 重连策略要做成显式产品行为，不能靠「不断开」蒙混。
9. **软键盘没有 `Ctrl`/`Esc`，`Cmd` 组合会被系统截获。** 能用的修饰键只有 `Ctrl`/`Alt`（和长按）。
10. **`rshell-platform::default_local_shell()` 在 iOS 上没有意义**（`SHELL` 环境变量不存在，
    兜底是 `/bin/sh`，而 iOS 上跑不了）。既然不做本地面板，调用点直接不暴露。
11. **Xcode 里 "My Mac" 有两个目标，选错等于白跑。** 名字都带 My Mac，但
    `My Mac (Designed for iPad)` 跑的是 **iPad 版二进制**，`My Mac` 跑的是 macOS 二进制。
    选后者再宣布「iPad 通了」是自欺——它连 UIKit 都没走。

---

## 11. 可复现命令

```bash
# 环境准备（幂等；上游已 vendor，不需要克隆）
./scripts/setup.sh

# 以下命令都在 rust/ 里跑
cd rust

# M0 端到端，不需要任何外部服务器或凭证
cargo run --example m0_loopback

# 打真实服务器
cargo run --example m0 -- <host> <port> <user> <password>

# 性能基准（本文档 §4 的全部数字）
cargo run --release --example bench_frame

# iOS 产物（静态库，要链进 App 才能跑）
cargo build --release --lib --target aarch64-apple-ios
```

M0b 的 Xcode 工程建法见 `ios-host/README.md`。

---

## 12. 当前进度

- [x] 可行性调研（alacritty 四层判定、rinf 事实纠正）
- [x] Rust 后端 iOS 编译 + 链接 + 符号审计
- [x] M0a 端到端（环回，无外部依赖）
- [x] 帧传输性能基准（M0 的性能问题已闭环）
- [x] 复用候选调研（渲染层 `terminal_view`；mDNS 备查）
- [x] 上游 vendor 进 `rust/upstream/`，iOS keyring 补丁固化
- [x] 全部产物收敛到 `GuoSSHell/`（Flutter 目录已清空）
- [x] **仓库初始化**（`git init -b main`，227 个文件已暂存，首次提交信息在 `.git/FIRST_COMMIT_MSG.txt`）
      ⚠ 本机 git 开了 `commit.gpgsign=true`，但 `~/.gnupg` 在当前进程里**不可读**
      （`Operation not permitted`，不是 gpg 没装）——**首次提交需要在你自己终端里执行**：
      `git commit -F .git/FIRST_COMMIT_MSG.txt`
- [ ] **M0b：把 `librshell_m0.a` 链进 iOS App 跑起来**
      阻塞在：需要一个建好的 Xcode App 目标（步骤见 `ios-host/README.md`）+ 一台可连的 SSH 服务器。
      快路：目标选 **My Mac (Designed for iPad)**，不需要先下载模拟器 runtime。
- [ ] M0-c 内网连接（手填地址 + 权限处理）
- [ ] M1 一帧终端画面（含 M1-a iPad 验证，先于 M1-b iPhone）
- [ ] M2 / M3 / M4
