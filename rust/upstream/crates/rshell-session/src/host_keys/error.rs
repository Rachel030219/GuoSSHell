use std::fmt;

use rshell_core::SessionFailure;

/// A fail-closed result from host-key verification. It intentionally does not retain the key,
/// known-hosts content, OS error text, or a filesystem path.
pub enum HostKeyError {
    InvalidEndpoint,
    Changed {
        host: String,
        port: u16,
        line: usize,
    },
    Rejected {
        host: String,
        port: u16,
    },
    Timeout {
        host: String,
        port: u16,
    },
    Interaction {
        host: String,
        port: u16,
    },
    Verification {
        host: String,
        port: u16,
    },
    Storage {
        host: String,
        port: u16,
        step: HostKeyStorageStep,
    },
}

/// The non-sensitive persistence step that failed while learning a confirmed host key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyStorageStep {
    CreateParent,
    CreateTemporary,
    CopyExisting,
    SyncTemporary,
    Learn,
    SyncLearned,
    HardenTemporary,
    Replace,
}

impl HostKeyError {
    pub const fn failure(&self) -> SessionFailure {
        match self {
            Self::InvalidEndpoint => SessionFailure::Validation,
            Self::Changed { .. } => SessionFailure::HostKeyChanged,
            Self::Rejected { .. } => SessionFailure::HostKeyRejected,
            Self::Timeout { .. } => SessionFailure::Timeout,
            Self::Interaction { .. } | Self::Verification { .. } | Self::Storage { .. } => {
                SessionFailure::Platform
            }
        }
    }

    pub(super) fn changed(host: &str, port: u16, line: usize) -> Self {
        Self::Changed {
            host: host.to_owned(),
            port,
            line,
        }
    }

    pub(super) fn rejected(host: &str, port: u16) -> Self {
        Self::Rejected {
            host: host.to_owned(),
            port,
        }
    }

    pub(super) fn timeout(host: &str, port: u16) -> Self {
        Self::Timeout {
            host: host.to_owned(),
            port,
        }
    }

    pub(super) fn interaction(host: &str, port: u16) -> Self {
        Self::Interaction {
            host: host.to_owned(),
            port,
        }
    }

    pub(super) fn verification(host: &str, port: u16) -> Self {
        Self::Verification {
            host: host.to_owned(),
            port,
        }
    }

    pub(super) fn storage(host: &str, port: u16, step: HostKeyStorageStep) -> Self {
        Self::Storage {
            host: host.to_owned(),
            port,
            step,
        }
    }

    fn category(&self) -> &'static str {
        match self {
            Self::InvalidEndpoint => "InvalidEndpoint",
            Self::Changed { .. } => "Changed",
            Self::Rejected { .. } => "Rejected",
            Self::Timeout { .. } => "Timeout",
            Self::Interaction { .. } => "Interaction",
            Self::Verification { .. } => "Verification",
            Self::Storage { .. } => "Storage",
        }
    }

    fn endpoint(&self) -> Option<(&str, u16)> {
        match self {
            Self::InvalidEndpoint => None,
            Self::Changed { host, port, .. }
            | Self::Rejected { host, port }
            | Self::Timeout { host, port }
            | Self::Interaction { host, port }
            | Self::Verification { host, port }
            | Self::Storage { host, port, .. } => Some((host, *port)),
        }
    }
}

impl fmt::Debug for HostKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("HostKeyError");
        debug.field("category", &self.category());
        if let Some((host, port)) = self.endpoint() {
            debug.field("host", &host).field("port", &port);
        }
        if let Self::Changed { line, .. } = self {
            debug.field("line", line);
        }
        if let Self::Storage { step, .. } = self {
            debug.field("step", step);
        }
        debug.finish()
    }
}

impl fmt::Display for HostKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.endpoint() {
            Some((host, port)) => write!(
                formatter,
                "host-key verification failed for {host}:{port} ({})",
                self.category()
            ),
            None => write!(
                formatter,
                "host-key verification failed ({})",
                self.category()
            ),
        }
    }
}

impl std::error::Error for HostKeyError {}
