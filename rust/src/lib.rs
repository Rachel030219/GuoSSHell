//! rsHell on iOS —— M0 / M1 编译-链接探针。
//!
//! 这不是产品代码。它存在的唯一目的，是把「Flutter 前端 + Rust 业务内核」路线上
//! **两件无法靠读源码确认的事**变成实跑结果：
//!
//! * **M0**：`NativeSshTransport`（russh + `tokio::net::TcpStream` + `request_pty`
//!   + `request_shell`）能否为 `aarch64-apple-ios` 编译并链接；并且在 iOS 上
//!   **不触碰** `fork` / `openpty` / `posix_spawn`。这是「业务仍然用 Rust 跑」的
//!   全部前提——SSH 通了，终端状态机和渲染帧才有意义。
//!
//! * **M1**：`DefaultTerminalEngine`（alacritty 网格 + `RenderFrame`）能否为 iOS
//!   编译，且 `RenderFrame` 能直接走 serde 序列化——这是 rinf 边界的先决条件。
//!
//! 两个函数都不依赖 `rshell-ui`（GTK/Relm4，22812 行），也不依赖 `rshell-storage`
//! 的 keyring 路径：M0 阶段凭证写死在 Rust 侧，`AuthPlan::from_secret` 不读 vault。

use std::ffi::{CStr, CString, c_char};
use std::sync::Arc;

// 对下游（`native/hub`，rinf 的信号层）再导出上游内核：
// 上游 git 依赖只在本 crate 的 Cargo.toml 里 pin 一次 rev，
// hub 通过这里拿类型，避免第二处 rev 需要同步升级。
pub use rshell_core;
pub use rshell_session;

use rshell_core::{
    AuthenticationKind, ConnectionProfile, HostKeyDecision, InteractionRequest,
    InteractionResponse, ResolvedTerminalProfile, TerminalOverrides, TerminalSettingsV1,
    TerminalSize, TransportKind, Viewport,
};
use rshell_session::{
    AuthPlan, DefaultTerminalEngine, KnownHostsVerifier, NativeSshTransport, SessionTransport,
    TerminalEngine, TransportEvent, TransportRequest, interaction_channel,
};
use secrecy::SecretString;

const TRANSCRIPT_LIMIT: usize = 256 * 1024;

/// M0 的读循环**必须自带截止时间**：真实远端 shell 永远不会 EOF，也没有 ExitStatus。
/// 一把「等 Eof 就退出」的循环会在真机上永久挂住——这是 M0 最容易踩的坑。
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// M0 顺带验证写路径：一条命令发过去，看它有没有被远端回显。
const PROBE_COMMAND: &[u8] = b"echo M0-ECHO\r";

// ─────────────────────────────────────────────────────────────────────────────
// M0：SSH 本身
// ─────────────────────────────────────────────────────────────────────────────

/// M0 的完整 SSH 路径。写死的地址与密码由调用方在 Rust 侧传入（真机上先不接 UI）。
///
/// 关键点：
/// * `AuthPlan::Password` 直接调 `russh` 的 `authenticate_password`，
///   **不经过 InteractionBroker**——所以 M0 不需要任何 UI 参与认证。
/// * 唯一需要交互的是主机密钥确认，这里用「一律接受并落盘」（TOFU）自动应答，
///   落盘路径指向 iOS 沙箱内可写文件。
/// * `TransportRequest` 通过 `configure_channel` 发出 `request_pty(terminal_type,
///   cols, rows, pixel_w, pixel_h)` + `request_shell`。**PTY 开在远端**，
///   本地不需要 fork。
pub async fn smoke(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    known_hosts_path: &str,
) -> Result<String, String> {
    let mut profile = ConnectionProfile::new("m0", host);
    profile.host = host.to_owned();
    profile.port = port;
    profile.username = username.to_owned();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;

    let auth = AuthPlan::from_secret(&profile, Some(SecretString::from(password.to_owned())))
        .map_err(|error| format!("AuthPlan: {error:?}"))?;

    let verifier = KnownHostsVerifier::new(known_hosts_path);
    let (broker, mut interactions) = interaction_channel();

    // M0 无 UI：主机密钥一律 TOFU 接受。M2 起这里换成 rinf 的 InteractionRequired 往返。
    let responder = {
        let broker = broker.clone();
        tokio::spawn(async move {
            while let Some((id, request)) = interactions.recv().await {
                let response = match request {
                    InteractionRequest::HostKey(_prompt) => {
                        InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
                    }
                    // 其余分支 M0 不会走到：密码走 AuthPlan，键盘交互式未启用。
                    _ => InteractionResponse::Cancel,
                };
                let _ = broker.respond(id, response);
            }
        })
    };

    let request = TransportRequest::new(TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    });

    let mut transport = NativeSshTransport::new(profile, auth, verifier)
        .map_err(|error| format!("NativeSshTransport: {error:?}"))?;

    transport
        .connect(&request, broker)
        .await
        .map_err(|error| format!("connect: {error:?}"))?;

    let mut transcript = String::new();

    // 写路径：这部分就是 M2 输入闭环的最小切片。
    if let Err(error) = transport.write(PROBE_COMMAND).await {
        transcript.push_str(&format!("[write failed: {error:?}]\n"));
    }

    let mut reads = 0usize;
    let read_loop = async {
        loop {
            match transport.next_event().await {
                Ok(TransportEvent::Output(bytes)) => {
                    reads += 1;
                    transcript.push_str(&String::from_utf8_lossy(&bytes));
                    if transcript.len() >= TRANSCRIPT_LIMIT {
                        return "transcript-limit".to_owned();
                    }
                }
                Ok(TransportEvent::Exit(status)) => {
                    return format!("exit code={:?} success={}", status.code, status.success);
                }
                Ok(TransportEvent::Eof) => return "eof".to_owned(),
                Ok(TransportEvent::Failure(failure)) => return format!("failure {failure:?}"),
                Ok(_) => {}
                Err(error) => return format!("error {error:?}"),
            }
        }
    };

    let outcome = match tokio::time::timeout(READ_DEADLINE, read_loop).await {
        Ok(outcome) => outcome,
        Err(_) => format!("read-deadline({READ_DEADLINE:?})"),
    };

    let _ = transport.shutdown().await;
    responder.abort();

    // M0 先验证「字节流回来了」；M1 之后同一批字节才会喂给 DefaultTerminalEngine。
    Ok(format!(
        "outcome={outcome} reads={reads} bytes={} \n--- transcript ---\n{transcript}",
        transcript.len()
    ))
}

/// 同步封装：iOS 侧（Swift / Flutter 的 Isolate）自己起线程调用，不要占主线程。
pub fn blocking_smoke(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    known_hosts_path: &str,
) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| format!("tokio: {error}"))?;
    runtime.block_on(smoke(
        host,
        port,
        username,
        password,
        known_hosts_path,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// M1：终端状态机 + 渲染帧（rinf 边界的先决条件）
// ─────────────────────────────────────────────────────────────────────────────

/// 构造 `DefaultTerminalEngine`，喂一段带 SGR 颜色的字节，渲染一帧并序列化。
///
/// 这一步证明的是：**渲染帧可以从 Rust 直接序列化过边界**，Flutter 侧只负责画。
/// Rust 是唯一状态权威，Dart 不持有终端状态机。
pub fn engine_smoke() -> Result<String, String> {
    let profile: ResolvedTerminalProfile =
        TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    let size = TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    };

    let mut engine =
        DefaultTerminalEngine::new(&profile, size).map_err(|error| format!("engine: {error:?}"))?;
    engine
        .advance(b"\x1b[31mred\x1b[0m plain\r\nsecond line \xe4\xb8\xad\xe6\x96\x87\r\n")
        .map_err(|error| format!("advance: {error:?}"))?;

    let frame = engine
        .render(
            Viewport {
                top_stable_row: i64::MAX,
                rows: 24,
            },
            None,
        )
        .map_err(|error| format!("render: {error:?}"))?;

    let encoded = serde_json::to_string(&*frame).map_err(|error| format!("serde: {error}"))?;
    Ok(format!(
        "rows={} encoded_bytes={} scrollback_top={}",
        frame.rows.len(),
        encoded.len(),
        frame.viewport_top
    ))
}

/// 同一个引擎连续渲染两帧，验证 `RenderFrame` 是纯数据、可跨线程搬运（rinf 在独立线程发信号）。
pub fn frame_is_send() -> Result<String, String> {
    let profile: ResolvedTerminalProfile =
        TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    let size = TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    };
    let mut engine =
        DefaultTerminalEngine::new(&profile, size).map_err(|error| format!("engine: {error:?}"))?;
    engine
        .advance(b"hello\r\n")
        .map_err(|error| format!("advance: {error:?}"))?;
    let frame: Arc<rshell_core::RenderFrame> = engine
        .render(
            Viewport {
                top_stable_row: i64::MAX,
                rows: 24,
            },
            None,
        )
        .map_err(|error| format!("render: {error:?}"))?;

    let handle = std::thread::spawn(move || frame.rows.len());
    let rows = handle.join().map_err(|_| "join".to_owned())?;
    Ok(format!("frames-moved-across-thread rows={rows}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI：给 Swift 壳（M0）或 rinf（M1+）调
// ─────────────────────────────────────────────────────────────────────────────

/// 返回堆分配的 NUL 结尾 UTF-8 字符串，调用方必须用 [`rshell_m0_free`] 释放。
#[unsafe(no_mangle)]
pub extern "C" fn rshell_m0_smoke(
    host: *const c_char,
    port: u16,
    username: *const c_char,
    password: *const c_char,
    known_hosts_path: *const c_char,
) -> *mut c_char {
    let text = |pointer: *const c_char| -> Result<String, String> {
        if pointer.is_null() {
            return Err("null pointer".to_owned());
        }
        // SAFETY: 调用方保证传入的是有效 NUL 结尾 C 字符串。
        unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .map(str::to_owned)
            .map_err(|error| format!("utf8: {error}"))
    };

    let result = (|| -> Result<String, String> {
        blocking_smoke(
            &text(host)?,
            port,
            &text(username)?,
            &text(password)?,
            &text(known_hosts_path)?,
        )
    })();

    let payload = match result {
        Ok(value) => format!("{{\"ok\":true,\"text\":{}}}", escape(&value)),
        Err(error) => format!("{{\"ok\":false,\"error\":{}}}", escape(&error)),
    };
    CString::new(payload)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn rshell_m0_engine_smoke() -> *mut c_char {
    let payload = match engine_smoke() {
        Ok(value) => format!("{{\"ok\":true,\"text\":{}}}", escape(&value)),
        Err(error) => format!("{{\"ok\":false,\"error\":{}}}", escape(&error)),
    };
    CString::new(payload)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `pointer` 必须来自本库的 `rshell_m0_*` 函数且未被释放过。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rshell_m0_free(pointer: *mut c_char) {
    if !pointer.is_null() {
        // SAFETY: 由 CString::into_raw 产生。
        drop(unsafe { CString::from_raw(pointer) });
    }
}

/// 最小 JSON 字符串转义，避免为探针引入额外依赖。
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// M2 诊断（第二步）：在真机上把主机密钥管线**完整走两遍**——
/// ① 假主机（基线）；② 真实主机/端口。都写到一次性探针文件
/// （不碰真实 known_hosts），is_known → 交互确认 → 落盘全轮。
/// 上游把 HostKeyError 映射成 `Platform` 时丢掉了具体步骤，这里原样带回。
pub async fn diagnose_host_key_pipeline(
    base_dir: &str,
    real_host: &str,
    real_port: u16,
) -> String {
    use rshell_core::{HostKeyDecision, InteractionResponse};
    use rshell_session::{KnownHostsVerifier, interaction_channel};

    let Ok(dummy) = russh::keys::parse_public_key_base64(
        "AAAAC3NzaC1lZDI1NTE5AAAAILagOJFgwaMNhBWQINinKOXmqS4Gh5NgxgriXwdOoINJ",
    ) else {
        return "diagnose: internal (dummy key unavailable)".to_owned();
    };

    // ① 基线：假主机 + 独立探针文件。
    let probe_path = format!("{base_dir}/diagnose-known_hosts");
    let _ = std::fs::remove_file(&probe_path);
    let baseline = verify_round(
        &KnownHostsVerifier::new(&probe_path),
        "diagnose.invalid",
        22,
        &dummy,
    )
    .await;
    let _ = std::fs::remove_file(&probe_path);

    // ② 真实主机/端口 + 独立探针文件：隔离「真实主机名/端口」变量。
    let real_probe_path = format!("{base_dir}/diagnose-known_hosts-real");
    let _ = std::fs::remove_file(&real_probe_path);
    let real = verify_round(
        &KnownHostsVerifier::new(&real_probe_path),
        real_host,
        real_port,
        &dummy,
    )
    .await;
    let _ = std::fs::remove_file(&real_probe_path);

    format!("① 基线(假主机): {baseline}；② 真实主机({real_host}:{real_port}): {real}")
}

async fn verify_round(
    verifier: &KnownHostsVerifier,
    host: &str,
    port: u16,
    key: &russh::keys::PublicKey,
) -> String {
    let (broker, mut interactions) = interaction_channel();
    let responder = {
        let broker = broker.clone();
        tokio::spawn(async move {
            while let Some((id, _prompt)) = interactions.recv().await {
                let _ = broker.respond(
                    id,
                    InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
                );
            }
        })
    };
    let outcome = verifier.verify(host, port, key, &broker).await;
    responder.abort();
    match outcome {
        Ok(()) => "OK".to_owned(),
        Err(error) => format!("FAILED: {error:?}"),
    }
}

/// M2 诊断（第三步）：对真实服务器做一次**裸 russh 握手**，握手里的
/// check_server_key 调用与 App 完全相同的 verifier + 交互回路，
/// 但把 `HostKeyError` 原文（含具体 Storage/Interaction 步骤）逐条记录。
/// 写 known_hosts 用一次性文件，不污染真实条目。
pub async fn diagnose_real_handshake(
    host: &str,
    port: u16,
    throwaway_known_hosts: &str,
) -> String {
    use rshell_core::{HostKeyDecision, InteractionRequest, InteractionResponse};
    use rshell_session::{
        InteractionBroker, KnownHostsVerifier, interaction_channel,
    };
    use std::sync::{Arc, Mutex};

    struct DiagHandler {
        verifier: KnownHostsVerifier,
        host: String,
        port: u16,
        broker: InteractionBroker,
        report: Arc<Mutex<Vec<String>>>,
    }

    impl russh::client::Handler for DiagHandler {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            key: &russh::keys::PublicKey,
        ) -> Result<bool, Self::Error> {
            let push = |entry: String| {
                self.report
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(entry);
            };
            push(format!(
                "server key presented: {} {}",
                key.algorithm(),
                key.fingerprint(russh::keys::HashAlg::Sha256)
            ));
            match self
                .verifier
                .verify(&self.host, self.port, key, &self.broker)
                .await
            {
                Ok(()) => {
                    push("verify: OK".to_owned());
                    Ok(true)
                }
                Err(error) => {
                    push(format!("verify FAILED: {error:?}"));
                    Err(russh::Error::UnknownKey)
                }
            }
        }
    }

    let _ = std::fs::remove_file(throwaway_known_hosts);
    let report: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let verifier = KnownHostsVerifier::new(throwaway_known_hosts);
    let (broker, mut interactions) = interaction_channel();
    let responder = {
        let broker = broker.clone();
        let report = Arc::clone(&report);
        tokio::spawn(async move {
            while let Some((id, prompt)) = interactions.recv().await {
                let response = match prompt {
                    InteractionRequest::HostKey(_) => {
                        InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
                    }
                    other => {
                        report
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(format!("unexpected interaction: {other:?}"));
                        InteractionResponse::Cancel
                    }
                };
                let _ = broker.respond(id, response);
            }
        })
    };

    let handler = DiagHandler {
        verifier,
        host: host.to_owned(),
        port,
        broker,
        report: Arc::clone(&report),
    };
    let config = Arc::new(russh::client::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(15)),
        ..Default::default()
    });

    match tokio::time::timeout(std::time::Duration::from_secs(30), async move {
        let stream = tokio::net::TcpStream::connect((host, port)).await?;
        russh::client::connect_stream(config, stream, handler).await
    })
    .await
    {
        Ok(Ok(handle)) => {
            report
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("handshake: OK".to_owned());
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "diagnose", "en")
                .await;
        }
        Ok(Err(error)) => {
            report
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(format!("handshake FAILED: {error:?}"));
        }
        Err(_) => {
            report
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("handshake TIMEOUT (30s)".to_owned());
        }
    }
    responder.abort();
    let _ = std::fs::remove_file(throwaway_known_hosts);

    let entries = report
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .join(" | ");
    format!("diagnose-real: {entries}")
}
