use std::fmt;

use rshell_core::{AuthenticationKind, TransportKind};
use rshell_storage::VaultError;

/// A categorized plan-construction failure without credential references, filesystem paths, or
/// vault-provided data.
pub enum AuthPlanError {
    UnsupportedCombination {
        host: String,
        transport: TransportKind,
        authentication: AuthenticationKind,
    },
    MissingCredentialRef {
        host: String,
        authentication: AuthenticationKind,
    },
    CredentialMissing {
        host: String,
        authentication: AuthenticationKind,
    },
    CredentialFault {
        host: String,
        authentication: AuthenticationKind,
        vault: VaultError,
    },
    MissingIdentityFile {
        host: String,
        authentication: AuthenticationKind,
    },
}

impl AuthPlanError {
    fn host(&self) -> &str {
        match self {
            Self::UnsupportedCombination { host, .. }
            | Self::MissingCredentialRef { host, .. }
            | Self::CredentialMissing { host, .. }
            | Self::CredentialFault { host, .. }
            | Self::MissingIdentityFile { host, .. } => host,
        }
    }

    fn kind(&self) -> AuthenticationKind {
        match self {
            Self::UnsupportedCombination { authentication, .. }
            | Self::MissingCredentialRef { authentication, .. }
            | Self::CredentialMissing { authentication, .. }
            | Self::CredentialFault { authentication, .. }
            | Self::MissingIdentityFile { authentication, .. } => *authentication,
        }
    }

    fn category(&self) -> &'static str {
        match self {
            Self::UnsupportedCombination { transport, .. } => match transport {
                TransportKind::SystemOpenSsh => "UnsupportedSystemOpenSsh",
                TransportKind::NativeSsh => "UnsupportedNativeSsh",
            },
            Self::MissingCredentialRef { .. } => "MissingCredentialRef",
            Self::CredentialMissing { .. } => "CredentialMissing",
            Self::CredentialFault { vault, .. } => match vault {
                VaultError::Unavailable => "VaultUnavailable",
                VaultError::NoEntry => "VaultNoEntry",
                VaultError::Denied => "VaultDenied",
                VaultError::Platform => "VaultPlatform",
            },
            Self::MissingIdentityFile { .. } => "MissingIdentityFile",
        }
    }
}

impl fmt::Debug for AuthPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthPlanError")
            .field("kind", &self.kind())
            .field("host", &self.host())
            .field("category", &self.category())
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for AuthPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "authentication planning failed for {} ({:?}, {}; [REDACTED])",
            self.host(),
            self.kind(),
            self.category()
        )
    }
}

impl std::error::Error for AuthPlanError {}
