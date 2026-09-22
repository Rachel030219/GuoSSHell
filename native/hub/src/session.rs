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

use rshell_m0::rshell_core::{
    AuthenticationKind, ConnectionProfile, HostKeyDecision, InteractionRequest,
    InteractionResponse, KeyCode, KeyModifiers, RenderFrame, ResolvedTerminalProfile,
    TerminalInput, TerminalOverrides, TerminalSettingsV1, TerminalSize, TransportKind, Viewport,
};
use rshell_m0::rshell_session::{
    AuthPlan, DefaultTerminalEngine, KnownHostsVerifier, NativeSshTransport, SessionTransport,
    TerminalEngine, TransportEvent, TransportRequest, interaction_channel,
};

use crate::frame_codec::pack_runs;
use crate::signals::{
    ConnectRequest, DisconnectRequest, FrameUpdate, InputRequest, PerfStats, ResizeRequest,
    SessionState, SessionStatus,
};

const NO_CURSOR: i32 = -1;
/// 远端 PTY 最小可行尺寸；Flutter 在极端布局下可能量出 0。
const MIN_COLS: u16 = 2;
const MIN_ROWS: u16 = 2;

enum SessionCommand {
    Resize(TerminalSize),
    Disconnect,
    Input(TerminalInput),
}

/// 常驻任务：接住 Dart 的三种请求，把 Resize/Disconnect 转交给当前会话。
/// 每个会话一个独立任务；新连接顶掉旧连接（M1 是单会话 UI）。
pub async fn supervisor() {
    let connect_rx = ConnectRequest::get_dart_signal_receiver();
    let resize_rx = ResizeRequest::get_dart_signal_receiver();
    let disconnect_rx = DisconnectRequest::get_dart_signal_receiver();
    let input_rx = InputRequest::get_dart_signal_receiver();
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
            pack = input_rx.recv() => {
                let Some(pack) = pack else { break };
                if let Some(tx) = &session_tx {
                    if let Some(input) = terminal_input_from_request(pack.message) {
                        let _ = tx.send(SessionCommand::Input(input));
                    }
                }
            }
        }
    }
}

async fn run_session(
    request: ConnectRequest,
    pending_size: Option<TerminalSize>,
    mut commands: UnboundedReceiver<SessionCommand>,
) {
    let target = format!("{}:{}", request.host, request.port);
    send_status(SessionState::Connecting, target.clone());

    let mut profile = ConnectionProfile::new("guosh", &request.host);
    profile.host = request.host.clone();
    profile.port = request.port;
    profile.username = request.username.clone();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;
    // 非空命令 = exec 模式：PTY 照开，但远端直接执行该命令而不是 shell
    //（上游 configure_channel 的行为）。M1 帧率实测（top/htop）靠它。
    profile.remote_command = if request.command.is_empty() {
        None
    } else {
        Some(request.command.clone())
    };

    // 密码在这里包成 SecretString（PLAN.md §2.2：secret 不可序列化，协议层手写转换）。
    let auth =
        match AuthPlan::from_secret(&profile, Some(SecretString::from(request.password.clone()))) {
            Ok(auth) => auth,
            Err(error) => {
                send_status(SessionState::Failed, format!("AuthPlan: {error:?}"));
                return;
            }
        };

    let Some(known_hosts_path) = known_hosts_path() else {
        send_status(
            SessionState::Failed,
            "no writable HOME directory".to_owned(),
        );
        return;
    };
    // 诊断探针：连接失败且失败类别是 Platform 时，把主机密钥管线的
    // 具体失败步骤附在 Failed 详情里（上游只给类别，细节被丢弃）。
    // ⚠ 探针要拿 known_hosts 的**父目录**当 base_dir——传文件路径会让
    // 探针文件落在「文件下面」，CreateParent 必然失败（曾因此误诊实机）。
    let known_hosts_dir = std::path::Path::new(&known_hosts_path)
        .parent()
        .and_then(|parent| parent.to_str())
        .unwrap_or(&known_hosts_path)
        .to_owned();
    let known_hosts_diagnosis =
        rshell_m0::diagnose_host_key_pipeline(&known_hosts_dir, &request.host, request.port)
            .await;
    let handshake_diagnosis = format!(
        "{} {}",
        known_hosts_diagnosis,
        rshell_m0::diagnose_real_handshake(
            &request.host,
            request.port,
            &format!("{known_hosts_path}-diagnose"),
        )
        .await
    );
    debug_print!("[session] {handshake_diagnosis}");
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
            send_status(
                SessionState::Failed,
                format!("NativeSshTransport: {error:?}"),
            );
            responder.abort();
            return;
        }
    };
    if let Err(error) = transport
        .connect(&TransportRequest::new(size), broker)
        .await
    {
        debug_print!("[session] connect failed: {error:?}");
        send_status(
                        SessionState::Failed,
                        format!("connect: {error:?} · {handshake_diagnosis}"),
                    );
        responder.abort();
        return;
    }
    debug_print!("[session] connected, starting engine loop");
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
    let mut stats = PerfWindow::new();

    // 连接建立即送第一帧（欢迎横幅可能已经进了引擎）。
    if let Ok(frame) = engine.render(viewport, None) {
        send_frame(&frame, &mut stats, 0);
    }
    if let Some(perf) = stats.maybe_report() {
        perf.send_signal_to_dart();
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
                                let render_start = std::time::Instant::now();
                                match engine.render(viewport, None) {
                                    Ok(frame) => {
                                        let render_us = micros_since(render_start);
                                        send_frame(&frame, &mut stats, render_us);
                                        if let Some(perf) = stats.maybe_report() {
                                            perf.send_signal_to_dart();
                                        }
                                    }
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
                    let render_start = std::time::Instant::now();
                    if let Ok(frame) = engine.render(viewport, None) {
                        let render_us = micros_since(render_start);
                        send_frame(&frame, &mut stats, render_us);
                    }
                }
                Some(SessionCommand::Input(input)) => {
                    // M2 输入闭环：键编码（ETX/Kitty/CSI-u）在引擎里，
                    // 这里只负责把编码结果写进 transport。
                    match engine.encode_input(input) {
                        Ok(bytes) if !bytes.is_empty() => {
                            if let Err(error) = transport.write(&bytes).await {
                                send_status(
                                    SessionState::Failed,
                                    format!("input write: {error:?}"),
                                );
                                break;
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            send_status(SessionState::Failed, format!("encode_input: {error:?}"));
                            break;
                        }
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

/// 把边界上的键名解析成上游 `KeyCode`（PLAN §5 M2：编码权威在 Rust）。
/// 返回 `None` = 无法识别的键，静默丢弃（不记日志——键入高频路径）。
fn parse_key_code(key: &str) -> Option<KeyCode> {
    const CHAR_PREFIX: &str = "character:";
    if let Some(rest) = key.strip_prefix(CHAR_PREFIX) {
        let mut chars = rest.chars();
        let first = chars.next()?;
        if chars.next().is_none() {
            return Some(KeyCode::Character(first));
        }
        return None;
    }
    Some(match key {
        "enter" => KeyCode::Enter,
        "escape" => KeyCode::Escape,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "page_up" => KeyCode::PageUp,
        "page_down" => KeyCode::PageDown,
        "arrow_up" => KeyCode::ArrowUp,
        "arrow_down" => KeyCode::ArrowDown,
        "arrow_left" => KeyCode::ArrowLeft,
        "arrow_right" => KeyCode::ArrowRight,
        other => {
            let digits = other.strip_prefix('f')?;
            let index = digits.parse::<u8>().ok()?;
            if !(1..=24).contains(&index) {
                return None;
            }
            KeyCode::F(index)
        }
    })
}

fn terminal_input_from_request(request: InputRequest) -> Option<TerminalInput> {
    if !request.text.is_empty() {
        return Some(TerminalInput::CommittedText(request.text));
    }
    let code = parse_key_code(&request.key)?;
    Some(TerminalInput::Key {
        code,
        modifiers: KeyModifiers {
            shift: request.shift,
            control: request.control,
            alt: request.alt,
            super_key: false,
        },
    })
}

/// 一个统计窗口（5 秒）内的帧开销累计 + 会话帧序号。
struct PerfWindow {
    window_start: std::time::Instant,
    seq: u32,
    frames: u32,
    render_us_total: u64,
    render_us_max: u32,
    pack_us_total: u64,
    pack_us_max: u32,
    bytes_total: u64,
}

impl PerfWindow {
    fn new() -> Self {
        Self {
            window_start: std::time::Instant::now(),
            seq: 0,
            frames: 0,
            render_us_total: 0,
            render_us_max: 0,
            pack_us_total: 0,
            pack_us_max: 0,
            bytes_total: 0,
        }
    }

    fn record(&mut self, render_us: u32, pack_us: u32, bytes: usize) -> u32 {
        self.seq = self.seq.wrapping_add(1);
        self.frames += 1;
        self.render_us_total += u64::from(render_us);
        self.render_us_max = self.render_us_max.max(render_us);
        self.pack_us_total += u64::from(pack_us);
        self.pack_us_max = self.pack_us_max.max(pack_us);
        self.bytes_total += bytes as u64;
        self.seq
    }

    /// 满 5 秒发一条汇总并重开窗口。
    fn maybe_report(&mut self) -> Option<PerfStats> {
        let elapsed = self.window_start.elapsed();
        if elapsed < std::time::Duration::from_secs(5) || self.frames == 0 {
            return None;
        }
        let frames = self.frames;
        let stats = PerfStats {
            frames,
            window_ms: u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX),
            render_us_avg: (self.render_us_total / u64::from(frames)) as u32,
            render_us_max: self.render_us_max,
            pack_us_avg: (self.pack_us_total / u64::from(frames)) as u32,
            pack_us_max: self.pack_us_max,
            bytes_avg: (self.bytes_total / u64::from(frames)) as u32,
        };
        *self = Self {
            seq: self.seq,
            ..Self::new()
        };
        Some(stats)
    }
}

fn micros_since(start: std::time::Instant) -> u32 {
    u32::try_from(start.elapsed().as_micros()).unwrap_or(u32::MAX)
}

fn send_frame(frame: &RenderFrame, stats: &mut PerfWindow, render_us: u32) {
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
    let pack_start = std::time::Instant::now();
    let binary = pack_runs(frame);
    let pack_us = micros_since(pack_start);
    let seq = stats.record(render_us, pack_us, binary.len());
    FrameUpdate {
        cols: frame.size.cols,
        rows: frame.size.rows,
        seq,
        cursor_col,
        cursor_row,
    }
    .send_signal_to_dart(binary);
}

/// known_hosts 落在 App 沙箱内（iOS 的 `HOME` 就是容器主目录）。
/// 第二次连接不再触发主机密钥交互——M0b 验收的同一条性质。
///
/// ⚠ 实机实测（M2，iPhone）：真机上 `HOME` 不一定指向可写的容器目录
/// （探针报 `Storage { step: CreateParent }`），所以不能用「HOME 存在」
/// 想当然——以**实际创建目录成功**为准，失败回退沙箱 `tmp`（TMPDIR 由
/// 系统注入，必在容器内）。tmp 可能被系统清理 → 重新 TOFU（M2 是自动
/// 接受，无 UX 影响）；目录决策随 M3 的 Keychain/设置一起重定。
fn known_hosts_path() -> Option<String> {
    let candidates = [
        std::env::var("HOME")
            .ok()
            .map(|home| std::path::PathBuf::from(home).join(".guosh")),
        Some(std::env::temp_dir().join("guosh")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if std::fs::create_dir_all(&candidate).is_ok() {
            return Some(candidate.join("known_hosts").display().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::{parse_key_code, terminal_input_from_request};
    use crate::signals::InputRequest;
    use rshell_m0::rshell_core::{KeyCode, TerminalInput};

    fn input(key: &str, control: bool) -> Option<TerminalInput> {
        terminal_input_from_request(InputRequest {
            text: String::new(),
            key: key.to_owned(),
            shift: false,
            control,
            alt: false,
        })
    }

    #[test]
    fn named_keys_parse() {
        for (name, expected) in [
            ("enter", KeyCode::Enter),
            ("escape", KeyCode::Escape),
            ("tab", KeyCode::Tab),
            ("backspace", KeyCode::Backspace),
            ("arrow_up", KeyCode::ArrowUp),
            ("arrow_left", KeyCode::ArrowLeft),
        ] {
            assert!(matches!(input(name, false), Some(TerminalInput::Key { code, .. }) if code == expected));
        }
        assert!(matches!(
            input("f12", false),
            Some(TerminalInput::Key { code: KeyCode::F(12), .. })
        ));
        assert!(input("f25", false).is_none());
        assert!(input("not_a_key", false).is_none());
    }

    #[test]
    fn character_key_parses() {
        assert!(matches!(
            input("character:c", false),
            Some(TerminalInput::Key { code: KeyCode::Character('c'), .. })
        ));
        // 多字符不是合法键
        assert!(input("character:ab", false).is_none());
    }

    #[test]
    fn text_wins_and_carries_no_key() {
        let request = InputRequest {
            text: "你好".to_owned(),
            key: String::new(),
            shift: false,
            control: false,
            alt: false,
        };
        assert!(matches!(
            terminal_input_from_request(request),
            Some(TerminalInput::CommittedText(text)) if text == "你好"
        ));
    }

    #[test]
    fn modifiers_survive_the_boundary() {
        assert!(matches!(
            input("character:c", true),
            Some(TerminalInput::Key { code: KeyCode::Character('c'), modifiers })
                if modifiers.control && !modifiers.shift && !modifiers.alt
        ));
    }
}
