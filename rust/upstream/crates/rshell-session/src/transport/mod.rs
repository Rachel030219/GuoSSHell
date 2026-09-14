use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rshell_core::{
    ExitStatus, InteractionId, InteractionRequest, InteractionResponse, SessionFailure,
    SessionState, TerminalSize,
};
use tokio::sync::{mpsc, oneshot};

use crate::TransportError;

mod local;
mod local_reader;
mod local_runtime;
#[cfg(test)]
mod local_runtime_tests;
mod native_ssh;
mod pty;
mod system_ssh;

pub use local::{LocalLaunch, LocalPtyFactory, LocalPtyTransport};
pub use native_ssh::NativeSshTransport;
pub use system_ssh::{SystemOpenSshTransport, build_system_ssh_argv};

const INTERACTION_CAPACITY: usize = 16;
const DEFAULT_TERMINAL_TYPE: &str = "xterm-256color";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransportCapabilities {
    pub agent: bool,
    pub public_key: bool,
    pub managed_password: bool,
    pub keyboard_interactive: bool,
    pub host_key_prompt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportRequest {
    initial_size: TerminalSize,
    terminal_type: Cow<'static, str>,
}

impl TransportRequest {
    pub const fn new(initial_size: TerminalSize) -> Self {
        Self {
            initial_size,
            terminal_type: Cow::Borrowed(DEFAULT_TERMINAL_TYPE),
        }
    }

    pub const fn initial_size(&self) -> TerminalSize {
        self.initial_size
    }

    pub fn terminal_type(&self) -> &str {
        &self.terminal_type
    }

    pub fn with_terminal_type(
        mut self,
        terminal_type: impl Into<String>,
    ) -> Result<Self, TransportError> {
        let terminal_type = terminal_type.into();
        if terminal_type.is_empty() || terminal_type.contains('\0') {
            return Err(TransportError::new(SessionFailure::Validation));
        }
        self.terminal_type = Cow::Owned(terminal_type);
        Ok(self)
    }
}

pub enum TransportEvent {
    Connected,
    AwaitingHostKey,
    AwaitingAuthentication,
    Output(Vec<u8>),
    Eof,
    Exit(ExitStatus),
    Failure(TransportError),
}

impl fmt::Debug for TransportEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connected => formatter.write_str("Connected"),
            Self::AwaitingHostKey => formatter.write_str("AwaitingHostKey"),
            Self::AwaitingAuthentication => formatter.write_str("AwaitingAuthentication"),
            Self::Output(bytes) => formatter
                .debug_tuple("Output")
                .field(&format_args!("[{} bytes]", bytes.len()))
                .finish(),
            Self::Eof => formatter.write_str("Eof"),
            Self::Exit(status) => formatter.debug_tuple("Exit").field(status).finish(),
            Self::Failure(error) => formatter.debug_tuple("Failure").field(error).finish(),
        }
    }
}

#[async_trait]
pub trait SessionTransport: Send {
    fn capabilities(&self) -> TransportCapabilities;

    fn child_process_id(&self) -> Option<u32> {
        None
    }

    async fn connect(
        &mut self,
        request: &TransportRequest,
        interactions: InteractionBroker,
    ) -> Result<(), TransportError>;

    async fn next_event(&mut self) -> Result<TransportEvent, TransportError>;
    async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError>;
    async fn resize(&mut self, size: TerminalSize) -> Result<(), TransportError>;
    async fn shutdown(&mut self) -> Result<(), TransportError>;
}

pub trait TransportFactory: Send + Sync {
    fn create(
        &self,
        request: &TransportRequest,
    ) -> Result<Box<dyn SessionTransport>, TransportError>;
}

type InteractionMessage = (InteractionId, InteractionRequest);
type PendingResponses = BTreeMap<InteractionId, oneshot::Sender<InteractionResponse>>;

struct PendingRequest {
    id: InteractionId,
    pending: Arc<Mutex<PendingResponses>>,
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.id);
    }
}

#[derive(Clone)]
pub struct InteractionBroker {
    request_tx: mpsc::Sender<InteractionMessage>,
    pending: Arc<Mutex<PendingResponses>>,
}

impl fmt::Debug for InteractionBroker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InteractionBroker([REDACTED])")
    }
}

impl InteractionBroker {
    pub async fn request(
        &self,
        request: InteractionRequest,
    ) -> Result<InteractionResponse, TransportError> {
        let id = interaction_id(&request);
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if pending.contains_key(&id) {
                return Err(TransportError::new(SessionFailure::Validation));
            }
            pending.insert(id, response_tx);
        }
        let _pending_request = PendingRequest {
            id,
            pending: Arc::clone(&self.pending),
        };
        if self.request_tx.send((id, request)).await.is_err() {
            return Err(TransportError::new(SessionFailure::Crashed));
        }
        match response_rx.await {
            Ok(response) => Ok(response),
            Err(_) => Err(TransportError::new(SessionFailure::Crashed)),
        }
    }

    pub fn respond(
        &self,
        id: InteractionId,
        response: InteractionResponse,
    ) -> Result<(), TransportError> {
        let sender = self
            .remove(id)
            .ok_or_else(|| TransportError::new(SessionFailure::Validation))?;
        sender
            .send(response)
            .map_err(|_| TransportError::new(SessionFailure::Crashed))
    }

    fn remove(&self, id: InteractionId) -> Option<oneshot::Sender<InteractionResponse>> {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id)
    }
}

pub fn interaction_channel() -> (InteractionBroker, mpsc::Receiver<InteractionMessage>) {
    let (request_tx, request_rx) = mpsc::channel(INTERACTION_CAPACITY);
    (
        InteractionBroker {
            request_tx,
            pending: Arc::new(Mutex::new(BTreeMap::new())),
        },
        request_rx,
    )
}

pub(crate) fn interaction_state(request: &InteractionRequest) -> SessionState {
    match request {
        InteractionRequest::HostKey(_) => SessionState::AwaitingHostKey,
        InteractionRequest::Password(_)
        | InteractionRequest::PrivateKeyPassphrase(_)
        | InteractionRequest::KeyboardInteractive(_) => SessionState::AwaitingAuthentication,
    }
}

fn interaction_id(request: &InteractionRequest) -> InteractionId {
    match request {
        InteractionRequest::HostKey(prompt) => prompt.id,
        InteractionRequest::Password(prompt) | InteractionRequest::PrivateKeyPassphrase(prompt) => {
            prompt.id
        }
        InteractionRequest::KeyboardInteractive(prompt) => prompt.id,
    }
}
