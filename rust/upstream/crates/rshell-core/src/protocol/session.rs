use std::{fmt, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::render::{
    ExitStatus, RenderFrame, SearchMatch, SearchQuery, SelectionRange, SessionFailure,
    SessionState, TerminalDisplayModes, TerminalInput, TerminalMouseEvent, TerminalSize,
};

use super::interactions::{InteractionId, InteractionRequest, InteractionResponse};

pub enum SessionUiCommand {
    Interrupt,
    ResetDisplay,
    Input(TerminalInput),
    Mouse(TerminalMouseEvent),
    Paste(secrecy::SecretString),
    Resize(TerminalSize),
    Scroll(i32),
    Search(SearchQuery),
    Select(SelectionRange),
    CopySelection,
    ClearScrollback,
    Respond {
        interaction: InteractionId,
        response: InteractionResponse,
    },
    Reconnect,
    Shutdown,
}

impl SessionUiCommand {
    pub fn paste(text: String) -> Self {
        Self::Paste(secrecy::SecretString::from(text))
    }

    pub fn paste_matches(&self, expected: &str) -> bool {
        use secrecy::ExposeSecret;

        matches!(self, Self::Paste(value) if value.expose_secret() == expected)
    }

    pub fn paste_len(&self) -> Option<usize> {
        use secrecy::ExposeSecret;

        match self {
            Self::Paste(value) => Some(value.expose_secret().len()),
            _ => None,
        }
    }
}

impl fmt::Debug for SessionUiCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupt => formatter.write_str("Interrupt"),
            Self::ResetDisplay => formatter.write_str("ResetDisplay"),
            Self::Input(input) => formatter.debug_tuple("Input").field(input).finish(),
            Self::Mouse(event) => formatter.debug_tuple("Mouse").field(event).finish(),
            Self::Paste(_) => formatter.write_str("Paste([REDACTED])"),
            Self::Resize(size) => formatter.debug_tuple("Resize").field(size).finish(),
            Self::Scroll(delta) => formatter.debug_tuple("Scroll").field(delta).finish(),
            Self::Search(query) => formatter.debug_tuple("Search").field(query).finish(),
            Self::Select(range) => formatter.debug_tuple("Select").field(range).finish(),
            Self::CopySelection => formatter.write_str("CopySelection"),
            Self::ClearScrollback => formatter.write_str("ClearScrollback"),
            Self::Respond {
                interaction,
                response,
            } => formatter
                .debug_struct("Respond")
                .field("interaction", interaction)
                .field("response", response)
                .finish(),
            Self::Reconnect => formatter.write_str("Reconnect"),
            Self::Shutdown => formatter.write_str("Shutdown"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayRecoveryNotice {
    pub interrupted_generation: u64,
    pub observed_generation: u64,
    pub modes: TerminalDisplayModes,
}

#[derive(Clone, PartialEq)]
pub enum SessionUiEvent {
    State(SessionState),
    Frame(Arc<RenderFrame>),
    RecoveryChanged(Option<DisplayRecoveryNotice>),
    Search(Vec<SearchMatch>),
    Copy(String),
    InteractionRequired(InteractionRequest),
    Exited(ExitStatus),
    Failed(SessionFailure),
    Crashed(String),
}

impl fmt::Debug for SessionUiEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(state) => formatter.debug_tuple("State").field(state).finish(),
            Self::Frame(frame) => formatter.debug_tuple("Frame").field(frame).finish(),
            Self::RecoveryChanged(notice) => formatter
                .debug_tuple("RecoveryChanged")
                .field(notice)
                .finish(),
            Self::Search(matches) => formatter.debug_tuple("Search").field(matches).finish(),
            Self::Copy(_) => formatter.write_str("Copy([REDACTED])"),
            Self::InteractionRequired(request) => formatter
                .debug_tuple("InteractionRequired")
                .field(request)
                .finish(),
            Self::Exited(status) => formatter.debug_tuple("Exited").field(status).finish(),
            Self::Failed(failure) => formatter.debug_tuple("Failed").field(failure).finish(),
            Self::Crashed(_) => formatter.write_str("Crashed([REDACTED])"),
        }
    }
}
