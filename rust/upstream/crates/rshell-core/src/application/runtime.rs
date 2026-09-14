use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::{sync::watch, task::AbortHandle};

use crate::{
    AppEvent, AppFailure, AppFailureCategory, AppViewModel, ConnectionId, ConnectionProfile,
    RecoveryAction, RenderFrame, ResolvedTerminalProfile, SessionFailure, SessionId,
    SessionUiEvent, UiCommand,
};

use super::{AppDependencies, SessionBinding};

pub(super) enum InternalEvent {
    Session(SessionId, SessionUiEvent),
    Frame(SessionId, Arc<RenderFrame>),
}

pub(super) struct LoopControl {
    pub(super) accepting: Arc<AtomicBool>,
    pub(super) closed: Arc<AtomicBool>,
    pub(super) shutdown: watch::Receiver<bool>,
}

pub(super) struct CommandLoop {
    pub(super) dependencies: AppDependencies,
    pub(super) view_model: AppViewModel,
    pub(super) events: async_channel::Sender<AppEvent>,
    internal: async_channel::Sender<InternalEvent>,
    view: watch::Sender<AppViewModel>,
    control: LoopControl,
    pub(super) forwarders: BTreeMap<SessionId, Vec<AbortHandle>>,
}

impl CommandLoop {
    pub(super) fn new(
        dependencies: AppDependencies,
        view_model: AppViewModel,
        events: async_channel::Sender<AppEvent>,
        internal: async_channel::Sender<InternalEvent>,
        view: watch::Sender<AppViewModel>,
        control: LoopControl,
    ) -> Self {
        Self {
            dependencies,
            view_model,
            events,
            internal,
            view,
            control,
            forwarders: BTreeMap::new(),
        }
    }

    pub(super) async fn run(
        mut self,
        commands: async_channel::Receiver<UiCommand>,
        internal: async_channel::Receiver<InternalEvent>,
        done: watch::Sender<Option<Result<(), super::AppError>>>,
    ) {
        loop {
            tokio::select! {
                biased;
                changed = self.control.shutdown.changed() => {
                    if changed.is_err() || *self.control.shutdown.borrow() {
                        break;
                    }
                },
                command = commands.recv() => match command {
                    Ok(command) => {
                        if !self.dispatch(command).await {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                forwarded = internal.recv() => match forwarded {
                    Ok(forwarded) => self.forward(forwarded).await,
                    Err(_) => break,
                },
            }
        }
        commands.close();
        self.control.accepting.store(false, Ordering::Release);
        let shutdown = self.finish_shutdown().await;
        self.control.closed.store(true, Ordering::Release);
        done.send_replace(Some(shutdown));
    }

    async fn dispatch(&mut self, command: UiCommand) -> bool {
        match command {
            UiCommand::ApplyCatalog { mutation, secret } => {
                self.apply_catalog(mutation, secret).await
            }
            UiCommand::SearchConnections(query) => self.search_connections(&query).await,
            UiCommand::NewLocalTab => self.new_local_tab().await,
            UiCommand::StartLocal { pane } => self.start_local(pane).await,
            UiCommand::Connect { pane, connection } => self.connect(pane, connection).await,
            UiCommand::Split { pane, axis } => self.split(pane, axis).await,
            UiCommand::ClosePane(pane) => self.close_pane(pane).await,
            UiCommand::CloseTab(tab) => self.close_tab(tab).await,
            UiCommand::RetryPane(pane) => self.retry_pane(pane).await,
            UiCommand::Session { session, command } => {
                let _ = self.session_command(session, command, false).await;
            }
            UiCommand::SaveTerminalProfile(profile) => self.save_terminal_profile(profile).await,
            UiCommand::SaveSettings(settings) => self.save_settings(settings).await,
            UiCommand::PreviewImport { source, path } => self.preview_import(source, path).await,
            UiCommand::CommitImport { preview, selected } => {
                self.commit_import(preview, selected).await
            }
            UiCommand::CancelImport { preview } => self.cancel_import(preview).await,
            UiCommand::Respond {
                session,
                interaction,
                response,
            } => self.respond(session, interaction, response).await,
            UiCommand::Shutdown => return false,
        }
        true
    }

    pub(super) async fn emit(&self, event: AppEvent) {
        let mut shutdown = self.control.shutdown.clone();
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            biased;
            _ = shutdown.changed() => {}
            _ = self.events.send(event) => {}
        }
    }

    pub(super) fn publish_view(&mut self) {
        self.view_model.revision = self.view_model.revision.saturating_add(1);
        self.view.send_replace(self.view_model.clone());
    }

    pub(super) async fn fail(&self, failure: AppFailure) {
        self.emit(AppEvent::OperationFailed(failure)).await;
    }

    pub(super) fn bind(&mut self, binding: SessionBinding) {
        let session = binding.id;
        self.unbind_session(session);
        self.view_model
            .session_states
            .insert(session, crate::SessionState::Created);
        let event_tx = self.internal.clone();
        let events = binding.events;
        let event_task = tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                if matches!(event, SessionUiEvent::Frame(_)) {
                    continue;
                }
                if event_tx
                    .send(InternalEvent::Session(session, event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let frame_tx = self.internal.clone();
        let mut frames = binding.frames;
        let frame_task = tokio::spawn(async move {
            loop {
                let frame = frames.borrow_and_update().clone();
                if let Some(frame) = frame
                    && frame_tx
                        .send(InternalEvent::Frame(session, frame))
                        .await
                        .is_err()
                {
                    break;
                }
                if frames.changed().await.is_err() {
                    break;
                }
            }
        });
        self.forwarders.insert(
            session,
            vec![event_task.abort_handle(), frame_task.abort_handle()],
        );
    }

    pub(super) fn unbind_session(&mut self, session: SessionId) -> bool {
        self.stop_session_forwarders(session);
        let removed_frame = self.view_model.latest_frames.remove(&session).is_some();
        let removed_recovery = self.view_model.display_recovery.remove(&session).is_some();
        removed_frame || removed_recovery
    }

    pub(super) fn stop_session_forwarders(&mut self, session: SessionId) {
        if let Some(handles) = self.forwarders.remove(&session) {
            for handle in handles {
                handle.abort();
            }
        }
    }

    pub(super) fn clear_session_artifacts(&mut self, session: SessionId) {
        self.unbind_session(session);
        self.view_model.session_states.remove(&session);
        self.view_model.error_panes.remove(&session);
    }

    pub(super) fn resolve_terminal_from(
        view_model: &AppViewModel,
        connection: Option<&ConnectionProfile>,
    ) -> Option<ResolvedTerminalProfile> {
        let requested = connection
            .and_then(|profile| profile.terminal_profile_id)
            .unwrap_or(view_model.settings.default_terminal_profile);
        let profile = view_model
            .terminal_profiles
            .iter()
            .find(|profile| profile.id == requested)?;
        Some(match connection {
            Some(connection) => profile.settings.resolve(&connection.terminal_overrides),
            None => profile
                .settings
                .resolve(&crate::TerminalOverrides::default()),
        })
    }

    pub(super) fn session_failure(
        failure: SessionFailure,
        connection: Option<ConnectionId>,
    ) -> AppFailure {
        let category = session_category(failure);
        let action = connection.map_or(RecoveryAction::Retry, RecoveryAction::EditConnection);
        AppFailure::retryable(category, "session operation failed", action)
    }
}

fn session_category(failure: SessionFailure) -> AppFailureCategory {
    match failure {
        SessionFailure::Validation => AppFailureCategory::Validation,
        SessionFailure::Storage => AppFailureCategory::Storage,
        SessionFailure::Vault => AppFailureCategory::Vault,
        SessionFailure::HostKeyRejected | SessionFailure::HostKeyChanged => {
            AppFailureCategory::HostKey
        }
        SessionFailure::Authentication => AppFailureCategory::Authentication,
        SessionFailure::Network | SessionFailure::Timeout | SessionFailure::SshChannel => {
            AppFailureCategory::Network
        }
        SessionFailure::Pty => AppFailureCategory::Pty,
        SessionFailure::Subprocess => AppFailureCategory::Subprocess,
        SessionFailure::Platform => AppFailureCategory::Platform,
        SessionFailure::Backpressure => AppFailureCategory::Backpressure,
        SessionFailure::Crashed => AppFailureCategory::Crashed,
    }
}
