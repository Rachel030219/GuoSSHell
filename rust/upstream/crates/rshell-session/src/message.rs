use std::{fmt, sync::Arc};

use rshell_core::{
    DisplayRecoveryNotice, ExitStatus, InteractionId, InteractionRequest, InteractionResponse,
    RenderFrame, ResolvedTerminalProfile, SearchMatch, SearchQuery, SelectionRange, SessionFailure,
    SessionId, SessionState, TerminalInput, TerminalMouseEvent, TerminalSize,
};
use secrecy::SecretString;
use tokio::sync::{broadcast, mpsc, watch};

use crate::{
    DefaultTerminalEngine, EngineError, PresentationPolicy, SessionError, TerminalEngine,
    TransportFactory, TransportRequest,
};

pub const COMMAND_CAPACITY: usize = 128;
pub const EVENT_CAPACITY: usize = 64;

pub enum SessionCommand {
    Interrupt,
    ResetDisplay,
    Input(TerminalInput),
    Mouse(TerminalMouseEvent),
    Paste(SecretString),
    Resize(TerminalSize),
    Scroll(i32),
    Search(SearchQuery),
    Select(SelectionRange),
    CopySelection,
    ClearScrollback,
    Respond(InteractionId, InteractionResponse),
    Reconnect,
    Shutdown,
}

impl fmt::Debug for SessionCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupt => formatter.write_str("Interrupt"),
            Self::ResetDisplay => formatter.write_str("ResetDisplay"),
            Self::Input(input) => formatter.debug_tuple("Input").field(input).finish(),
            Self::Mouse(event) => formatter.debug_tuple("Mouse").field(event).finish(),
            Self::Paste(_) => formatter.write_str("Paste([REDACTED])"),
            Self::Resize(size) => formatter.debug_tuple("Resize").field(size).finish(),
            Self::Scroll(rows) => formatter.debug_tuple("Scroll").field(rows).finish(),
            Self::Search(query) => formatter.debug_tuple("Search").field(query).finish(),
            Self::Select(range) => formatter.debug_tuple("Select").field(range).finish(),
            Self::CopySelection => formatter.write_str("CopySelection"),
            Self::ClearScrollback => formatter.write_str("ClearScrollback"),
            Self::Respond(id, response) => formatter
                .debug_tuple("Respond")
                .field(id)
                .field(response)
                .finish(),
            Self::Reconnect => formatter.write_str("Reconnect"),
            Self::Shutdown => formatter.write_str("Shutdown"),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum SessionEvent {
    StateChanged(SessionState),
    /// One-time notification for the first frame of a session.
    /// All frame updates are delivered through `SessionClient::frames`.
    FrameReady(Arc<RenderFrame>),
    RecoveryChanged(Option<DisplayRecoveryNotice>),
    SearchCompleted(Vec<SearchMatch>),
    CopyReady(String),
    InteractionRequired(InteractionRequest),
    Exited(ExitStatus),
    Failed(SessionFailure),
    Crashed(String),
}

impl fmt::Debug for SessionEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateChanged(state) => {
                formatter.debug_tuple("StateChanged").field(state).finish()
            }
            Self::FrameReady(frame) => formatter.debug_tuple("FrameReady").field(frame).finish(),
            Self::RecoveryChanged(notice) => formatter
                .debug_tuple("RecoveryChanged")
                .field(notice)
                .finish(),
            Self::SearchCompleted(matches) => formatter
                .debug_tuple("SearchCompleted")
                .field(matches)
                .finish(),
            Self::CopyReady(_) => formatter.write_str("CopyReady([REDACTED])"),
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

pub struct SessionClient {
    pub id: SessionId,
    pub commands: mpsc::Sender<SessionCommand>,
    pub events: broadcast::Receiver<SessionEvent>,
    pub frames: watch::Receiver<Option<Arc<RenderFrame>>>,
}

impl SessionClient {
    pub fn try_command(&self, command: SessionCommand) -> Result<(), SessionError> {
        try_command(&self.commands, command)
    }
}

pub(crate) fn try_command(
    commands: &mpsc::Sender<SessionCommand>,
    command: SessionCommand,
) -> Result<(), SessionError> {
    commands.try_send(command).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => SessionError::Backpressure,
        mpsc::error::TrySendError::Closed(_) => SessionError::Closed,
    })
}

pub struct SessionLaunch {
    pub(crate) request: TransportRequest,
    pub(crate) engine: Box<dyn TerminalEngine>,
    pub(crate) factory: Option<Arc<dyn TransportFactory>>,
    pub(crate) presentation_policy: PresentationPolicy,
}

impl SessionLaunch {
    pub fn new(request: TransportRequest, engine: Box<dyn TerminalEngine>) -> Self {
        Self {
            request,
            engine,
            factory: None,
            presentation_policy: PresentationPolicy::default(),
        }
    }

    pub fn with_default_engine(
        request: TransportRequest,
        profile: &ResolvedTerminalProfile,
    ) -> Result<Self, EngineError> {
        let engine = DefaultTerminalEngine::new(profile, request.initial_size())?;
        Ok(
            Self::new(request, Box::new(engine)).with_presentation_policy(PresentationPolicy {
                scroll_on_output: profile.scroll_on_output,
                scroll_on_keypress: profile.scroll_on_keypress,
            }),
        )
    }

    pub fn request(&self) -> &TransportRequest {
        &self.request
    }

    pub fn with_factory(mut self, factory: Arc<dyn TransportFactory>) -> Self {
        self.factory = Some(factory);
        self
    }

    pub fn with_presentation_policy(mut self, policy: PresentationPolicy) -> Self {
        self.presentation_policy = policy;
        self
    }
}
