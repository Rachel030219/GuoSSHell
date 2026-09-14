use std::fmt;

use rshell_core::{
    CatalogMutation, ConnectionCatalog, ConnectionGroup, ConnectionProfile, CredentialRef,
};
use secrecy::SecretString;

use crate::{
    command::{CredentialCommand, CredentialReply},
    credentials::{CrashPoint, CredentialCoordinator, CredentialOperationError, new_reference},
};

pub struct CredentialImportItem {
    profile: ConnectionProfile,
    secret: Option<SecretString>,
}

impl CredentialImportItem {
    pub fn new(profile: ConnectionProfile, secret: Option<SecretString>) -> Self {
        Self { profile, secret }
    }
}

impl fmt::Debug for CredentialImportItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialImportItem")
            .field("profile_id", &self.profile.id)
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

pub struct CredentialImportBatch {
    groups: Vec<ConnectionGroup>,
    items: Vec<CredentialImportItem>,
    import_marker: Option<String>,
}

impl CredentialImportBatch {
    pub fn new(groups: Vec<ConnectionGroup>, items: Vec<CredentialImportItem>) -> Self {
        Self {
            groups,
            items,
            import_marker: None,
        }
    }

    pub(crate) fn with_import_marker(mut self, key: String) -> Self {
        self.import_marker = Some(key);
        self
    }
}

impl fmt::Debug for CredentialImportBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialImportBatch")
            .field("groups", &self.groups.len())
            .field("profiles", &self.items.len())
            .field(
                "import_marker",
                &self.import_marker.as_ref().map(|_| "[PRESENT]"),
            )
            .field(
                "secrets",
                &self
                    .items
                    .iter()
                    .filter(|item| item.secret.is_some())
                    .count(),
            )
            .finish()
    }
}

impl CredentialCoordinator {
    pub fn commit_import(
        &self,
        batch: CredentialImportBatch,
    ) -> Result<ConnectionCatalog, CredentialOperationError> {
        let _guard = self.operation_guard();
        let existing = self
            .repository
            .load_catalog()
            .map_err(|_| CredentialOperationError::Storage)?;
        let import_marker = batch.import_marker.clone();
        if let Some(marker) = import_marker.as_deref()
            && self.import_marker_exists(marker)?
        {
            return Err(CredentialOperationError::AlreadyImported);
        }
        let (groups, profiles, secrets) = prepare_batch(existing, batch)?;
        let references = secrets
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect();
        let operation_ids = match self.command(CredentialCommand::PrepareImport(references))? {
            CredentialReply::Operations(ids) => ids,
            _ => return Err(CredentialOperationError::Storage),
        };
        self.crash_at(CrashPoint::AfterPrepare)?;
        if operation_ids.len() != secrets.len() {
            return Err(CredentialOperationError::Storage);
        }
        for (operation_id, (reference, secret)) in operation_ids.iter().zip(&secrets) {
            self.vault
                .put(reference, secret)
                .map_err(|_| CredentialOperationError::Vault)?;
            self.crash_at(CrashPoint::AfterVaultPutBeforeState)?;
            self.mark_applied(*operation_id)?;
            self.crash_at(CrashPoint::AfterVaultApplied)?;
        }
        let commit = self.commit_reconciliation(CredentialCommand::FinalizeImport {
            operation_ids,
            groups,
            profiles,
            import_marker,
        })?;
        self.crash_at(CrashPoint::AfterCatalogCommitBeforeCleanup)?;
        self.finish_commit(commit)
    }
}

type PreparedImport = (
    Vec<ConnectionGroup>,
    Vec<ConnectionProfile>,
    Vec<(CredentialRef, SecretString)>,
);

fn prepare_batch(
    existing: ConnectionCatalog,
    batch: CredentialImportBatch,
) -> Result<PreparedImport, CredentialOperationError> {
    let mut catalog = existing.clone();
    let mut group_ids = existing
        .groups
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    for group in &batch.groups {
        if !group_ids.insert(group.id) {
            return Err(CredentialOperationError::Conflict);
        }
    }
    let mut profile_ids = existing
        .connections
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    for item in &batch.items {
        if !profile_ids.insert(item.profile.id) {
            return Err(CredentialOperationError::Conflict);
        }
    }
    let groups = order_groups(&existing, batch.groups)?;
    for group in &groups {
        catalog
            .apply(CatalogMutation::CreateGroup(group.clone()))
            .map_err(|_| CredentialOperationError::Validation)?;
    }
    let mut profiles = Vec::with_capacity(batch.items.len());
    let mut secrets = Vec::new();
    for item in batch.items {
        let mut profile = item.profile;
        match item.secret {
            Some(secret) => {
                let reference = new_reference();
                profile.credential_ref = Some(reference.clone());
                secrets.push((reference, secret));
            }
            None if profile.credential_ref.as_ref().is_some_and(|reference| {
                !existing
                    .connections
                    .values()
                    .any(|current| current.credential_ref.as_ref() == Some(reference))
            }) =>
            {
                return Err(CredentialOperationError::Validation);
            }
            None => {}
        }
        catalog
            .apply(CatalogMutation::Create(profile.clone()))
            .map_err(|_| CredentialOperationError::Validation)?;
        profiles.push(profile);
    }
    Ok((groups, profiles, secrets))
}

fn order_groups(
    existing: &ConnectionCatalog,
    mut groups: Vec<ConnectionGroup>,
) -> Result<Vec<ConnectionGroup>, CredentialOperationError> {
    let mut known = existing
        .groups
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(groups.len());
    while !groups.is_empty() {
        let Some(index) = groups
            .iter()
            .position(|group| group.parent_id.is_none_or(|parent| known.contains(&parent)))
        else {
            return Err(CredentialOperationError::Conflict);
        };
        let group = groups.remove(index);
        if !known.insert(group.id) {
            return Err(CredentialOperationError::Validation);
        }
        ordered.push(group);
    }
    Ok(ordered)
}
