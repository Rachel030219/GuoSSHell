//! M2 诊断：对真实服务器做两层探测，区分「russh ↔ 服务器协议问题」和
//! 「verifier/交互层问题」。不需要密码——只走握手 + 主机密钥确认。
//!
//! ```text
//! cargo run --example hostkey_probe -- <host> <port>
//! ```

use rshell_core::{
    HostKeyDecision, InteractionRequest, InteractionResponse, ResolvedTerminalProfile,
    TerminalOverrides, TerminalSettingsV1, TerminalSize, TransportKind,
};
use rshell_session::{
    AuthPlan, DefaultTerminalEngine, KnownHostsVerifier, NativeSshTransport, SessionTransport,
    TerminalEngine, TransportRequest, interaction_channel,
};

/// 第一层：裸 russh 客户端握手（check_server_key 一律接受）。
/// 走到这一步还失败 = russh 0.62.5 与该服务器的协议层问题。
async fn raw_russh_probe(host: &str, port: u16) -> Result<String, String> {
    use russh::client;

    struct AcceptAll;

    impl client::Handler for AcceptAll {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            server_public_key: &russh::keys::PublicKey,
        ) -> Result<bool, Self::Error> {
            println!(
                "[raw] 服务器主机密钥：{} {}",
                server_public_key.algorithm(),
                server_public_key.fingerprint(russh::keys::HashAlg::Sha256)
            );
            Ok(true)
        }
    }

    let config = std::sync::Arc::new(client::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(10)),
        ..Default::default()
    });

    let address = format!("{host}:{port}");
    let stream = tokio::net::TcpStream::connect(&address)
        .await
        .map_err(|error| format!("tcp connect FAILED: {error:?}"))?;
    println!("[raw] TCP 已连接 {address}");
    let handle = client::connect_stream(config, stream, AcceptAll)
        .await
        .map_err(|error| format!("raw connect FAILED: {error:?}"))?;

    // 握手 + 主机密钥都过了；不认证，直接断开。
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "probe done", "en")
        .await;
    Ok("raw handshake + host key OK".to_owned())
}

/// 第二层：完整 transport（含 KnownHostsVerifier TOFU + interaction 轮），
/// 用假密码——预期失败在 Authentication；若失败在 Platform/其他，说明
/// verifier 层对这台服务器有特异性问题。
async fn transport_probe(host: &str, port: u16) -> Result<String, String> {
    let mut profile = rshell_core::ConnectionProfile::new("probe", host);
    profile.host = host.to_owned();
    profile.port = port;
    profile.username = "probe-unknown-user".to_owned();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = rshell_core::AuthenticationKind::Password;

    let auth = AuthPlan::from_secret(&profile, Some(secrecy::SecretString::from("wrong-password")))
        .map_err(|error| format!("auth plan: {error:?}"))?;

    let known_hosts = std::env::temp_dir().join("guosh-hostkey-probe-known_hosts");
    let _ = std::fs::remove_file(&known_hosts);
    let verifier = KnownHostsVerifier::new(known_hosts.display().to_string().as_str());
    let (broker, mut interactions) = interaction_channel();

    let responder = {
        let broker = broker.clone();
        tokio::spawn(async move {
            while let Some((id, prompt)) = interactions.recv().await {
                println!("[transport] 交互请求：{:?}", std::mem::discriminant(&prompt));
                let _ = broker.respond(
                    id,
                    match prompt {
                        InteractionRequest::HostKey(_) => {
                            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
                        }
                        other => {
                            println!("[transport] 意外交互类型：{other:?}");
                            InteractionResponse::Cancel
                        }
                    },
                );
            }
        })
    };

    let term_profile: ResolvedTerminalProfile =
        TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    let mut engine = DefaultTerminalEngine::new(
        &term_profile,
        TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        },
    )
    .map_err(|error| format!("engine: {error:?}"))?;

    let mut transport = NativeSshTransport::new(profile, auth, verifier)
        .map_err(|error| format!("transport new: {error:?}"))?;

    let outcome = transport
        .connect(
            &TransportRequest::new(TerminalSize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 96,
            }),
            broker,
        )
        .await;

    responder.abort();

    match outcome {
        Ok(()) => {
            // 认证应该失败（假密码）；失败类别若是 Authentication = verifier 层全部通过。
            let _ = engine;
            let _ = transport.shutdown().await;
            Err("transport connect OK（假密码竟通过？检查服务器）".to_owned())
        }
        Err(error) => {
            let _ = transport.shutdown().await;
            Err(format!("transport connect: {error:?}"))
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: hostkey_probe <host> <port>");
        std::process::exit(2);
    }
    let host = args[0].clone();
    let port: u16 = args[1].parse().expect("port");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");

    runtime.block_on(async move {
        println!("══ 1/2 裸 russh 握手 ══");
        match raw_russh_probe(&host, port).await {
            Ok(message) => println!("✅ {message}"),
            Err(message) => {
                println!("❌ {message}");
                return;
            }
        }
        println!("══ 2/2 完整 transport（TOFU + 假密码）══");
        match transport_probe(&host, port).await {
            Ok(message) => println!("✅ {message}"),
            Err(message) => println!("⚠️  {message}"),
        }
    });
}
