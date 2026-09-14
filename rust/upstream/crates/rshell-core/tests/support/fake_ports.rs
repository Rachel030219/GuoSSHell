use std::{
    collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::Path,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rshell_core::{
    AppBootstrapState, AppDependencies, AppSettings, CatalogMutation, ConnectionCatalog,
    ConnectionProfile, ConnectionRepository, CredentialOperationError, CredentialPort,
    CredentialRef, ImportCandidateId, ImportCommitResult, ImportError, ImportPort, ImportPreviewId,
    ImportPreviewView, ImportReportView, ImportSourceKind, PaneId, RenderFrame, RepositoryError,
    ResolvedTerminalProfile, SecretUpdate, SessionBinding, SessionFailure, SessionId, SessionPort,
    SessionUiCommand, SessionUiEvent, TerminalProfile, TerminalSize,
};
use secrecy::{ExposeSecret, SecretString};

#[derive(Clone)]
pub struct RecordingPorts {
    state: Arc<Mutex<State>>,
    apply_started: Arc<tokio::sync::Notify>,
    apply_release: Arc<tokio::sync::Notify>,
}

struct State {
    calls: Vec<String>,
    catalog: ConnectionCatalog,
    terminal_profiles: Vec<TerminalProfile>,
    settings: AppSettings,
    repository_failure: bool,
    credential_failure: bool,
    block_apply: bool,
    import_error: Option<ImportError>,
    launch_failure: bool,
    credential_reads: usize,
    credential_get_error: Option<CredentialOperationError>,
    pending_secret: Option<SecretString>,
    last_credential_fingerprint: Option<u64>,
    ssh_secret_fingerprints: Vec<Option<u64>>,
    secret_received: bool,
    ssh_secret_present: Option<bool>,
    pending_previews: BTreeSet<ImportPreviewId>,
    cancelled_previews: Vec<ImportPreviewId>,
    sessions: BTreeMap<SessionId, FakeSession>,
    live_sessions: BTreeSet<SessionId>,
    shutdown_failures: BTreeMap<SessionId, SessionFailure>,
    shutdown_all_failure: Option<SessionFailure>,
    session_commands: Vec<(SessionId, SessionUiCommand)>,
    session_command_failure: bool,
    shutdowns: usize,
}

struct FakeSession {
    events: async_channel::Sender<SessionUiEvent>,
    frames: tokio::sync::watch::Sender<Option<Arc<RenderFrame>>>,
}

impl RecordingPorts {
    pub fn new(bootstrap: &AppBootstrapState) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                calls: Vec::new(),
                catalog: bootstrap.catalog.clone(),
                terminal_profiles: bootstrap.terminal_profiles.clone(),
                settings: bootstrap.settings.clone(),
                repository_failure: false,
                credential_failure: false,
                block_apply: false,
                import_error: None,
                launch_failure: false,
                credential_reads: 0,
                credential_get_error: None,
                pending_secret: None,
                last_credential_fingerprint: None,
                ssh_secret_fingerprints: Vec::new(),
                secret_received: false,
                ssh_secret_present: None,
                pending_previews: BTreeSet::new(),
                cancelled_previews: Vec::new(),
                sessions: BTreeMap::new(),
                live_sessions: BTreeSet::new(),
                shutdown_failures: BTreeMap::new(),
                shutdown_all_failure: None,
                session_commands: Vec::new(),
                session_command_failure: false,
                shutdowns: 0,
            })),
            apply_started: Arc::new(tokio::sync::Notify::new()),
            apply_release: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn dependencies(&self) -> AppDependencies {
        AppDependencies {
            repository: Arc::new(self.clone()),
            credentials: Arc::new(self.clone()),
            imports: Arc::new(self.clone()),
            sessions: Arc::new(self.clone()),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }

    pub fn clear_calls(&self) {
        self.state.lock().unwrap().calls.clear();
    }

    pub fn fail_repository(&self, value: bool) {
        self.state.lock().unwrap().repository_failure = value;
    }

    pub fn fail_import(&self, value: bool) {
        self.state.lock().unwrap().import_error = value.then_some(ImportError::Storage);
    }

    pub fn import_error(&self, error: Option<ImportError>) {
        self.state.lock().unwrap().import_error = error;
    }

    pub fn fail_credentials(&self, value: bool) {
        self.state.lock().unwrap().credential_failure = value;
    }

    pub fn block_catalog_apply(&self) {
        self.state.lock().unwrap().block_apply = true;
    }

    pub async fn wait_for_catalog_apply(&self) {
        self.apply_started.notified().await;
    }

    pub fn release_catalog_apply(&self) {
        self.apply_release.notify_one();
    }

    pub fn fail_launch(&self, value: bool) {
        self.state.lock().unwrap().launch_failure = value;
    }

    pub fn expect_secret(&self, value: &str) {
        self.set_secret(SecretString::from(value.to_owned()));
    }

    pub fn set_secret(&self, secret: SecretString) {
        self.state.lock().unwrap().pending_secret = Some(secret);
    }

    pub fn credential_get_error(&self, error: CredentialOperationError) {
        self.state.lock().unwrap().credential_get_error = Some(error);
    }

    pub fn credential_reads(&self) -> usize {
        self.state.lock().unwrap().credential_reads
    }

    pub fn secret_received(&self) -> bool {
        self.state.lock().unwrap().secret_received
    }

    pub fn second_launch_received_replacement_secret(&self) -> bool {
        let state = self.state.lock().unwrap();
        let [Some(first), Some(second)] = state.ssh_secret_fingerprints.as_slice() else {
            return false;
        };
        first != second
    }

    pub fn ssh_secret_present(&self) -> Option<bool> {
        self.state.lock().unwrap().ssh_secret_present
    }

    pub fn latest_session(&self) -> SessionId {
        *self
            .state
            .lock()
            .unwrap()
            .sessions
            .keys()
            .next_back()
            .unwrap()
    }

    pub fn send_session_event(&self, session: SessionId, event: SessionUiEvent) {
        self.state
            .lock()
            .unwrap()
            .sessions
            .get(&session)
            .unwrap()
            .events
            .try_send(event)
            .unwrap();
    }

    pub fn send_frame(&self, session: SessionId, frame: Arc<RenderFrame>) {
        self.state
            .lock()
            .unwrap()
            .sessions
            .get(&session)
            .unwrap()
            .frames
            .send_replace(Some(frame));
    }

    pub async fn wait_for_binding_closed(&self, session: SessionId) {
        let (events, frames) = {
            let state = self.state.lock().unwrap();
            let session = state.sessions.get(&session).unwrap();
            (session.events.clone(), session.frames.clone())
        };
        tokio::join!(events.closed(), frames.closed());
    }

    pub fn session_commands(&self) -> Vec<(SessionId, String)> {
        self.state
            .lock()
            .unwrap()
            .session_commands
            .iter()
            .map(|(id, command)| (*id, format!("{command:?}")))
            .collect()
    }

    pub fn fail_session_commands(&self, value: bool) {
        self.state.lock().unwrap().session_command_failure = value;
    }

    pub fn pending_preview_count(&self) -> usize {
        self.state.lock().unwrap().pending_previews.len()
    }

    pub fn cancelled_previews(&self) -> Vec<ImportPreviewId> {
        self.state.lock().unwrap().cancelled_previews.clone()
    }

    pub fn shutdowns(&self) -> usize {
        self.state.lock().unwrap().shutdowns
    }

    pub fn fail_shutdown_all(&self, failure: SessionFailure) {
        self.state.lock().unwrap().shutdown_all_failure = Some(failure);
    }

    #[allow(dead_code)]
    pub fn is_session_live(&self, session: SessionId) -> bool {
        self.state.lock().unwrap().live_sessions.contains(&session)
    }

    #[allow(dead_code)]
    pub fn live_session_count(&self) -> usize {
        self.state.lock().unwrap().live_sessions.len()
    }

    #[allow(dead_code)]
    pub fn fail_shutdown_for(&self, session: SessionId, failure: SessionFailure) {
        self.state
            .lock()
            .unwrap()
            .shutdown_failures
            .insert(session, failure);
    }

    #[allow(dead_code)]
    pub fn clear_shutdown_failure(&self, session: SessionId) {
        self.state
            .lock()
            .unwrap()
            .shutdown_failures
            .remove(&session);
    }

    fn launch(&self) -> Result<SessionBinding, SessionFailure> {
        let mut state = self.state.lock().unwrap();
        if state.launch_failure {
            return Err(SessionFailure::Pty);
        }
        let id = SessionId::new();
        let (event_tx, event_rx) = async_channel::bounded(16);
        let (frame_tx, frame_rx) = tokio::sync::watch::channel(None);
        state.sessions.insert(
            id,
            FakeSession {
                events: event_tx,
                frames: frame_tx,
            },
        );
        state.live_sessions.insert(id);
        Ok(SessionBinding {
            id,
            events: event_rx,
            frames: frame_rx,
        })
    }
}

#[async_trait]
impl ConnectionRepository for RecordingPorts {
    async fn load_catalog(&self) -> Result<ConnectionCatalog, RepositoryError> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push("repository.load_catalog".into());
        Ok(self.state.lock().unwrap().catalog.clone())
    }

    async fn apply(
        &self,
        _mutation: CatalogMutation,
    ) -> Result<ConnectionCatalog, RepositoryError> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push("repository.apply".into());
        Err(RepositoryError::Unavailable)
    }

    async fn load_terminal_profiles(&self) -> Result<Vec<TerminalProfile>, RepositoryError> {
        Ok(self.state.lock().unwrap().terminal_profiles.clone())
    }

    async fn save_terminal_profile(&self, profile: TerminalProfile) -> Result<(), RepositoryError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("repository.save_terminal_profile".into());
        if state.repository_failure {
            return Err(RepositoryError::Unavailable);
        }
        if let Some(existing) = state
            .terminal_profiles
            .iter_mut()
            .find(|item| item.id == profile.id)
        {
            *existing = profile;
        } else {
            state.terminal_profiles.push(profile);
        }
        Ok(())
    }

    async fn load_settings(&self) -> Result<AppSettings, RepositoryError> {
        Ok(self.state.lock().unwrap().settings.clone())
    }

    async fn save_settings(&self, settings: AppSettings) -> Result<(), RepositoryError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("repository.save_settings".into());
        if state.repository_failure {
            return Err(RepositoryError::Unavailable);
        }
        state.settings = settings;
        Ok(())
    }
}

#[async_trait]
impl CredentialPort for RecordingPorts {
    async fn apply_catalog(
        &self,
        mutation: CatalogMutation,
        _secret: SecretUpdate,
    ) -> Result<ConnectionCatalog, CredentialOperationError> {
        let should_block = self.state.lock().unwrap().block_apply;
        if should_block {
            self.apply_started.notify_one();
            self.apply_release.notified().await;
        }
        let mut state = self.state.lock().unwrap();
        state.calls.push("credentials.apply_catalog".into());
        if state.credential_failure {
            return Err(CredentialOperationError::ReconciliationRequired);
        }
        state.catalog.apply(mutation).map_err(|_| {
            CredentialOperationError::Repository(RepositoryError::Constraint(
                "catalog mutation rejected".into(),
            ))
        })?;
        Ok(state.catalog.clone())
    }

    async fn get(
        &self,
        _key: &CredentialRef,
    ) -> Result<Option<SecretString>, CredentialOperationError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("credentials.get".into());
        state.credential_reads += 1;
        if let Some(error) = state.credential_get_error.clone() {
            return Err(error);
        }
        let secret = state.pending_secret.take();
        state.last_credential_fingerprint = secret.as_ref().map(secret_fingerprint);
        Ok(secret)
    }
}

#[async_trait]
impl ImportPort for RecordingPorts {
    async fn preview(
        &self,
        source: ImportSourceKind,
        _path: &Path,
    ) -> Result<ImportPreviewView, ImportError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("imports.preview".into());
        let id = ImportPreviewId::new();
        state.pending_previews.insert(id);
        Ok(ImportPreviewView {
            id,
            source,
            groups: Vec::new(),
            candidates: Vec::new(),
            warnings: Vec::new(),
        })
    }

    async fn commit(
        &self,
        preview: ImportPreviewId,
        _selected: &BTreeSet<ImportCandidateId>,
    ) -> Result<ImportCommitResult, ImportError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("imports.commit".into());
        if let Some(error) = state.import_error.clone() {
            return Err(error);
        }
        if !state.pending_previews.remove(&preview) {
            return Err(ImportError::PreviewExpired);
        }
        Ok(ImportCommitResult {
            report: ImportReportView::default(),
            catalog: state.catalog.clone(),
        })
    }

    async fn cancel(&self, preview: ImportPreviewId) -> Result<(), ImportError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("imports.cancel".into());
        if let Some(error) = state.import_error.clone() {
            return Err(error);
        }
        state.pending_previews.remove(&preview);
        state.cancelled_previews.push(preview);
        Ok(())
    }
}

#[async_trait]
impl SessionPort for RecordingPorts {
    async fn launch_local(
        &self,
        _pane: PaneId,
        _terminal: ResolvedTerminalProfile,
    ) -> Result<SessionBinding, SessionFailure> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push("session.launch_local".into());
        self.launch()
    }

    async fn launch_ssh(
        &self,
        _pane: PaneId,
        _profile: ConnectionProfile,
        _terminal: ResolvedTerminalProfile,
        _initial_size: TerminalSize,
        secret: Option<SecretString>,
    ) -> Result<SessionBinding, SessionFailure> {
        let received = secret.as_ref().map(secret_fingerprint);
        {
            let mut state = self.state.lock().unwrap();
            state.calls.push("session.launch_ssh".into());
            state.secret_received =
                received.is_some() && received == state.last_credential_fingerprint;
            state.ssh_secret_present = Some(secret.is_some());
            state.ssh_secret_fingerprints.push(received);
        }
        self.launch()
    }

    async fn command(
        &self,
        session: SessionId,
        command: SessionUiCommand,
    ) -> Result<(), SessionFailure> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("session.command".into());
        if state.session_command_failure {
            return Err(SessionFailure::Backpressure);
        }
        state.session_commands.push((session, command));
        Ok(())
    }

    async fn shutdown(&self, session: SessionId) -> Result<(), SessionFailure> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("session.shutdown".into());
        if let Some(failure) = state.shutdown_failures.get(&session).copied() {
            return Err(failure);
        }
        state.live_sessions.remove(&session);
        Ok(())
    }

    async fn shutdown_all(&self) -> Result<(), SessionFailure> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("session.shutdown_all".into());
        state.shutdowns += 1;
        state.live_sessions.clear();
        state.shutdown_all_failure.map_or(Ok(()), Err)
    }
}

fn secret_fingerprint(secret: &SecretString) -> u64 {
    let mut hasher = DefaultHasher::new();
    secret.expose_secret().hash(&mut hasher);
    hasher.finish()
}

pub fn bootstrap_state() -> AppBootstrapState {
    AppBootstrapState {
        catalog: ConnectionCatalog::default(),
        settings: AppSettings::default(),
        terminal_profiles: vec![TerminalProfile::default()],
    }
}
