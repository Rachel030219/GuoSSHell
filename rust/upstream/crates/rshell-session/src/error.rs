use rshell_core::SessionFailure;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("terminal dimensions must be non-zero (got {cols}x{rows})")]
    InvalidSize { cols: u16, rows: u16 },
    #[error("terminal input is unsupported: {0}")]
    UnsupportedInput(&'static str),
    #[error("terminal mouse input is unsupported: {0}")]
    UnsupportedMouse(&'static str),
    #[error("terminal presentation generation is exhausted")]
    PresentationGenerationExhausted,
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("session command queue is full")]
    Backpressure,
    #[error("session is closed")]
    Closed,
    #[error("session does not exist")]
    UnknownSession,
    #[error("session manager requires a Tokio runtime")]
    RuntimeUnavailable,
    #[error("session supervisor could not be joined")]
    ActorJoin,
    #[error("session transport shutdown failed ({0:?})")]
    TransportShutdown(SessionFailure),
    #[error("session transport child remained active after shutdown")]
    ChildProcessAlive,
}

/// A classified transport failure with no secret-bearing diagnostic payload.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("transport operation failed ({failure:?})")]
pub struct TransportError {
    failure: SessionFailure,
}

impl TransportError {
    pub const fn new(failure: SessionFailure) -> Self {
        Self { failure }
    }

    pub const fn failure(self) -> SessionFailure {
        self.failure
    }
}
