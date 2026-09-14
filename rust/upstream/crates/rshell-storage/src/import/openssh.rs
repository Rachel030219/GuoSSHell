mod lexer;
mod parser;
mod resolve;

use std::{collections::BTreeSet, path::Path};

use rshell_core::{ConnectionId, ConnectionProfile};

use crate::{CredentialCoordinator, CredentialImportBatch, CredentialImportItem};

use super::{ImportError, ImportReport, ImportWarning, map_credential_error};

#[derive(Debug, Clone, PartialEq)]
pub struct OpenSshCandidate {
    pub id: ConnectionId,
    pub host_pattern: String,
    pub host_name: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<std::path::PathBuf>,
    pub proxy_jump: Option<String>,
    pub importable: bool,
    pub profile: ConnectionProfile,
    pub warnings: Vec<ImportWarning>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenSshPreview {
    pub candidates: Vec<OpenSshCandidate>,
    pub warnings: Vec<ImportWarning>,
}

#[derive(Debug, Clone, Copy)]
pub struct OpenSshConfigImporter;

impl OpenSshConfigImporter {
    pub const fn new() -> Self {
        Self
    }

    pub fn preview(&self, path: impl AsRef<Path>) -> Result<OpenSshPreview, ImportError> {
        let config = parser::parse(path.as_ref())?;
        resolve::preview(config)
    }

    pub fn commit(
        &self,
        coordinator: &CredentialCoordinator,
        preview: OpenSshPreview,
        selected: &BTreeSet<ConnectionId>,
    ) -> Result<ImportReport, ImportError> {
        if selected.is_empty() {
            return Ok(ImportReport {
                skipped_connections: preview.candidates.len(),
                ..ImportReport::default()
            });
        }
        let available = preview
            .candidates
            .iter()
            .map(|candidate| candidate.id)
            .collect::<BTreeSet<_>>();
        if !selected.is_subset(&available) {
            return Err(ImportError::InvalidSelection);
        }
        let chosen = preview
            .candidates
            .into_iter()
            .filter(|candidate| selected.contains(&candidate.id))
            .collect::<Vec<_>>();
        if chosen.iter().any(|candidate| !candidate.importable)
            || has_invalid_ids(&chosen)
            || has_duplicate_names(&chosen)
        {
            return Err(ImportError::InvalidSelection);
        }
        let report = ImportReport {
            imported_connections: chosen.len(),
            skipped_connections: available.len() - chosen.len(),
            ..ImportReport::default()
        };
        let items = chosen
            .into_iter()
            .map(|candidate| CredentialImportItem::new(candidate.profile, None))
            .collect();
        coordinator
            .commit_import(CredentialImportBatch::new(Vec::new(), items))
            .map_err(map_credential_error)?;
        Ok(report)
    }
}

fn has_invalid_ids(candidates: &[OpenSshCandidate]) -> bool {
    if candidates
        .iter()
        .any(|candidate| candidate.id != candidate.profile.id)
    {
        return true;
    }
    let candidate_ids = candidates
        .iter()
        .map(|candidate| candidate.id)
        .collect::<BTreeSet<_>>();
    let profile_ids = candidates
        .iter()
        .map(|candidate| candidate.profile.id)
        .collect::<BTreeSet<_>>();
    candidate_ids.len() != candidates.len() || profile_ids.len() != candidates.len()
}

fn has_duplicate_names(candidates: &[OpenSshCandidate]) -> bool {
    candidates
        .iter()
        .map(|candidate| candidate.profile.name.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        != candidates.len()
}

impl Default for OpenSshConfigImporter {
    fn default() -> Self {
        Self::new()
    }
}
