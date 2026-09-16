//! 会话 actor：把 M0 验证过的装配（`rust/src/lib.rs` 的 smoke 路径——
//! `ConnectionProfile` / `AuthPlan` / `KnownHostsVerifier` / `interaction_channel` /
//! `NativeSshTransport`）接上 rinf 信号，并把远端字节喂进
//! `DefaultTerminalEngine` → `render` → run 压缩 → `FrameUpdate`。
//!
//! 单任务拥有 transport（它的方法都要 `&mut self`），用 `select!` 同时听
//! 远端事件与 Dart 命令——这是上游 actor 的形状，M1 用裸 tokio 任务就够了。

use rinf::{DartSignal, RustSignal, RustSignalBinary, debug_print};
use secrecy::SecretString;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use rshell_core::{
    AuthenticationKind, ConnectionProfile, HostKeyDecision, InteractionRequest,
    InteractionResponse, RenderFrame, ResolvedTerminalProfile, TerminalOverrides,
    TerminalSettingsV1, TerminalSize, TransportKind, Viewport,
};
use rshell_session::{
    AuthPlan, DefaultTerminalEngine, KnownHostsVerifier, NativeSshTransport, SessionTransport,
    TerminalEngine, TransportEvent, TransportRequest, interaction_channel,
};

use crate::frame_codec::pack_runs;
use crate::signals::{
    ConnectRequest, DisconnectRequest, FrameUpdate, ResizeRequest, SessionState, SessionStatus,
};

const NO_CURSOR: i32 = -1;
/// 远端 PTY 最小可行尺寸；Flutter 在极端布局下可能量出 0。
const MIN_COLS: u16 = 2;
const MIN_ROWS: u16 = 2;

enum SessionCommand {
    Resize(TerminalSize),
    Disconnect,
}

/// 常驻任务：接住 Dart 的三种请求，把 Resize/Disconnect 转交给当前会话。
/// 每个会话一个独立任务；新连接顶掉旧连接（M1 是单会话 UI）。
pub async fn supervisor() {
    let connect_rx = ConnectRequest::get_dart_signal_receiver();
    let resize_rx = ResizeRequest::get_dart_signal_receiver();
    let disconnect_rx = DisconnectRequest::get_dart_signal_receiver();
    let mut session_tx: Option<UnboundedSender<SessionCommand>> = None;
    // 连接表单阶段 Dart 就会量格子并发 ResizeRequest（此刻还没有会话）。
    // 记住最新几何，spawn 会话时带上——否则首个 PTY 尺寸只能用请求里的缺省值。
    let mut latest_size: Option<TerminalSize> = None;

    loop {
        tokio::select! {
            pack = connect_rx.recv() => {
                let Some(pack) = pack else { break };
                debug_print!("[session] connect request: {}:{} (pending size {:?})",
                    pack.message.host, pack.message.port,
                    latest_size.map(|size| (size.cols, size.rows)));
                if let Some(old) = session_tx.take() {
                    let _ = old.send(SessionCommand::Disconnect);
                }
                let (tx, rx) = unbounded_channel();
                session_tx = Some(tx);
                tokio::spawn(run_session(pack.message, latest_size, rx));
            }
            pack = resize_rx.recv() => {
                let Some(pack) = pack else { break };
                let request = pack.message;
                let size = TerminalSize {
                    cols: request.cols,
                    rows: request.rows,
                    pixel_width: request.pixel_width,
                    pixel_height: request.pixel_height,
                    dpi: request.dpi,
                };
                latest_size = Some(size);
                if let Some(tx) = &session_tx {
                    let _ = tx.send(SessionCommand::Resize(size));
                }
            }
            pack = disconnect_rx.recv() => {
                let Some(_pack) = pack else { break };
                if let Some(tx) = session_tx.take() {
                    let _ = tx.send(SessionCommand::Disconnect);
                }
            }
        }
    }
}

async fn run_session(
    request: ConnectRequest,
    pending_size: Option<TerminalSize>,
    mut commands: UnboundedReceiver<SessionCommand>,
) {    let target = format!("{}:{}", request.host, request.port);
    send_status(SessionState::Connecting, target.clone());

    let mut profile = ConnectionProfile::new("guosh", &request.host);
    profile.host = request.host.clone();
    profile.port = request.port;
    profile.username = request.username.clone();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;

    // 密码在这里包成 SecretString（PLAN.md §2.2：secret 不可序列化，协议层手写转换）。
    let auth = match AuthPlan::from_secret(
        &profile,
        Some(SecretString::from(request.password.clone())),
    ) {
        Ok(auth) => auth,
        Err(error) => {
            send_status(SessionState::Failed, format!("AuthPlan: {error:?}"));
            return;
        }
    };

    let Some(known_hosts_path) = known_hosts_path() else {
        send_status(SessionState::Failed, "no writable HOME directory".to_owned());
        return;
    };
    let verifier = KnownHostsVerifier::new(&known_hosts_path);
    let (broker, mut interactions) = interaction_channel();

    // M1 沿用 M0 的 TOFU：主机密钥一律接受并落盘（PLAN.md §8，M3 换成真实确认 UI）。
    let responder = {
        let broker = broker.clone();
        tokio::spawn(async move {
            while let Some((id, prompt)) = interactions.recv().await {
                let response = match prompt {
                    InteractionRequest::HostKey(_) => {
                        InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
                    }
                    _ => InteractionResponse::Cancel,
                };
                let _ = broker.respond(id, response);
            }
        })
    };

    let size = pending_size.unwrap_or(TerminalSize {
        cols: request.cols.max(MIN_COLS),
        rows: request.rows.max(MIN_ROWS),
        pixel_width: request.pixel_width,
        pixel_height: request.pixel_height,
        dpi: request.dpi,
    });

    let mut transport = match NativeSshTransport::new(profile, auth, verifier) {
        Ok(transport) => transport,
        Err(error) => {
            send_status(SessionState::Failed, format!("NativeSshTransport: {error:?}"));
            responder.abort();
            return;
        }
    };
    if let Err(error) = transport.connect(&TransportRequest::new(size), broker).await {        debug_print!("[session] connect failed: {error:?}");
        send_status(SessionState::Failed, format!("connect: {error:?}"));
        responder.abort();
        return;
    }    debug_print!("[session] connected, starting engine loop");
    send_status(SessionState::Connected, target);

    let term_profile: ResolvedTerminalProfile =
        TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    let mut engine = match DefaultTerminalEngine::new(&term_profile, size) {
        Ok(engine) => engine,
        Err(error) => {
            send_status(SessionState::Failed, format!("engine: {error:?}"));
            responder.abort();
            return;
        }
    };
    let mut viewport = Viewport {
        top_stable_row: i64::MAX, // 永远看最底部（活动区），与 bench/探针一致
        rows: size.rows,
    };

    // 连接建立即送第一帧（欢迎横幅可能已经进了引擎）。
    if let Ok(frame) = engine.render(viewport, None) {
        send_frame(&frame);
    }

    loop {
        tokio::select! {
            event = transport.next_event() => match event {
                Ok(TransportEvent::Output(bytes)) => {
                    match engine.advance(&bytes) {
                        Ok(delta) => {
                            // 引擎对远端查询的应答（DA / 光标位置报告…）必须回写，
                            // 否则对端会一直等（m0 探针忽略它只是因为探针不需要应答）。
                            if !delta.outbound.is_empty() {
                                let _ = transport.write(&delta.outbound).await;
                            }
                            if delta.dirty {
                                match engine.render(viewport, None) {
                                    Ok(frame) => send_frame(&frame),
                                    Err(error) => {
                                        send_status(SessionState::Failed, format!("render: {error:?}"));
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            send_status(SessionState::Failed, format!("advance: {error:?}"));
                            break;
                        }
                    }
                }
                Ok(TransportEvent::Failure(failure)) => {
                    send_status(SessionState::Failed, format!("session: {failure:?}"));
                    break;
                }
                Ok(TransportEvent::Eof) => {
                    send_status(SessionState::Closed, "eof".to_owned());
                    break;
                }
                Ok(TransportEvent::Exit(status)) => {
                    send_status(
                        SessionState::Closed,
                        format!("exit code={:?} success={}", status.code, status.success),
                    );
                    break;
                }
                Ok(_) => {}
                Err(error) => {
                    send_status(SessionState::Failed, format!("transport: {error:?}"));
                    break;
                }
            },
            command = commands.recv() => match command {
                Some(SessionCommand::Resize(new_size)) => {
                    if let Err(error) = engine.resize(new_size) {
                        send_status(SessionState::Failed, format!("engine resize: {error:?}"));
                        break;
                    }
                    viewport.rows = new_size.rows;
                    if let Err(error) = transport.resize(new_size).await {
                        // window-change 失败不立刻判死；远端布局暂旧，后续 resize 可再试。
                        rinf::debug_print!("window-change failed: {error:?}");
                    }
                    if let Ok(frame) = engine.render(viewport, None) {
                        send_frame(&frame);
                    }
                }
                Some(SessionCommand::Disconnect) | None => {
                    send_status(SessionState::Closed, "disconnect".to_owned());
                    break;
                }
            },
        }
    }

    let _ = transport.shutdown().await;
    responder.abort();
}

fn send_status(state: SessionState, detail: String) {
    SessionStatus { state, detail }.send_signal_to_dart();
}

fn send_frame(frame: &RenderFrame) {
    let (cursor_col, cursor_row) = match &frame.cursor {
        Some(cursor) => {
            let row = cursor.position.stable_row - frame.viewport_top;
            let visible = row >= 0 && row < i64::try_from(frame.rows.len()).unwrap_or(i64::MAX);
            if visible {
                (i32::from(cursor.position.column), row as i32)
            } else {
                (NO_CURSOR, NO_CURSOR)
            }
        }
        None => (NO_CURSOR, NO_CURSOR),
    };
    FrameUpdate {
        cols: frame.size.cols,
        rows: frame.size.rows,
        generation: frame.generation,
        cursor_col,
        cursor_row,
    }
    .send_signal_to_dart(pack_runs(frame));
}

/// known_hosts 落在 App 沙箱内（iOS 的 `HOME` 就是容器主目录）。
/// 第二次连接不再触发主机密钥交互——M0b 验收的同一条性质。
fn known_hosts_path() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let dir = std::path::Path::new(&home).join(".guosh");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("known_hosts").display().to_string())
}
