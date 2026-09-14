# rsHell

rsHell 是使用 Rust、GTK4 与 Relm4 构建的跨平台 SSH 终端管理器。P0 提供 Windows Terminal Dark / Fluent 风格的原生桌面界面、本地终端、两种 SSH transport、严格主机密钥确认、系统凭据库、标签页与分屏，以及旧配置导入。

## P0 功能

- 本地 shell 与完整终端模拟：ANSI 样式、Unicode、滚动缓冲、搜索、选择、复制粘贴和尺寸调整。
- 唯一终端后端是 Alacritty 0.26（`alacritty-terminal@0.26.0`）。其 100 MiB×5、120×40 p95、1000 行滚动缓冲 SHA-256 的记录契约为 **GO**；`scripts/qa/terminal-engine-gate.ps1` 会将后端、记录和测量不一致视为失败。
- 连接目录：创建、编辑、复制、移动、删除和搜索连接；支持嵌套分组与 tags。
- 工作区：多标签页、水平/垂直分屏、关闭、重连和明确的会话状态/错误页面。
- 两种 SSH transport：

  | 能力 | System OpenSSH | Native SSH |
  |---|---:|---:|
  | SSH agent | 是 | 否 |
  | 私钥 | 是 | 是 |
  | 密码 | 交由系统 OpenSSH | 是 |
  | Keyboard-interactive | 交由系统 OpenSSH | 是 |
  | 主机密钥确认 | OpenSSH strict/ask | 应用内 strict confirmation |

- Native SSH 的密码、加密私钥 passphrase 与 keyboard-interactive 回答由应用安全交互界面提交；秘密不会写入 SQLite。
- 首次未知主机密钥必须显示算法与 SHA-256 fingerprint 并由用户确认；已变更密钥 fail closed。Native SSH 使用应用私有 `known_hosts`，不会修改用户的 OpenSSH 文件。
- 旧 JSON 与 OpenSSH config 先进行静态 preview，再以选中项原子导入；不会执行 `ProxyCommand` 或其他配置命令。
- Fluent 暗色界面使用编译进二进制的产品图标，不依赖宿主 GTK icon theme。

## 数据与安全

- 连接、分组、tags、终端 profiles 和设置保存在平台应用 state 目录中的 `rshell.sqlite3`。
- 密码和私钥 passphrase 只保存在系统凭据库：Linux Secret Service、macOS Keychain 或 Windows Credential Manager。
- Native SSH 的应用私有主机密钥文件位于平台 config 目录中的 `known_hosts`。
- SQLite 写入使用事务；跨 SQLite/系统凭据库的更新通过持久化 credential journal 恢复。启动时会先 reconcile 未完成操作。
- QA 会扫描 SQLite、WAL/SHM、报告和日志，拒绝任何 scenario secret 泄漏。

平台目录由 `directories::ProjectDirs` 以应用标识 `io.github.hugefiver.rshell` 解析，具体位置遵循各操作系统的标准应用目录。

## 安装与运行

从 [Releases](https://github.com/hugefiver/rshell/releases) 下载对应包：

- `linux-x86_64`
- `macos-arm64`
- `windows-x86_64`

Windows release 包含运行所需的 GTK runtime。Linux 和 macOS 需要系统 GTK4。

启动：

```text
rshell
```

可通过 `RSHELL_SHELL` 指定本地 shell 的完整程序路径。

## 从源码构建

通用依赖：

- Rust stable（edition 2024）
- PowerShell 7（运行 QA 脚本）
- System OpenSSH：`ssh`、`ssh-keygen` 和 `ssh-agent`
- GTK4 与 `pkg-config`

平台说明：

- Linux：安装 `libgtk-4-dev`、`pkg-config`；真实凭据 QA 还需要 D-Bus、Secret Service（例如 `gnome-keyring`）和 `secret-tool`。
- macOS：`brew install gtk4 pkg-config`；凭据使用临时测试 Keychain，产品使用用户 Keychain。
- Windows：使用 [gvsbuild](https://github.com/wingtk/gvsbuild) 构建 GTK4，并将 GTK 的 `bin`、`lib` 和 `lib/pkgconfig` 分别加入 `PATH`、`LIB` 和 `PKG_CONFIG_PATH`。不需要 OpenSSL、vcpkg 或旧 C SSH 库。

Windows 示例：

```powershell
$gtkRoot = "C:\gtk-build\gtk\x64\release"
$env:PKG_CONFIG_PATH = "$gtkRoot\lib\pkgconfig"
$env:LIB = "$gtkRoot\lib"
$env:PATH = "$gtkRoot\bin;$env:PATH"
cargo build --release --workspace --locked
```

Linux/macOS 在 GTK4 可由 `pkg-config` 发现后执行：

```text
cargo build --release --workspace --locked
```

## 开发与验证

完整自动质量门：

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
pwsh -NoProfile -File scripts/qa/terminal-engine-gate.ps1
```

真实 P0 surface（会使用 GTK、PTY、System OpenSSH、真实系统凭据库与临时 QA 资源）：

```powershell
pwsh -NoProfile -File scripts/qa/p0-smoke.ps1 -Mode All
pwsh -NoProfile -File scripts/qa/assert-no-secrets.ps1 -ArtifactRoot artifacts/p0-smoke
```

Smoke 报告、JUnit 与截图写入 `artifacts/p0-smoke/`；报告中的 artifact 路径仅为 leaf 文件名，不会写入工作区或用户绝对路径。`direct_session_child_count == 0` 仅证明注册表中的直接 PID 已停止，不是进程树证明；Windows 的 Job Object `immediate_descendant_is_contained_before_first_user_marker` 真实即时后代测试才证明树在首个用户标记前已被包含并在 teardown 后终止。脚本对 timeout、失败的服务准备、未清理的进程/凭据/journal 和缺失证据 fail closed。

## 架构

```text
src/                    composition root、bootstrap、cleanup 与 P0 smoke
crates/rshell-core/     连接领域、workspace、协议与 application ports
crates/rshell-platform/ 平台目录、权限、进程、shell 与 clipboard
crates/rshell-session/  终端引擎、session actor、PTY 与 SSH transports
crates/rshell-storage/  SQLite、系统凭据库 journal 与 importers
crates/rshell-ui/       GTK4/Relm4 Fluent UI 与 TerminalView
resources/              编译期 CSS 与产品 SVG 图标
scripts/qa/             P0 smoke、secret、workflow 与 package contracts
```

CI 在 Linux x86_64、macOS arm64 与 Windows x86_64 上依次运行 workspace gates、记录的 Alacritty 0.26 terminal-engine gate、真实 SSH/vault/GTK smoke。Release workflow 在每个构建矩阵路径先运行同一 gate，再构建并验证三个平台包的架构、runtime、启动报告与禁止的旧依赖标记。

## 许可证

[MIT](LICENSE)
