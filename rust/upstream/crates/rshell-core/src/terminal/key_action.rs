use super::{SettingsValidationCode, SettingsValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSendSequence {
    Vt220Delete,
    Delete127,
    Backspace8,
}

impl TerminalSendSequence {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Vt220Delete => "\u{1b}[3~",
            Self::Delete127 => "\u{7f}",
            Self::Backspace8 => "\u{8}",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalKeyAction {
    Send(TerminalSendSequence),
    ClearScrollback,
    NewTab,
    SplitVertical,
}

pub fn parse_terminal_key_action(
    action: &str,
) -> Result<TerminalKeyAction, SettingsValidationError> {
    match action {
        "send:\u{1b}[3~" => Ok(TerminalKeyAction::Send(TerminalSendSequence::Vt220Delete)),
        "send:\u{7f}" => Ok(TerminalKeyAction::Send(TerminalSendSequence::Delete127)),
        "send:\u{8}" => Ok(TerminalKeyAction::Send(TerminalSendSequence::Backspace8)),
        "clear_scrollback" => Ok(TerminalKeyAction::ClearScrollback),
        "new_tab" => Ok(TerminalKeyAction::NewTab),
        "split_vertical" => Ok(TerminalKeyAction::SplitVertical),
        _ => Err(SettingsValidationError {
            field: "key_bindings.action",
            code: SettingsValidationCode::InvalidAction,
        }),
    }
}
