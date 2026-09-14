use crate::{AppError, AppEvent, ErrorPaneView, PaneId, SessionFailure, SessionId, SessionUiEvent};

use super::runtime::{CommandLoop, InternalEvent};

impl CommandLoop {
    pub(super) async fn forward(&mut self, forwarded: InternalEvent) {
        let (session, event) = match forwarded {
            InternalEvent::Frame(session, frame) => {
                if !self.forwarders.contains_key(&session) || !self.session_is_bound(session) {
                    return;
                }
                if self
                    .view_model
                    .latest_frames
                    .get(&session)
                    .is_some_and(|current| current.generation > frame.generation)
                {
                    return;
                }
                self.view_model.latest_frames.insert(session, frame.clone());
                self.publish_view();
                (session, SessionUiEvent::Frame(frame))
            }
            InternalEvent::Session(session, event) => {
                if !self.forwarders.contains_key(&session) || !self.session_is_bound(session) {
                    return;
                }
                (session, sanitize_event(event))
            }
        };
        let completed = matches!(
            &event,
            SessionUiEvent::Exited(_) | SessionUiEvent::Failed(_) | SessionUiEvent::Crashed(_)
        );
        match &event {
            SessionUiEvent::State(state) => {
                self.view_model.session_states.insert(session, *state);
                self.publish_view();
            }
            SessionUiEvent::RecoveryChanged(Some(notice)) => {
                self.view_model.display_recovery.insert(session, *notice);
                self.publish_view();
            }
            SessionUiEvent::RecoveryChanged(None)
                if self.view_model.display_recovery.remove(&session).is_some() =>
            {
                self.publish_view();
            }
            SessionUiEvent::InteractionRequired(request) => {
                self.emit(AppEvent::InteractionRequired {
                    session,
                    request: request.clone(),
                })
                .await;
            }
            SessionUiEvent::Failed(failure) => {
                self.view_model
                    .session_states
                    .insert(session, crate::SessionState::Failed);
                self.retain_error_pane(session, *failure, "session failed");
            }
            SessionUiEvent::Crashed(_) => {
                self.view_model
                    .session_states
                    .insert(session, crate::SessionState::Crashed);
                self.retain_error_pane(session, SessionFailure::Crashed, "session actor crashed");
            }
            SessionUiEvent::Exited(_) => {
                self.view_model
                    .session_states
                    .insert(session, crate::SessionState::Exited);
                self.publish_view();
            }
            _ => {}
        }
        self.emit(AppEvent::Session { session, event }).await;
        if completed && self.unbind_session(session) {
            self.publish_view();
        }
    }

    pub(super) async fn finish_shutdown(&mut self) -> Result<(), AppError> {
        self.cancel_all_imports().await;
        let session_shutdown = self
            .dependencies
            .sessions
            .shutdown_all()
            .await
            .map_err(AppError::SessionShutdown);
        for handles in self.forwarders.values() {
            for handle in handles {
                handle.abort();
            }
        }
        self.forwarders.clear();
        self.view_model.workspace = Default::default();
        self.view_model.pane_launches.clear();
        self.view_model.session_states.clear();
        self.view_model.latest_frames.clear();
        self.view_model.display_recovery.clear();
        self.view_model.error_panes.clear();
        self.publish_view();
        let _ = self.events.try_send(AppEvent::ShutdownComplete);
        session_shutdown
    }

    pub(super) fn session_is_bound(&self, session: SessionId) -> bool {
        self.view_model
            .workspace
            .tabs
            .iter()
            .any(|tab| tab.pane_tree.session_ids().contains(&session))
    }

    pub(super) fn retain_error_pane(
        &mut self,
        session: SessionId,
        failure: SessionFailure,
        diagnostic: &'static str,
    ) {
        let host = self
            .pane_for_session(session)
            .and_then(|pane| self.view_model.pane_launches.get(&pane))
            .and_then(|target| target.host())
            .map(str::to_owned);
        self.view_model.error_panes.insert(
            session,
            ErrorPaneView {
                failure,
                diagnostic,
                host,
                timestamp_unix_seconds: unix_timestamp(),
            },
        );
        self.publish_view();
    }

    fn pane_for_session(&self, session: SessionId) -> Option<PaneId> {
        self.view_model.workspace.tabs.iter().find_map(|tab| {
            let mut found = None;
            tab.pane_tree.visit_leaves(&mut |pane, candidate| {
                if candidate == Some(session) {
                    found = Some(pane);
                }
            });
            found
        })
    }
}

fn sanitize_event(event: SessionUiEvent) -> SessionUiEvent {
    match event {
        SessionUiEvent::Crashed(_) => SessionUiEvent::Crashed("session actor crashed".into()),
        other => other,
    }
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
