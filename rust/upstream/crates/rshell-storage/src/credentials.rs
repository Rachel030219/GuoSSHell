use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use rshell_core::{CatalogMutation, ConnectionCatalog, CredentialRef, SecretUpdate};
use secrecy::SecretString;

use crate::{
    CredentialVault, SqliteRepository, StorageError, VaultError,
    command::{CredentialCommand, CredentialReply},
    credential_journal::{
        CredentialCommit, CredentialJournalRow, CredentialOperationAction, CredentialOperationId,
    },
    credential_mutation::{PreparedMutation, prepare_mutation},
};

pub use crate::credential_import::{CredentialImportBatch, CredentialImportItem};
pub(crate) use crate::credential_mutation::new_reference;
pub use crate::credential_types::{CrashPoint, CredentialOperationError, ReconcileReport};

pub struct CredentialCoordinator {
    pub(crate) repository: Arc<SqliteRepository>,
    pub(crate) vault: Arc<dyn CredentialVault>,
    operation_lock: Mutex<()>,
    crash: Mutex<Option<CrashPoint>>,
}

impl fmt::Debug for CredentialCoordinator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialCoordinator")
    }
}

impl CredentialCoordinator {
    pub fn new(repository: Arc<SqliteRepository>, vault: Arc<dyn CredentialVault>) -> Self {
        Self {
            repository,
            vault,
            operation_lock: Mutex::new(()),
            crash: Mutex::new(None),
        }
    }

    pub fn inject_crash_once(&self, point: CrashPoint) {
        *lock(&self.crash) = Some(point);
    }

    pub fn get(
        &self,
        credential_ref: &CredentialRef,
    ) -> Result<Option<SecretString>, CredentialOperationError> {
        self.vault
            .get(credential_ref)
            .map_err(|_| CredentialOperationError::Vault)
    }

    pub fn apply_catalog(
        &self,
        mutation: CatalogMutation,
        secret: SecretUpdate,
    ) -> Result<ConnectionCatalog, CredentialOperationError> {
        let _guard = lock(&self.operation_lock);
        let catalog = self.repository.load_catalog().map_err(storage)?;
        match prepare_mutation(&catalog, mutation, secret)? {
            PreparedMutation::NoPut(mutation) => {
                let commit = self.commit(CredentialCommand::ApplyNoPut(Box::new(mutation)))?;
                self.crash_at(CrashPoint::AfterCatalogCommitBeforeCleanup)?;
                self.finish_commit(commit)
            }
            PreparedMutation::Put {
                mutation,
                reference,
                secret,
            } => self.apply_put(mutation, reference, secret),
        }
    }

    pub fn reconcile(&self) -> Result<ReconcileReport, CredentialOperationError> {
        let _guard = lock(&self.operation_lock);
        let rows = self.rows()?;
        let catalog = self.repository.load_catalog().map_err(storage)?;
        let mut report = ReconcileReport::default();
        for row in rows {
            let referenced = catalog
                .connections
                .values()
                .any(|profile| profile.credential_ref.as_ref() == Some(&row.credential_ref));
            let vault_result = match row.action {
                CredentialOperationAction::PutNew if referenced => Ok(()),
                CredentialOperationAction::DeleteOld if referenced => Ok(()),
                CredentialOperationAction::PutNew | CredentialOperationAction::DeleteOld => {
                    self.vault.delete(&row.credential_ref)
                }
            };
            if vault_result.is_err() {
                report.failed += 1;
            } else {
                self.complete(row.operation_id)?;
                report.completed += 1;
            }
        }
        report.remaining = self.rows()?.len();
        Ok(report)
    }

    fn apply_put(
        &self,
        mutation: CatalogMutation,
        reference: CredentialRef,
        secret: SecretString,
    ) -> Result<ConnectionCatalog, CredentialOperationError> {
        let operation_id = match self.command(CredentialCommand::PreparePut(reference.clone()))? {
            CredentialReply::Operation(id) => id,
            _ => return Err(CredentialOperationError::Storage),
        };
        self.crash_at(CrashPoint::AfterPrepare)?;
        self.vault.put(&reference, &secret).map_err(vault)?;
        self.crash_at(CrashPoint::AfterVaultPutBeforeState)?;
        self.mark_applied(operation_id)?;
        self.crash_at(CrashPoint::AfterVaultApplied)?;
        let commit = self.commit_reconciliation(CredentialCommand::FinalizePut {
            operation_id,
            mutation: Box::new(mutation),
        })?;
        self.crash_at(CrashPoint::AfterCatalogCommitBeforeCleanup)?;
        self.finish_commit(commit)
    }

    pub(crate) fn finish_commit(
        &self,
        commit: CredentialCommit,
    ) -> Result<ConnectionCatalog, CredentialOperationError> {
        for row in commit.pending_deletes {
            if self.vault.delete(&row.credential_ref).is_ok() {
                let _ = self.complete(row.operation_id);
            }
        }
        Ok(commit.catalog)
    }

    pub(crate) fn command(
        &self,
        command: CredentialCommand,
    ) -> Result<CredentialReply, CredentialOperationError> {
        self.repository
            .credential_operation(command)
            .map_err(storage)
    }

    pub(crate) fn commit(
        &self,
        command: CredentialCommand,
    ) -> Result<CredentialCommit, CredentialOperationError> {
        match self.command(command)? {
            CredentialReply::Commit(commit) => Ok(commit),
            _ => Err(CredentialOperationError::Storage),
        }
    }

    pub(crate) fn commit_reconciliation(
        &self,
        command: CredentialCommand,
    ) -> Result<CredentialCommit, CredentialOperationError> {
        self.commit(command)
            .map_err(|_| CredentialOperationError::ReconciliationRequired)
    }

    pub(crate) fn mark_applied(
        &self,
        operation_id: CredentialOperationId,
    ) -> Result<(), CredentialOperationError> {
        match self.command(CredentialCommand::MarkApplied(operation_id)) {
            Ok(CredentialReply::Complete) => Ok(()),
            _ => Err(CredentialOperationError::ReconciliationRequired),
        }
    }

    pub(crate) fn import_marker_exists(
        &self,
        marker: &str,
    ) -> Result<bool, CredentialOperationError> {
        match self.command(CredentialCommand::ImportMarkerExists(marker.into()))? {
            CredentialReply::MarkerExists(exists) => Ok(exists),
            _ => Err(CredentialOperationError::Storage),
        }
    }

    pub(crate) fn crash_at(&self, point: CrashPoint) -> Result<(), CredentialOperationError> {
        let mut crash = lock(&self.crash);
        if *crash == Some(point) {
            *crash = None;
            Err(CredentialOperationError::InjectedCrash(point))
        } else {
            Ok(())
        }
    }

    pub(crate) fn operation_guard(&self) -> MutexGuard<'_, ()> {
        lock(&self.operation_lock)
    }

    fn rows(&self) -> Result<Vec<CredentialJournalRow>, CredentialOperationError> {
        match self.command(CredentialCommand::List)? {
            CredentialReply::Rows(rows) => Ok(rows),
            _ => Err(CredentialOperationError::Storage),
        }
    }

    fn complete(&self, id: CredentialOperationId) -> Result<(), CredentialOperationError> {
        match self.command(CredentialCommand::Complete(id))? {
            CredentialReply::Complete => Ok(()),
            _ => Err(CredentialOperationError::Storage),
        }
    }
}

fn storage(_: StorageError) -> CredentialOperationError {
    CredentialOperationError::Storage
}
fn vault(_: VaultError) -> CredentialOperationError {
    CredentialOperationError::Vault
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
