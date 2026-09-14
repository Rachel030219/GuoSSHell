use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::connection::{AuthenticationKind, ConnectionGroup};

macro_rules! uuid_newtype {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
    };
}

uuid_newtype!(ImportPreviewId);
uuid_newtype!(ImportCandidateId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSourceKind {
    LegacyRshellJson,
    OpenSshConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPreviewView {
    pub id: ImportPreviewId,
    pub source: ImportSourceKind,
    pub groups: Vec<ConnectionGroup>,
    pub candidates: Vec<ImportCandidateView>,
    pub warnings: Vec<ImportWarningView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportCandidateView {
    pub id: ImportCandidateId,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub source_label: String,
    pub has_secret: bool,
    pub selectable: bool,
    pub authentication: AuthenticationKind,
    pub credential_reference_present: bool,
    pub terminal_override_present: bool,
    pub importable: bool,
    pub wildcard: bool,
    pub warnings: Vec<ImportWarningView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportWarningView {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReportView {
    pub imported_groups: usize,
    pub imported_connections: usize,
    pub skipped_candidates: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppFailureCategory {
    Validation,
    Storage,
    Vault,
    HostKey,
    Authentication,
    Network,
    Pty,
    Subprocess,
    Platform,
    Backpressure,
    Crashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    None,
    Retry,
    EditConnection(crate::ConnectionId),
}

/// A stable, redacted application failure. Context is static by construction, so lower-level
/// errors, paths, connection values, and credentials cannot accidentally cross the UI boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppFailure {
    pub category: AppFailureCategory,
    pub context: &'static str,
    pub retryable: bool,
    pub action: RecoveryAction,
}

impl AppFailure {
    pub const fn retryable(
        category: AppFailureCategory,
        context: &'static str,
        action: RecoveryAction,
    ) -> Self {
        Self {
            category,
            context,
            retryable: true,
            action,
        }
    }

    pub const fn fatal(category: AppFailureCategory, context: &'static str) -> Self {
        Self {
            category,
            context,
            retryable: false,
            action: RecoveryAction::None,
        }
    }
}
