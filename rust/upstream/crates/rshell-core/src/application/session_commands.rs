use crate::{
    AppEvent, AppFailure, AppFailureCategory, InteractionId, InteractionResponse, PaneId,
    RecoveryAction, SessionId, SessionUiCommand, WorkspaceError,
};

use super::runtime::CommandLoop;

impl CommandLoop {
    pub(super) async fn session_command(
        &self,
        session: SessionId,
        command: SessionUiCommand,
        report_missing: bool,
    ) -> bool {
        if !self.session_is_bound(session) {
            if report_missing {
                self.fail(unknown_session()).await;
            }
            return false;
        }
        if let Err(error) = self.dependencies.sessions.command(session, command).await {
            self.fail(Self::session_failure(error, None)).await;
            return false;
        }
        true
    }

    pub(super) async fn respond(
        &self,
        session: SessionId,
        interaction: InteractionId,
        response: InteractionResponse,
    ) {
        if self
            .session_command(
                session,
                SessionUiCommand::Respond {
                    interaction,
                    response,
                },
                true,
            )
            .await
        {
            self.emit(AppEvent::InteractionResponded {
                session,
                interaction,
            })
            .await;
        }
    }

    pub(super) async fn close_pane(&mut self, pane: PaneId) {
        let Some((tab_index, session)) = self.pane_location(pane) else {
            self.fail(invalid_workspace()).await;
            return;
        };
        let candidate = self.view_model.workspace.tabs[tab_index]
            .pane_tree
            .clone()
            .close(pane);
        let candidate = match candidate {
            Ok(candidate) => candidate,
            Err(WorkspaceError::LastPane) => {
                let tab = self.view_model.workspace.tabs[tab_index].id;
                self.close_tab(tab).await;
                return;
            }
            Err(_) => {
                self.fail(invalid_workspace()).await;
                return;
            }
        };
        if let Some(session) = session
            && !self.shutdown_bound_session(session).await
        {
            return;
        }
        let active_pane = candidate.pane_ids()[0];
        self.view_model.workspace.tabs[tab_index].pane_tree = candidate;
        self.view_model.workspace.tabs[tab_index].active_pane = active_pane;
        self.view_model.pane_launches.remove(&pane);
        self.workspace_changed().await;
    }

    pub(super) async fn close_tab(&mut self, tab: uuid::Uuid) {
        let Some(index) = self
            .view_model
            .workspace
            .tabs
            .iter()
            .position(|candidate| candidate.id == tab)
        else {
            self.fail(invalid_workspace()).await;
            return;
        };
        let sessions = self.view_model.workspace.tabs[index]
            .pane_tree
            .session_ids();
        let mut stopped = Vec::new();
        for session in &sessions {
            match self.dependencies.sessions.shutdown(*session).await {
                Ok(()) => stopped.push(*session),
                Err(error) => {
                    for stopped in stopped {
                        self.stop_session_forwarders(stopped);
                        self.view_model
                            .session_states
                            .insert(stopped, crate::SessionState::Exited);
                        self.view_model.error_panes.remove(&stopped);
                    }
                    self.publish_view();
                    self.fail(Self::session_failure(error, None)).await;
                    return;
                }
            }
        }
        for session in sessions {
            self.clear_session_artifacts(session);
        }
        let removed = self.view_model.workspace.tabs.remove(index);
        self.view_model.workspace.active_tab = self
            .view_model
            .workspace
            .tabs
            .get(index.min(self.view_model.workspace.tabs.len().saturating_sub(1)))
            .map(|tab| tab.id);
        for pane in removed.pane_tree.pane_ids() {
            self.view_model.pane_launches.remove(&pane);
        }
        self.workspace_changed().await;
    }
}

fn unknown_session() -> AppFailure {
    AppFailure::retryable(
        AppFailureCategory::Validation,
        "session is no longer available",
        RecoveryAction::Retry,
    )
}

fn invalid_workspace() -> AppFailure {
    AppFailure::retryable(
        AppFailureCategory::Validation,
        "workspace operation is invalid",
        RecoveryAction::Retry,
    )
}
