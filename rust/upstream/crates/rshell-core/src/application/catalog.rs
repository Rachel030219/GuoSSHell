use crate::{
    AppEvent, AppFailure, AppFailureCategory, CatalogMutation, CredentialOperationError,
    RecoveryAction, RepositoryError, SecretUpdate, VaultFailure,
};

use super::runtime::CommandLoop;

impl CommandLoop {
    pub(super) async fn apply_catalog(&mut self, mutation: CatalogMutation, secret: SecretUpdate) {
        match self
            .dependencies
            .credentials
            .apply_catalog(mutation, secret)
            .await
        {
            Ok(catalog) => {
                self.view_model.catalog = catalog.clone();
                self.publish_view();
                self.emit(AppEvent::CatalogChanged(catalog)).await;
            }
            Err(error) => self.fail(credential_failure(error)).await,
        }
    }

    pub(super) async fn search_connections(&self, query: &str) {
        self.emit(AppEvent::SearchResults(
            self.view_model.catalog.search(query),
        ))
        .await;
    }
}

pub(super) fn repository_failure(error: RepositoryError) -> AppFailure {
    let category = match error {
        RepositoryError::Busy => AppFailureCategory::Backpressure,
        RepositoryError::Unavailable
        | RepositoryError::Constraint(_)
        | RepositoryError::Corrupt => AppFailureCategory::Storage,
    };
    AppFailure::retryable(category, "storage operation failed", RecoveryAction::Retry)
}

pub(super) fn credential_failure(error: CredentialOperationError) -> AppFailure {
    match error {
        CredentialOperationError::Vault(failure) => {
            let category = match failure {
                VaultFailure::Unavailable
                | VaultFailure::NoEntry
                | VaultFailure::Denied
                | VaultFailure::Platform => AppFailureCategory::Vault,
            };
            AppFailure::retryable(
                category,
                "credential operation failed",
                RecoveryAction::Retry,
            )
        }
        CredentialOperationError::Repository(error) => repository_failure(error),
        CredentialOperationError::ReconciliationRequired => AppFailure::retryable(
            AppFailureCategory::Storage,
            "credential reconciliation required",
            RecoveryAction::Retry,
        ),
    }
}
