//! M0a（加强版）：**不依赖任何外部服务器**地跑通完整 M0 路径。
//!
//! 在同一个进程里起一个最小 russh 服务端（只认 `probe/probe`，接受任意主机密钥），
//! 然后用 `rshell_m0::blocking_smoke` 走一遍和真机完全相同的代码路径：
//! TCP → 版本交换 → 密钥交换 → **主机密钥 TOFU 落盘** → 密码认证 →
//! `request_pty(xterm-256color, 80x24)` → `request_shell` → 读字节 → `window_change`。
//!
//! 它验证的正是「写死地址和密码验证 SSH 本身」这句话里最容易写错的部分：
//! 请求序列、交互自动应答、以及 `TransportEvent` 的产出。
//! Rust 侧逻辑一旦在这里是绿的，M0b（真机）剩下的未知量就只有 iOS 沙箱。
//!
//! ```text
//! cargo run --example m0_loopback
//! ```

use std::sync::Arc;
use std::time::Duration;

use russh::server::{Auth, ChannelOpenHandle, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId};

struct LoopbackServer;

impl Server for LoopbackServer {
    type Handler = LoopbackHandler;

    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> LoopbackHandler {
        LoopbackHandler
    }
}

struct LoopbackHandler;

impl Handler for LoopbackHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == "probe" && password == "probe" {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    /// 把客户端实际发来的 pty-req 参数回显出去——这是 M0 要验证的核心事实。
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel);
        session.data(
            channel,
            format!(
                "PTY-ACK term={term} cols={col_width} rows={row_height} px={pix_width}x{pix_height}\r\n"
            )
            .into_bytes(),
        )?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel);
        session.data(channel, b"M0-LOOPBACK-SHELL-OK\r\n$ ".to_vec())?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, data.to_vec())?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(
            channel,
            format!("RESIZE-ACK cols={col_width} rows={row_height}\r\n").into_bytes(),
        )?;
        Ok(())
    }
}

/// 一次性生成环回测试用的主机密钥。
///
/// 刻意走 `ssh-keygen` 而不是 `PrivateKey::random`：后者要求调用方提供 `ssh-key`
/// 期望的那一个 `rand_core` 版本（0.10），否则 `CryptoRng` trait 对不上。
/// M0 不该在密钥生成上花时间。
fn loopback_host_key() -> russh::keys::PrivateKey {
    let path = std::env::temp_dir().join("rshell-m0-loopback-hostkey");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("pub"));

    let status = std::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-f"])
        .arg(&path)
        .status()
        .expect("ssh-keygen must be available to run the loopback test");
    assert!(status.success(), "ssh-keygen failed");

    russh::keys::load_secret_key(&path, None).expect("load generated host key")
}

fn main() {
    let config = Arc::new(russh::server::Config {
        keys: vec![loopback_host_key()],
        inactivity_timeout: None,
        auth_rejection_time: Duration::from_millis(10),
        ..Default::default()
    });

    // 服务端跑在自己的运行时/线程里，主线程用 blocking_smoke（它自建运行时）。
    let (port_tx, port_rx) = std::sync::mpsc::channel::<u16>();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("server runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            port_tx
                .send(listener.local_addr().expect("addr").port())
                .expect("send port");
            loop {
                let Ok((socket, peer)) = listener.accept().await else {
                    break;
                };
                let config = Arc::clone(&config);
                tokio::spawn(async move {
                    let handler = LoopbackServer.new_client(Some(peer));
                    if let Err(error) = russh::server::run_stream(config, socket, handler).await {
                        eprintln!("[server] session ended: {error}");
                    }
                });
            }
        });
    });

    let port = port_rx.recv().expect("server port");
    let known_hosts = std::env::temp_dir().join("rshell-m0-loopback-known_hosts");
    // 每次从干净状态开始，确保真的走到「未知主机密钥 → 交互 → TOFU 落盘」。
    let _ = std::fs::remove_file(&known_hosts);

    println!("loopback server on 127.0.0.1:{port}");
    println!("known_hosts = {}", known_hosts.display());

    let outcome = rshell_m0::blocking_smoke(
        "127.0.0.1",
        port,
        "probe",
        "probe",
        known_hosts.to_str().expect("utf8 path"),
    );

    match &outcome {
        Ok(text) => println!("\n===== M0 结果 =====\n{text}"),
        Err(error) => {
            eprintln!("\n===== M0 失败 =====\n{error}");
            std::process::exit(1);
        }
    }

    let landed = outcome
        .as_ref()
        .map(|text| {
            text.contains("PTY-ACK term=xterm-256color")
                && text.contains("cols=80 rows=24")
                && text.contains("M0-LOOPBACK-SHELL-OK")
                && text.contains("echo M0-ECHO")
        })
        .unwrap_or(false);

    let persisted = known_hosts.is_file();
    println!(
        "\nPTY/shell 请求序列 + 写路径 : {}",
        if landed { "YES" } else { "NO" }
    );
    println!(
        "主机密钥 TOFU 已落盘        : {} ({})",
        if persisted { "YES" } else { "NO" },
        known_hosts.display()
    );

    if persisted {
        let _ = std::fs::remove_file(&known_hosts);
    }
    std::process::exit(if landed && persisted { 0 } else { 1 });
}
