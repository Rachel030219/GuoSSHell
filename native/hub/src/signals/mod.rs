//! rinf 边界上的信号类型（适配层）。
//!
//! 业务模型（`RenderFrame` / `SessionUiCommand` ...）属于上游 `rshell-core::protocol`，
//! 不在这里重建（PLAN.md 铁律 1/4）。这里只放三样东西：
//! * Dart → Rust 的请求信号（协议类型的平铺投影）
//! * Rust → Dart 的状态信号
//! * Rust → Dart 的帧信号（结构化字段 + 二进制通道里的 run 压缩字节流）

use rinf::{DartSignal, RustSignal, RustSignalBinary, SignalPiece};
use serde::{Deserialize, Serialize};

// ── Dart → Rust ──────────────────────────────────────────────────────────────

/// 建立一条 SSH 会话。
///
/// 密码按明文过边界——上游的 `SecretString` 刻意不可序列化（PLAN.md §2.2），
/// Rust 在接收处立刻包成 `SecretString`，之后不再以明文持有、不进日志。
#[derive(Deserialize, DartSignal)]
pub struct ConnectRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// 非空 = exec 模式：连接后在远端直接执行该命令（如 `top`），不进 shell。
    /// M1 的帧率实测靠它，不需要输入能力。
    pub command: String,
    /// 首次几何。度量的唯一权威是 Flutter（PLAN.md §8）：
    /// 它量完格子后随连接请求一起带来。
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: u32,
}

/// 视口变化（旋转 / 改窗口）。→ `engine.resize` + `transport.resize`（远端 window-change）。
#[derive(Deserialize, DartSignal)]
pub struct ResizeRequest {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: u32,
}

#[derive(Deserialize, DartSignal)]
pub struct DisconnectRequest {}

/// 终端输入（M2 输入闭环的边界）。
///
/// 键与文本二选一：`text` 非空 = IME 提交/粘贴的文本（`CommittedText`）；
/// 否则 `key` 携带键名——`"character:x"`（单字符）或命名键
/// （enter/escape/tab/backspace/delete/insert/home/end/page_up/page_down/
/// arrow_up/arrow_down/arrow_left/arrow_right/`f:N`）。
/// 键编码（ETX/Kitty/CSI-u…）是 Rust 侧 `encode_input` 的事，Dart 只转发。
#[derive(Deserialize, DartSignal)]
pub struct InputRequest {
    pub text: String,
    pub key: String,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// 触摸鼠标事件（M2a 鼠标转发的边界）。
///
/// Rust 组装成 `TerminalMouseEvent` 交给 `engine.encode_mouse`；远端没开
/// 鼠标上报时 encode 返回 Err，静默忽略（触摸行为完全不变）。
/// 滚轮必须用 `kind: "scroll"`（上游 validate 会拒绝「滚轮走 press」）。
#[derive(Deserialize, DartSignal)]
pub struct MouseRequest {
    /// press / release / scroll
    pub kind: String,
    /// left / middle / right / wheel_up / wheel_down
    pub button: String,
    pub col: u16,
    pub row: u16,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// 选区变更（M2a 方案 A：选区的权威在引擎，M2a 之前我们在 Dart 侧重造了一遍）。
///
/// `clear = true` 表示清除，其余字段忽略；否则 anchor/focus 是**引擎绝对行号**
/// （`stable_row`，不是视口行号——Dart 已换算好）。两个端点不分先后，引擎
/// 渲染/取文时自己排序，所以拖耳朵越过对端不用特殊处理。
#[derive(Deserialize, DartSignal)]
pub struct SelectionRequest {
    pub clear: bool,
    pub anchor_row: i64,
    pub anchor_col: u16,
    pub focus_row: i64,
    pub focus_col: u16,
    /// 方块选（列选区）。M2a 只做整行，恒 false。
    pub rectangular: bool,
}

/// 复制当前选区（取文在引擎里，见 `TerminalEngine::selected_text`）。
#[derive(Deserialize, DartSignal)]
pub struct CopyRequest {}

// ── Rust → Dart ──────────────────────────────────────────────────────────────

#[derive(Serialize, SignalPiece)]
pub enum SessionState {
    Connecting,
    Connected,
    Failed,
    Closed,
}

#[derive(Serialize, RustSignal)]
pub struct SessionStatus {
    pub state: SessionState,
    /// 人类可读的补充信息（失败分类 / 退出码等）。绝不包含密码。
    pub detail: String,
}

/// 一帧终端画面。二进制部分是 [`crate::frame_codec::pack_runs`] 的产物，
/// 走 `RustSignalBinary` 的原始字节通道（PLAN.md §4.3 的落地方案）。
#[derive(Serialize, RustSignalBinary)]
pub struct FrameUpdate {
    pub cols: u16,
    pub rows: u16,
    /// 本会话内单调递增的帧序号。Dart 侧用它数**丢帧**：
    /// 收到的 seq 跳变 = 中间有帧没送达（验收：表现为晚一帧，不是花屏）。
    pub seq: u32,
    /// 光标的视口内坐标（列, 行）。`-1` 表示不可见（隐藏或滚出视口）。
    pub cursor_col: i32,
    pub cursor_row: i32,
    /// 远端开启了鼠标上报（DECSET 1000/1002/1003）。Dart 据此决定
    /// 触摸点击/滚轮是转发给远端还是保持本地行为（M2a）。
    pub mouse_reporting: bool,
    /// 远端在备用屏（vim/less 等 TUI）。
    pub alternate_screen: bool,
}

/// 选区回显（引擎当前持有选区的原样投影）。
///
/// Dart 用它给耳朵/气泡定位。`anchor`/`focus` **保留 Dart 传入时的角色、不排序**
/// ——拖耳朵越过对端时角色才不会乱（引擎自己渲染/取文时才排序）。
#[derive(Serialize, RustSignal)]
pub struct SelectionState {
    pub has_selection: bool,
    pub anchor_row: i64,
    pub anchor_col: u16,
    pub focus_row: i64,
    pub focus_col: u16,
}

/// 引擎取出的选区文本。引擎已按自己的规则跨行拼接、裁掉行尾空格（`text.rs`
/// 的 `selection_text`），Dart 直接进剪贴板。无选区时是空串。
#[derive(Serialize, RustSignal)]
pub struct ClipboardText {
    pub text: String,
}

/// 每 5 秒一条的性能汇总（M1 帧率验收的数字来源）。
/// 单帧预算 16.67 ms（PLAN §4）：render_us + pack_us 的 max 是 Rust 侧的真实开销。
#[derive(Serialize, RustSignal)]
pub struct PerfStats {
    /// 统计窗口内实际打包发出的帧数。fps = frames / window_ms * 1000。
    pub frames: u32,
    pub window_ms: u32,
    pub render_us_avg: u32,
    pub render_us_max: u32,
    pub pack_us_avg: u32,
    pub pack_us_max: u32,
    /// 每帧压缩字节数的平均值（典型 5.8–7.2 KB，TUI 满屏最坏 61 KB）。
    pub bytes_avg: u32,
}
