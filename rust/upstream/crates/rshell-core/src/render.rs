use std::{fmt, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::terminal::{KeyCode, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    pub top_stable_row: i64,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellPosition {
    pub stable_row: i64,
    pub column: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRange {
    pub start: CellPosition,
    pub end: CellPosition,
    pub rectangular: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub needle: String,
    pub case_sensitive: bool,
    pub regex: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub start: CellPosition,
    pub end: CellPosition,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDisplayModes {
    pub alternate_screen: bool,
    pub enhanced_keyboard: bool,
    pub mouse_reporting: bool,
    pub application_cursor: bool,
    pub cursor_hidden: bool,
    pub stale_title: bool,
}

impl TerminalDisplayModes {
    pub const fn has_residue(self) -> bool {
        self.alternate_screen
            || self.enhanced_keyboard
            || self.mouse_reporting
            || self.application_cursor
            || self.cursor_hidden
            || self.stale_title
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRecovery {
    pub before: TerminalDisplayModes,
    pub after: TerminalDisplayModes,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderFrame {
    pub generation: u64,
    pub size: TerminalSize,
    pub viewport_top: i64,
    pub rows: Arc<[RenderRow]>,
    pub cursor: Option<RenderCursor>,
    pub title: String,
    #[serde(default)]
    pub display_modes: TerminalDisplayModes,
    #[serde(default)]
    pub alternate_screen: bool,
    #[serde(default)]
    pub mouse_reporting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderRow {
    pub stable_row: i64,
    pub wrapped: bool,
    pub cells: Arc<[RenderCell]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderCell {
    pub text: String,
    pub width: u8,
    pub foreground: Color,
    pub background: Color,
    pub attributes: CellAttributes,
    #[serde(default)]
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    Default,
    Ansi(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellAttributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub reverse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderCursor {
    pub position: CellPosition,
    pub shape: CursorShape,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Block,
    Beam,
    Underline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    pub code: Option<i32>,
    pub success: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionFailure {
    Validation,
    Storage,
    Vault,
    HostKeyRejected,
    HostKeyChanged,
    Authentication,
    Network,
    Pty,
    SshChannel,
    Subprocess,
    Platform,
    Backpressure,
    Timeout,
    Crashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Created,
    Connecting,
    AwaitingHostKey,
    AwaitingAuthentication,
    Connected,
    Reconnecting,
    Closing,
    Exited,
    Failed,
    Crashed,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalInput {
    CommittedText(String),
    Key {
        code: KeyCode,
        modifiers: KeyModifiers,
    },
}

impl fmt::Debug for TerminalInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommittedText(_) => formatter.write_str("CommittedText([REDACTED])"),
            Self::Key { code, modifiers } => formatter
                .debug_struct("Key")
                .field("code", code)
                .field("modifiers", modifiers)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalMouseEvent {
    pub kind: MouseEventKind,
    pub button: Option<MouseButton>,
    pub cell: CellPosition,
    /// Zero-based row in the frame viewport captured with this event.
    #[serde(default)]
    pub viewport_row: u16,
    pub pixel_x: u32,
    pub pixel_y: u32,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseEventKind {
    Press,
    Release,
    Move,
    Scroll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
    WheelUp,
    WheelDown,
}
