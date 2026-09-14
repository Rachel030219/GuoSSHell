use std::sync::Arc;

use async_trait::async_trait;
use rshell_core::{
    CatalogMutation, ConnectionCatalog, CredentialOperationError, CredentialPort, CredentialRef,
    RepositoryError, SecretUpdate, VaultFailure,
};
use secrecy::SecretString;

use crate::{
    CredentialCoordinator, CredentialOperationError as StorageCredentialError, VaultError,
};

#[derive(Clone)]
pub struct CredentialPortAdapter {
    coordinator: Arc<CredentialCoordinator>,
}

impl CredentialPortAdapter {
    pub fn new(coordinator: Arc<CredentialCoordinator>) -> Self {
        Self { coordinator }
    }

    pub fn coordinator(&self) -> &Arc<CredentialCoordinator> {
        &self.coordinator
    }
}

#[async_trait]
impl CredentialPort for CredentialPortAdapter {
    async fn apply_catalog(
        &self,
        mutation: CatalogMutation,
        secret: SecretUpdate,
    ) -> Result<ConnectionCatalog, CredentialOperationError> {
        let coordinator = Arc::clone(&self.coordinator);
        tokio::task::spawn_blocking(move || coordinator.apply_catalog(mutation, secret))
            .await
            .map_err(|_| unavailable())?
            .map_err(map_credential)
    }

    async fn get(
        &self,
        key: &CredentialRef,
    ) -> Result<Option<SecretString>, CredentialOperationError> {
        let vault = Arc::clone(&self.coordinator.vault);
        let key = key.clone();
        tokio::task::spawn_blocking(move || vault.get(&key))
            .await
            .map_err(|_| unavailable())?
            .map_err(|error| CredentialOperationError::Vault(map_vault(error)))
    }
}

pub(super) fn map_credential(error: StorageCredentialError) -> CredentialOperationError {
    match error {
        StorageCredentialError::Vault => CredentialOperationError::Vault(VaultFailure::Unavailable),
        StorageCredentialError::ReconciliationRequired
        | StorageCredentialError::InjectedCrash(_) => {
            CredentialOperationError::ReconciliationRequired
        }
        StorageCredentialError::Validation
        | StorageCredentialError::Conflict
        | StorageCredentialError::AlreadyImported => CredentialOperationError::Repository(
            RepositoryError::Constraint("credential mutation rejected".into()),
        ),
        StorageCredentialError::Storage => unavailable(),
    }
}

fn unavailable() -> CredentialOperationError {
    CredentialOperationError::Repository(RepositoryError::Unavailable)
}

fn map_vault(error: VaultError) -> VaultFailure {
    match error {
        VaultError::Unavailable => VaultFailure::Unavailable,
        VaultError::NoEntry => VaultFailure::NoEntry,
        VaultError::Denied => VaultFailure::Denied,
        VaultError::Platform => VaultFailure::Platform,
    }
}
