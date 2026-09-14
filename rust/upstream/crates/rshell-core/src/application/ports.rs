use std::{collections::BTreeSet, path::Path, sync::Arc};

use async_trait::async_trait;
use secrecy::SecretString;
use thiserror::Error;
use tokio::sync::watch;

use crate::{
    AppSettings, CatalogMutation, ConnectionCatalog, ConnectionProfile, CredentialRef,
    ImportCandidateId, ImportPreviewId, ImportPreviewView, ImportReportView, ImportSourceKind,
    PaneId, RenderFrame, ResolvedTerminalProfile, SecretUpdate, SessionFailure, SessionId,
    SessionUiCommand, SessionUiEvent, TerminalProfile, TerminalSize, UiCommand,
};

pub const UI_COMMAND_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RepositoryError {
    #[error("storage unavailable")]
    Unavailable,
    #[error("storage busy")]
    Busy,
    #[error("storage constraint")]
    Constraint(String),
    #[error("storage data is corrupt")]
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum VaultFailure {
    #[error("credential vault unavailable")]
    Unavailable,
    #[error("credential is missing")]
    NoEntry,
    #[error("credential access denied")]
    Denied,
    #[error("credential platform failure")]
    Platform,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CredentialOperationError {
    #[error("credential vault operation failed")]
    Vault(VaultFailure),
    #[error("credential repository operation failed")]
    Repository(RepositoryError),
    #[error("credential reconciliation is required")]
    ReconciliationRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ImportError {
    #[error("import source could not be read")]
    Read,
    #[error("import source could not be parsed")]
    Parse,
    #[error("import values are invalid")]
    Validation,
    #[error("import conflicts with existing data")]
    Conflict,
    #[error("import credential vault operation failed")]
    Vault,
    #[error("import storage operation failed")]
    Storage,
    #[error("import source was already imported")]
    AlreadyImported,
    #[error("import credential reconciliation is required")]
    ReconciliationRequired,
    #[error("import preview expired")]
    PreviewExpired,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportCommitResult {
    pub report: ImportReportView,
    pub catalog: ConnectionCatalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum UiPortError {
    #[error("application is busy")]
    Busy,
    #[error("application command port is closed")]
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AppError {
    #[error("application bootstrap state is invalid")]
    InvalidBootstrap,
    #[error("initial local session failed")]
    InitialSession(SessionFailure),
    #[error("application session shutdown failed")]
    SessionShutdown(SessionFailure),
    #[error("application command loop is closed")]
    Closed,
}

#[async_trait]
pub trait ConnectionRepository: Send + Sync {
    async fn load_catalog(&self) -> Result<ConnectionCatalog, RepositoryError>;
    async fn apply(&self, mutation: CatalogMutation) -> Result<ConnectionCatalog, RepositoryError>;
    async fn load_terminal_profiles(&self) -> Result<Vec<TerminalProfile>, RepositoryError>;
    async fn save_terminal_profile(&self, profile: TerminalProfile) -> Result<(), RepositoryError>;
    async fn load_settings(&self) -> Result<AppSettings, RepositoryError>;
    async fn save_settings(&self, settings: AppSettings) -> Result<(), RepositoryError>;
}

#[async_trait]
pub trait CredentialPort: Send + Sync {
    async fn apply_catalog(
        &self,
        mutation: CatalogMutation,
        secret: SecretUpdate,
    ) -> Result<ConnectionCatalog, CredentialOperationError>;
    async fn get(
        &self,
        key: &CredentialRef,
    ) -> Result<Option<SecretString>, CredentialOperationError>;
}

#[async_trait]
pub trait ImportPort: Send + Sync {
    async fn preview(
        &self,
        source: ImportSourceKind,
        path: &Path,
    ) -> Result<ImportPreviewView, ImportError>;
    async fn commit(
        &self,
        preview: ImportPreviewId,
        selected: &BTreeSet<ImportCandidateId>,
    ) -> Result<ImportCommitResult, ImportError>;
    async fn cancel(&self, preview: ImportPreviewId) -> Result<(), ImportError>;
}

pub struct SessionBinding {
    pub id: SessionId,
    pub events: async_channel::Receiver<SessionUiEvent>,
    pub frames: watch::Receiver<Option<Arc<RenderFrame>>>,
}

#[async_trait]
pub trait SessionPort: Send + Sync {
    async fn launch_local(
        &self,
        pane: PaneId,
        terminal: ResolvedTerminalProfile,
    ) -> Result<SessionBinding, SessionFailure>;
    async fn launch_ssh(
        &self,
        pane: PaneId,
        profile: ConnectionProfile,
        terminal: ResolvedTerminalProfile,
        initial_size: TerminalSize,
        secret: Option<SecretString>,
    ) -> Result<SessionBinding, SessionFailure>;
    async fn command(
        &self,
        session: SessionId,
        command: SessionUiCommand,
    ) -> Result<(), SessionFailure>;
    /// Gracefully stops one actor and returns only after it is no longer live.
    /// Implementations must bound this wait and fail closed on timeout.
    async fn shutdown(&self, session: SessionId) -> Result<(), SessionFailure>;
    async fn shutdown_all(&self) -> Result<(), SessionFailure>;
}

pub trait UiCommandPort: Send + Sync {
    fn try_send(&self, command: UiCommand) -> Result<(), UiPortError>;
}

pub struct AppDependencies {
    pub repository: Arc<dyn ConnectionRepository>,
    pub credentials: Arc<dyn CredentialPort>,
    pub imports: Arc<dyn ImportPort>,
    pub sessions: Arc<dyn SessionPort>,
}
