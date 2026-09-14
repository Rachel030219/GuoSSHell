use secrecy::SecretString;

use crate::{
    AppFailure, AppFailureCategory, ConnectionId, ConnectionProfile, PaneId, PaneLaunchTarget,
    RecoveryAction, ResolvedTerminalProfile, SessionBinding, SessionFailure, TerminalSize,
};

use super::{credentials::launch_secret, runtime::CommandLoop};

enum PreparedLaunch {
    Local(ResolvedTerminalProfile),
    Connection(Box<PreparedConnection>),
}

struct PreparedConnection {
    profile: ConnectionProfile,
    terminal: ResolvedTerminalProfile,
    size: TerminalSize,
    secret: Option<SecretString>,
}

impl CommandLoop {
    pub(super) async fn start_local(&mut self, pane: PaneId) {
        self.relaunch(pane, PaneLaunchTarget::Local).await;
    }

    pub(super) async fn connect(&mut self, pane: PaneId, connection: ConnectionId) {
        let Some(profile) = self.view_model.catalog.connections.get(&connection) else {
            self.fail(validation_for_connection(connection)).await;
            return;
        };
        self.relaunch(
            pane,
            PaneLaunchTarget::Connection {
                id: connection,
                host: profile.host.clone(),
            },
        )
        .await;
    }

    pub(super) async fn retry_pane(&mut self, pane: PaneId) {
        let Some(target) = self.view_model.pane_launches.get(&pane).cloned() else {
            self.fail(validation_failure()).await;
            return;
        };
        self.relaunch(pane, target).await;
    }

    async fn relaunch(&mut self, pane: PaneId, target: PaneLaunchTarget) {
        let Some((tab_index, old)) = self.pane_location(pane) else {
            self.fail(validation_failure()).await;
            return;
        };
        let Some(prepared) = self.prepare(&target).await else {
            return;
        };
        if let Some(old) = old
            && !self.shutdown_bound_session(old).await
        {
            return;
        }
        match self.launch(pane, prepared).await {
            Ok((binding, title)) => {
                let _ = self.view_model.workspace.tabs[tab_index]
                    .pane_tree
                    .replace_session(pane, Some(binding.id));
                if let Some(title) = title {
                    self.view_model.workspace.tabs[tab_index].title = title;
                }
                self.view_model.pane_launches.insert(pane, target);
                self.bind(binding);
                self.workspace_changed().await;
            }
            Err(error) => {
                if let Some(old) = old {
                    self.view_model
                        .session_states
                        .insert(old, crate::SessionState::Failed);
                    self.retain_error_pane(old, error, "session launch failed");
                }
                self.fail(Self::session_failure(error, target.connection_id()))
                    .await;
            }
        }
    }

    async fn prepare(&self, target: &PaneLaunchTarget) -> Option<PreparedLaunch> {
        match target {
            PaneLaunchTarget::Local => {
                Self::resolve_terminal_from(&self.view_model, None).map(PreparedLaunch::Local)
            }
            PaneLaunchTarget::Connection { id, .. } => {
                let Some(profile) = self.view_model.catalog.connections.get(id).cloned() else {
                    self.fail(validation_for_connection(*id)).await;
                    return None;
                };
                let Some(terminal) = Self::resolve_terminal_from(&self.view_model, Some(&profile))
                else {
                    self.fail(validation_for_connection(*id)).await;
                    return None;
                };
                let secret =
                    match launch_secret(self.dependencies.credentials.as_ref(), &profile).await {
                        Ok(secret) => secret,
                        Err(failure) => {
                            self.fail(failure).await;
                            return None;
                        }
                    };
                let size = initial_size(&terminal);
                Some(PreparedLaunch::Connection(Box::new(PreparedConnection {
                    profile,
                    terminal,
                    size,
                    secret,
                })))
            }
        }
    }

    async fn launch(
        &self,
        pane: PaneId,
        prepared: PreparedLaunch,
    ) -> Result<(SessionBinding, Option<String>), SessionFailure> {
        match prepared {
            PreparedLaunch::Local(terminal) => self
                .dependencies
                .sessions
                .launch_local(pane, terminal)
                .await
                .map(|binding| (binding, Some("Local".into()))),
            PreparedLaunch::Connection(prepared) => {
                let PreparedConnection {
                    profile,
                    terminal,
                    size,
                    secret,
                } = *prepared;
                let title = profile.name.clone();
                self.dependencies
                    .sessions
                    .launch_ssh(pane, profile, terminal, size, secret)
                    .await
                    .map(|binding| (binding, Some(title)))
            }
        }
    }

    pub(super) async fn shutdown_bound_session(&mut self, session: crate::SessionId) -> bool {
        match self.dependencies.sessions.shutdown(session).await {
            Ok(()) => {
                self.clear_session_artifacts(session);
                true
            }
            Err(error) => {
                self.fail(Self::session_failure(error, None)).await;
                false
            }
        }
    }
}

fn initial_size(terminal: &ResolvedTerminalProfile) -> TerminalSize {
    TerminalSize {
        cols: terminal.cols,
        rows: terminal.rows,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    }
}

fn validation_failure() -> AppFailure {
    AppFailure::retryable(
        AppFailureCategory::Validation,
        "workspace operation is invalid",
        RecoveryAction::Retry,
    )
}

fn validation_for_connection(connection: ConnectionId) -> AppFailure {
    AppFailure::retryable(
        AppFailureCategory::Validation,
        "connection operation is invalid",
        RecoveryAction::EditConnection(connection),
    )
}
