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
