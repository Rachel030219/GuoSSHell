# rust/ —— 本项目的 Rust 代码

当前内容是 **M0 的可执行验证**，不是产品代码。它把「SSH 本身在 iOS 上能不能用」
这个问题在真机之前回答掉。完整规划见 [`../PLAN.md`](../PLAN.md)。

依赖的是 vendored 在 `upstream/` 里的 rsHell 内核（见 [`upstream/PROVENANCE.md`](upstream/PROVENANCE.md)）。

## 文件

| 文件 | 作用 |
|---|---|
| `src/lib.rs` | M0 全部逻辑：`smoke()`（异步）、`blocking_smoke()`、C ABI `rshell_m0_smoke()`；外加 `engine_smoke()` / `frame_is_send()` 作为 M1 的前置验证 |
| `examples/m0_loopback.rs` | **同进程起一个 russh 服务端**，把整条路径跑通。不需要任何外部服务器或凭证 |
| `examples/m0.rs` | 打真实服务器：`cargo run --example m0 -- <host> <port> <user> <password> [known_hosts]` |
| `examples/bench_frame.rs` | 帧传输性能基准，产出 `PLAN.md` §4 的全部数字 |

## 快速开始

```bash
../scripts/setup.sh
cargo run --example m0_loopback        # 应当输出 M0 结果 + 两个 YES
cargo run --release --example bench_frame
cargo build --release --lib --target aarch64-apple-ios
```

`m0_loopback` 的期望输出：

```
===== M0 结果 =====
outcome=read-deadline(10s) reads=3 bytes=89
--- transcript ---
PTY-ACK term=xterm-256color cols=80 rows=24 px=0x0
M0-LOOPBACK-SHELL-OK
$ echo M0-ECHO

PTY/shell 请求序列 + 写路径 : YES
主机密钥 TOFU 已落盘        : YES (...)
```

真机 / iPad 验证（M0b）见 [`../ios-host/README.md`](../ios-host/README.md)。

## 已实测结论

- `alacritty_terminal 0.26.0` 全链为 `aarch64-apple-ios` 编译通过，**零改动**。
- 整个内核为 iOS 编译只卡一处：`apple-native-keyring-store` 的 `protected` feature
  （已作为 vendored 改动固化在 `upstream/crates/rshell-storage/Cargo.toml`）。
- 为 iOS 完整链接出的 dylib **不含** `_openpty` / `_forkpty` / `_login_tty` / `_fork` /
  `_posix_spawn` / `_execve`——未解析符号只有 libSystem 的 POSIX socket/线程 + CommonCrypto。
- release + strip 后 dylib ≈ **3.7 MB**（release `.a` 43.8 MB，strip 前 debug dylib 9.1 MB）。

## 三个平台陷阱

1. **别用模拟器（或 "Designed for iPad"）验收 iOS 特有问题。** 它们跑在 macOS 用户态，
   `fork`/`openpty` 都在，拿它测「本地 PTY 在 iOS 上不工作」会得出错误结论。
   但 M0b 问的是「出站 TCP 在 App 沙箱里能不能连」，这个它们能验。
2. **读循环必须有截止时间。** 真实远端 shell 永远不会 EOF、也没有 `ExitStatus`；
   「等 Eof 就退出」的循环会永久挂住。`READ_DEADLINE`（10 s）就是为此存在。
3. **内网目标机会触发 iOS 本地网络隐私。** 需要 `NSLocalNetworkUsageDescription`，
   未授权时 `connect()` 表现为**静默超时**而不是报错。公网 VPS 无此问题。

## 说明

`src/lib.rs` 的依赖刻意不含 GTK 层（`rshell-ui`）与 keyring 的 vault 路径：
M0 阶段凭证写死在 Rust 侧，`AuthPlan::from_secret()` 不读 vault。
静态库产物 `librshell_m0.a` 直接链进 Xcode target 或 Flutter plugin 即可。
