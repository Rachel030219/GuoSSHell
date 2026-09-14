use std::{collections::BTreeSet, path::PathBuf};

use crate::{
    AppEvent, AppFailure, AppFailureCategory, ImportCandidateId, ImportError, ImportPreviewId,
    ImportSourceKind, RecoveryAction,
};

use super::runtime::CommandLoop;

impl CommandLoop {
    pub(super) async fn preview_import(&mut self, source: ImportSourceKind, path: PathBuf) {
        match self.dependencies.imports.preview(source, &path).await {
            Ok(preview) => {
                self.view_model
                    .pending_imports
                    .insert(preview.id, preview.clone());
                self.publish_view();
                self.emit(AppEvent::ImportPreview(preview)).await;
            }
            Err(error) => self.fail(import_failure(error)).await,
        }
    }

    pub(super) async fn commit_import(
        &mut self,
        preview: ImportPreviewId,
        selected: BTreeSet<ImportCandidateId>,
    ) {
        if !self.view_model.pending_imports.contains_key(&preview) {
            self.fail(import_failure(ImportError::PreviewExpired)).await;
            return;
        }
        match self.dependencies.imports.commit(preview, &selected).await {
            Ok(result) => {
                self.view_model.pending_imports.remove(&preview);
                self.view_model.catalog = result.catalog.clone();
                self.publish_view();
                self.emit(AppEvent::CatalogChanged(result.catalog)).await;
                self.emit(AppEvent::ImportCompleted(result.report)).await;
            }
            Err(error) => {
                let failure = import_failure(error.clone());
                self.reconcile_expired_preview(preview, error);
                self.fail(failure).await;
            }
        }
    }

    pub(super) async fn cancel_import(&mut self, preview: ImportPreviewId) {
        if !self.view_model.pending_imports.contains_key(&preview) {
            self.fail(import_failure(ImportError::PreviewExpired)).await;
            return;
        }
        match self.dependencies.imports.cancel(preview).await {
            Ok(()) => {
                self.view_model.pending_imports.remove(&preview);
                self.publish_view();
                self.emit(AppEvent::ImportCancelled(preview)).await;
            }
            Err(error) => {
                let failure = import_failure(error.clone());
                self.reconcile_expired_preview(preview, error);
                self.fail(failure).await;
            }
        }
    }

    pub(super) async fn cancel_all_imports(&mut self) {
        let previews = self
            .view_model
            .pending_imports
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for preview in previews {
            let _ = self.dependencies.imports.cancel(preview).await;
        }
        self.view_model.pending_imports.clear();
        self.publish_view();
    }

    fn reconcile_expired_preview(&mut self, preview: ImportPreviewId, error: ImportError) {
        if error == ImportError::PreviewExpired {
            self.view_model.pending_imports.remove(&preview);
            self.publish_view();
        }
    }
}

fn import_failure(error: ImportError) -> AppFailure {
    match error {
        ImportError::Read
        | ImportError::Parse
        | ImportError::Validation
        | ImportError::Conflict => AppFailure::retryable(
            AppFailureCategory::Validation,
            "import input is invalid",
            RecoveryAction::Retry,
        ),
        ImportError::Vault => AppFailure::retryable(
            AppFailureCategory::Vault,
            "import credential operation failed",
            RecoveryAction::Retry,
        ),
        ImportError::PreviewExpired => AppFailure::retryable(
            AppFailureCategory::Storage,
            "import preview expired",
            RecoveryAction::Retry,
        ),
        ImportError::Storage
        | ImportError::AlreadyImported
        | ImportError::ReconciliationRequired => AppFailure::retryable(
            AppFailureCategory::Storage,
            "import operation failed",
            RecoveryAction::Retry,
        ),
    }
}
