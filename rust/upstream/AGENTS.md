# AGENTS.md — rsHell

Cross-platform SSH terminal manager and GUI terminal emulator. Rust + GTK4 (via relm4).

## Architecture

```
src/
├── main.rs          # Entry point → RelmApp → RshellApp. Suppresses GIO/DBUS warnings via env vars.
├── lib.rs           # Module declarations only (app, connection, ssh, terminal, theme)
├── app.rs           # Main UI component (RshellApp, relm4 SimpleComponent). AppMsg enum, ConnectionDraft,
│                    #   AppWidgets. Imperative GTK widget tree in init(). Sidebar, editor dialog,
│                    #   tab bar, split panes (Single/HSplit/VSplit/TopBottom3/Grid), status bar.
├── terminal.rs      # PTY session management. SessionPhase, SessionSnapshot, TerminalSessionHandle (thread-safe).
│                    #   launch_local_session / launch_session, reader + command threads, wezterm event pump.
├── connection.rs    # Connection profile storage. ConnectionBackend (SystemOpenSsh/WezTermSsh), ConnectionProfile,
│                    #   ConnectionFolder, ConnectionStore (CRUD + normalize/sort), ConnectionRepository (JSON persistence).
├── ssh.rs           # SSH command builders. build_system_command (CommandBuilder), build_wezterm_config (ConfigMap).
└── theme.rs         # Global CSS loader (apply_global_css). Loads resources/style.css via GResource.

examples/            # GTK CSS test + minimal relm4 counter. Not production code.
resources/
├── style.css        # Fluent UI Dark theme (colors, spacing, widget styling)
└── rshell.gresource.xml  # GResource manifest bundling style.css
```

### Key flows

- **Local shell**: `app.rs` sends `AppMsg::NewLocalTab` → `terminal.rs::launch_local_session()` → spawns PTY via `portable-pty`, starts reader/command threads
- **SSH session**: `app.rs` sends `AppMsg::ConnectToServer` → `terminal.rs::launch_session()` → either `ssh.rs::build_system_command()` (System OpenSSH via PTY) or `ssh.rs::build_wezterm_config()` (WezTerm native SSH)
- **UI refresh**: 250ms glib timer → `AppMsg::RefreshSessions` → reads `SessionSnapshot` from each `TerminalSessionHandle` → redraws terminal content via `gtk4::DrawingArea`
- **Persistence**: `ConnectionRepository` reads/writes `{config_local_dir}/rshell/connections.json`

### Threading & lock ordering

Per-session: reader thread (PTY → terminal state) + command thread (user input → PTY).
Lock acquisition order (deadlock prevention): **terminal → master → snapshot**.
`TerminalSessionHandle` wraps `Arc<Mutex<...>>` for thread-safe access from GTK main loop.

## Conventions

- **Rust edition**: 2024
- **Formatting**: Default rustfmt (no rustfmt.toml)
- **Linting**: Default clippy, CI enforces `cargo clippy -- -D warnings`
- **Tests**: Inline `#[cfg(test)] mod tests` in connection.rs and ssh.rs. No mocking library — tests create structs directly. IO tests use `tempfile`.
- **Error handling**: `anyhow::Result` throughout. No custom error types.
- **App ID**: `io.github.hugefiver.rshell`

## Build & CI

- **CI** (`.github/workflows/ci.yml`): push to master + PRs. Linux/macOS/Windows.
  - `cargo check`
  - `cargo test --lib`
  - `cargo clippy -- -D warnings`
- **Release** (`.github/workflows/release.yml`): tagged → stable GitHub release, master push → nightly pre-release.
  - Targets: linux-x86_64, macos-arm64, windows-x86_64
  - Windows bundles GTK DLLs + glib schemas + gdk-pixbuf loaders via gvsbuild
- **Windows build prereqs**: gvsbuild (GTK4), vcpkg (OpenSSL, libssh2)
- **Local build (Windows)**:
  ```powershell
  # 1. Install gvsbuild (requires Visual Studio 2022 + Python 3.x)
  pip install gvsbuild
  gvsbuild build gtk4 librsvg

  # 2. Set environment for cargo
  $gtkRoot = "C:\gtk-build\gtk\x64\release"
  $env:PKG_CONFIG_PATH = "$gtkRoot\lib\pkgconfig"
  $env:LIB = "$gtkRoot\lib"
  $env:PATH = "$gtkRoot\bin;$env:PATH"

  # 3. Build & run
  cargo build
  cargo run
  ```
  gvsbuild output goes to `C:\gtk-build\gtk\x64\release\`. The `bin\`, `lib\`, `lib\pkgconfig\` dirs must be on PATH/LIB/PKG_CONFIG_PATH respectively.
- **Verify changes**: `cargo check; cargo test --lib; cargo clippy -- -D warnings`

## Gotchas

- **Lock ordering**: Always acquire terminal → master → snapshot. Violating this causes deadlocks. See `terminal.rs` line 87.
- **Passwords stored plaintext** in JSON config (connection.rs `ConnectionProfile.password`). Known TODO.
- **GIO suppression**: `main.rs` uses `unsafe { std::env::set_var(...) }` to suppress DBUS/GIO warnings. Required for clean startup.
- **Pinned dependencies**: `wezterm-term` pinned to git rev `05343b3`. `portable-pty` and `smol` versions pinned by `wezterm-ssh 0.4.0`. `gtk4` version locked by `relm4 0.10.x`.
- **`RSHELL_SHELL`** env var overrides the default local shell path.
- **`app.rs` is monolithic** (~2300 lines): all UI state, message handling, widget construction, and layout logic in one file.
- **No integration tests**: `cargo test --lib` only. Examples are for manual GTK testing.
