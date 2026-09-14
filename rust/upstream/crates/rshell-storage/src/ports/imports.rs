use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use async_trait::async_trait;
use rshell_core::{
    ConnectionId, ImportCandidateId, ImportCommitResult, ImportError, ImportPort, ImportPreviewId,
    ImportPreviewView, ImportSourceKind,
};
use tokio::time::Instant;

use crate::{
    CredentialCoordinator, ImportPreview, LegacyJsonImporter, OpenSshConfigImporter,
    OpenSshPreview, SqliteRepository,
};

use super::{
    import_errors::map_import,
    import_views::{legacy_view, openssh_view, report_view},
};

const PREVIEW_TTL: Duration = Duration::from_secs(15 * 60);

pub struct ImportPortAdapter {
    repository: Arc<SqliteRepository>,
    coordinator: Arc<CredentialCoordinator>,
    pending: Mutex<BTreeMap<ImportPreviewId, PendingPreview>>,
    ttl: Duration,
}

struct PendingPreview {
    created: Instant,
    candidates: BTreeMap<ImportCandidateId, ConnectionId>,
    value: PendingValue,
}

enum PendingValue {
    Legacy(ImportPreview),
    OpenSsh(OpenSshPreview),
}

impl ImportPortAdapter {
    pub fn new(repository: Arc<SqliteRepository>, coordinator: Arc<CredentialCoordinator>) -> Self {
        Self::with_ttl(repository, coordinator, PREVIEW_TTL)
    }

    pub fn with_ttl(
        repository: Arc<SqliteRepository>,
        coordinator: Arc<CredentialCoordinator>,
        ttl: Duration,
    ) -> Self {
        Self {
            repository,
            coordinator,
            pending: Mutex::new(BTreeMap::new()),
            ttl,
        }
    }

    /// Deterministic tick hook. Every port call also invokes this cleanup.
    pub fn cleanup_expired(&self) -> usize {
        let now = Instant::now();
        let mut pending = lock(&self.pending);
        let before = pending.len();
        pending.retain(|_, preview| now.duration_since(preview.created) < self.ttl);
        before - pending.len()
    }

    pub fn pending_count(&self) -> usize {
        lock(&self.pending).len()
    }

    fn insert(&self, view: ImportPreviewView, pending: PendingPreview) -> ImportPreviewView {
        lock(&self.pending).insert(view.id, pending);
        view
    }
}

#[async_trait]
impl ImportPort for ImportPortAdapter {
    async fn preview(
        &self,
        source: ImportSourceKind,
        path: &Path,
    ) -> Result<ImportPreviewView, ImportError> {
        self.cleanup_expired();
        let path = path.to_path_buf();
        let id = ImportPreviewId::new();
        match source {
            ImportSourceKind::LegacyRshellJson => {
                let preview = run_legacy_preview(path).await?;
                let (view, candidates) = legacy_view(id, &preview);
                Ok(self.insert(
                    view,
                    PendingPreview {
                        created: Instant::now(),
                        candidates,
                        value: PendingValue::Legacy(preview),
                    },
                ))
            }
            ImportSourceKind::OpenSshConfig => {
                let preview = run_openssh_preview(path).await?;
                let (view, candidates) = openssh_view(id, &preview);
                Ok(self.insert(
                    view,
                    PendingPreview {
                        created: Instant::now(),
                        candidates,
                        value: PendingValue::OpenSsh(preview),
                    },
                ))
            }
        }
    }

    async fn commit(
        &self,
        preview: ImportPreviewId,
        selected: &BTreeSet<ImportCandidateId>,
    ) -> Result<ImportCommitResult, ImportError> {
        self.cleanup_expired();
        let (pending, selected) = {
            let mut previews = lock(&self.pending);
            let pending = previews.get(&preview).ok_or(ImportError::PreviewExpired)?;
            let selected = selected
                .iter()
                .map(|id| {
                    pending
                        .candidates
                        .get(id)
                        .copied()
                        .ok_or(ImportError::Validation)
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            let pending = previews
                .remove(&preview)
                .expect("validated pending preview must remain under lock");
            (pending, selected)
        };
        let coordinator = Arc::clone(&self.coordinator);
        let report = match pending.value {
            PendingValue::Legacy(value) => {
                tokio::task::spawn_blocking(move || {
                    LegacyJsonImporter::new().commit(&coordinator, value, &selected)
                })
                .await
            }
            PendingValue::OpenSsh(value) => {
                tokio::task::spawn_blocking(move || {
                    OpenSshConfigImporter::new().commit(&coordinator, value, &selected)
                })
                .await
            }
        }
        .map_err(|_| ImportError::Storage)?
        .map_err(map_import)?;
        let repository = Arc::clone(&self.repository);
        let catalog = tokio::task::spawn_blocking(move || repository.load_catalog())
            .await
            .map_err(|_| ImportError::Storage)?
            .map_err(|_| ImportError::Storage)?;
        Ok(ImportCommitResult {
            report: report_view(report),
            catalog,
        })
    }

    async fn cancel(&self, preview: ImportPreviewId) -> Result<(), ImportError> {
        self.cleanup_expired();
        lock(&self.pending)
            .remove(&preview)
            .map(|_| ())
            .ok_or(ImportError::PreviewExpired)
    }
}

async fn run_legacy_preview(path: PathBuf) -> Result<ImportPreview, ImportError> {
    tokio::task::spawn_blocking(move || LegacyJsonImporter::new().preview(path))
        .await
        .map_err(|_| ImportError::Storage)?
        .map_err(map_import)
}

async fn run_openssh_preview(path: PathBuf) -> Result<OpenSshPreview, ImportError> {
    tokio::task::spawn_blocking(move || OpenSshConfigImporter::new().preview(path))
        .await
        .map_err(|_| ImportError::Storage)?
        .map_err(map_import)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
