use std::sync::Arc;

use async_trait::async_trait;
use rshell_core::{
    AppSettings, CatalogMutation, ConnectionCatalog, ConnectionRepository, RepositoryError,
    TerminalProfile,
};

use crate::{SqliteRepository, StorageError};

#[derive(Clone)]
pub struct RepositoryPortAdapter {
    repository: Arc<SqliteRepository>,
}

impl RepositoryPortAdapter {
    pub fn new(repository: Arc<SqliteRepository>) -> Self {
        Self { repository }
    }

    pub fn repository(&self) -> &Arc<SqliteRepository> {
        &self.repository
    }
}

#[async_trait]
impl ConnectionRepository for RepositoryPortAdapter {
    async fn load_catalog(&self) -> Result<ConnectionCatalog, RepositoryError> {
        let repository = Arc::clone(&self.repository);
        blocking(move || repository.load_catalog()).await
    }

    async fn apply(&self, mutation: CatalogMutation) -> Result<ConnectionCatalog, RepositoryError> {
        let repository = Arc::clone(&self.repository);
        blocking(move || {
            repository.apply(mutation)?;
            repository.load_catalog()
        })
        .await
    }

    async fn load_terminal_profiles(&self) -> Result<Vec<TerminalProfile>, RepositoryError> {
        let repository = Arc::clone(&self.repository);
        blocking(move || repository.load_terminal_profiles()).await
    }

    async fn save_terminal_profile(&self, profile: TerminalProfile) -> Result<(), RepositoryError> {
        let repository = Arc::clone(&self.repository);
        blocking(move || repository.save_terminal_profile(profile)).await
    }

    async fn load_settings(&self) -> Result<AppSettings, RepositoryError> {
        let repository = Arc::clone(&self.repository);
        blocking(move || repository.load_settings()).await
    }

    async fn save_settings(&self, settings: AppSettings) -> Result<(), RepositoryError> {
        let repository = Arc::clone(&self.repository);
        blocking(move || repository.save_settings(settings)).await
    }
}

pub(super) async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
) -> Result<T, RepositoryError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| RepositoryError::Unavailable)?
        .map_err(map_storage)
}

pub(super) fn map_storage(error: StorageError) -> RepositoryError {
    match error {
        StorageError::Busy => RepositoryError::Busy,
        StorageError::Constraint => RepositoryError::Constraint("constraint failed".into()),
        StorageError::Corrupt | StorageError::Serialization => RepositoryError::Corrupt,
        StorageError::Migration
        | StorageError::Io
        | StorageError::Crashed
        | StorageError::QueueClosed => RepositoryError::Unavailable,
    }
}
