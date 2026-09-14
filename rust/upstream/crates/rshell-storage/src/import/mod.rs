mod legacy;
mod legacy_mapping;
mod legacy_terminal;
mod openssh;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use rshell_core::{ConnectionGroup, ConnectionId, ConnectionProfile};
use secrecy::SecretString;

use crate::{
    CredentialCoordinator, CredentialImportBatch, CredentialImportItem, CredentialOperationError,
};

pub use legacy::LegacyJsonImporter;
pub use openssh::{OpenSshCandidate, OpenSshConfigImporter, OpenSshPreview};

const LEGACY_FINGERPRINT_PREFIX: &str = "import.legacy.sha256:";

#[derive(Debug, Clone, PartialEq)]
pub struct ImportConnectionCandidate {
    pub id: ConnectionId,
    pub profile: ConnectionProfile,
    pub has_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportWarning {
    RecoveredFromBackup,
    HostKeyPolicyUpgraded,
    KittyGraphicsDisabled,
    DependsOnOpenSshConfig,
    MultipleIdentityFiles,
    UnsupportedDirective { directive: String },
    DynamicValue { directive: String, value: String },
    InvalidHost { host: String },
    InvalidPort { value: String },
}

pub struct ImportPreview {
    pub groups: Vec<ConnectionGroup>,
    pub connections: Vec<ImportConnectionCandidate>,
    pub warnings: Vec<ImportWarning>,
    fingerprint_key: String,
    secrets: BTreeMap<ConnectionId, SecretString>,
}

impl fmt::Debug for ImportPreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportPreview")
            .field("groups", &self.groups)
            .field("connections", &self.connections)
            .field("warnings", &self.warnings)
            .field("source_fingerprint", &"[REDACTED]")
            .field("secret_count", &self.secrets.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported_groups: usize,
    pub imported_connections: usize,
    pub skipped_connections: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    Io,
    InvalidJson,
    NoUsableSource,
    InvalidUuid,
    InvalidPort,
    InvalidConnection,
    InvalidSelection,
    IdConflict,
    AlreadyImported,
    IncludeCycle,
    IncludeDepth,
    Credential(CredentialOperationError),
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io => "legacy import I/O failed",
            Self::InvalidJson => "legacy import JSON is invalid",
            Self::NoUsableSource => "legacy import source and backup are invalid",
            Self::InvalidUuid => "legacy import contains an invalid identifier",
            Self::InvalidPort => "legacy import contains an invalid port",
            Self::InvalidConnection => "legacy import contains an invalid connection",
            Self::InvalidSelection => "legacy import selection is invalid",
            Self::IdConflict => "legacy import conflicts with existing identifiers",
            Self::AlreadyImported => "legacy import source was already imported",
            Self::IncludeCycle => "OpenSSH config include cycle detected",
            Self::IncludeDepth => "OpenSSH config include depth exceeded",
            Self::Credential(_) => "legacy import credential operation failed",
        })
    }
}

impl std::error::Error for ImportError {}

impl ImportPreview {
    pub(crate) fn new(
        groups: Vec<ConnectionGroup>,
        connections: Vec<ImportConnectionCandidate>,
        warnings: Vec<ImportWarning>,
        digest: String,
        secrets: BTreeMap<ConnectionId, SecretString>,
    ) -> Self {
        Self {
            groups,
            connections,
            warnings,
            fingerprint_key: format!("{LEGACY_FINGERPRINT_PREFIX}{digest}"),
            secrets,
        }
    }

    fn selected_batch(
        mut self,
        selected: &BTreeSet<ConnectionId>,
    ) -> Result<(CredentialImportBatch, ImportReport), ImportError> {
        let available = self
            .connections
            .iter()
            .map(|candidate| candidate.id)
            .collect::<BTreeSet<_>>();
        if !selected.is_subset(&available) {
            return Err(ImportError::InvalidSelection);
        }
        let group_ids = self
            .connections
            .iter()
            .filter(|candidate| selected.contains(&candidate.id))
            .filter_map(|candidate| candidate.profile.group_id)
            .collect::<BTreeSet<_>>();
        let groups = self
            .groups
            .into_iter()
            .filter(|group| group_ids.contains(&group.id))
            .collect::<Vec<_>>();
        let items = self
            .connections
            .into_iter()
            .filter(|candidate| selected.contains(&candidate.id))
            .map(|candidate| {
                let secret = self.secrets.remove(&candidate.id);
                CredentialImportItem::new(candidate.profile, secret)
            })
            .collect::<Vec<_>>();
        let report = ImportReport {
            imported_groups: groups.len(),
            imported_connections: items.len(),
            skipped_connections: available.len() - items.len(),
        };
        Ok((
            CredentialImportBatch::new(groups, items).with_import_marker(self.fingerprint_key),
            report,
        ))
    }
}

impl LegacyJsonImporter {
    pub fn commit(
        &self,
        coordinator: &CredentialCoordinator,
        preview: ImportPreview,
        selected: &BTreeSet<ConnectionId>,
    ) -> Result<ImportReport, ImportError> {
        if selected.is_empty() {
            return Ok(ImportReport {
                skipped_connections: preview.connections.len(),
                ..ImportReport::default()
            });
        }
        let (batch, report) = preview.selected_batch(selected)?;
        coordinator
            .commit_import(batch)
            .map_err(map_credential_error)?;
        Ok(report)
    }
}

fn map_credential_error(error: CredentialOperationError) -> ImportError {
    match error {
        CredentialOperationError::Conflict => ImportError::IdConflict,
        CredentialOperationError::AlreadyImported => ImportError::AlreadyImported,
        other => ImportError::Credential(other),
    }
}
