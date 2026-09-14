use crate::{
    AppEvent, AppFailure, AppFailureCategory, PaneId, PaneLaunchTarget, PaneTree, RecoveryAction,
    SplitAxis, TabState,
};

use super::runtime::CommandLoop;

impl CommandLoop {
    pub(super) async fn new_local_tab(&mut self) {
        let Some(terminal) = Self::resolve_terminal_from(&self.view_model, None) else {
            self.fail(validation_failure()).await;
            return;
        };
        let pane = PaneId::new();
        match self
            .dependencies
            .sessions
            .launch_local(pane, terminal)
            .await
        {
            Ok(binding) => {
                let id = uuid::Uuid::new_v4();
                self.view_model.workspace.tabs.push(TabState {
                    id,
                    title: "Local".into(),
                    pane_tree: PaneTree::with_session(pane, binding.id),
                    active_pane: pane,
                });
                self.view_model.workspace.active_tab = Some(id);
                self.view_model
                    .pane_launches
                    .insert(pane, PaneLaunchTarget::Local);
                self.bind(binding);
                self.workspace_changed().await;
            }
            Err(error) => self.fail(Self::session_failure(error, None)).await,
        }
    }

    pub(super) async fn split(&mut self, pane: PaneId, axis: SplitAxis) {
        let Some((tab_index, _)) = self.pane_location(pane) else {
            self.fail(validation_failure()).await;
            return;
        };
        let new_pane = PaneId::new();
        let candidate = self.view_model.workspace.tabs[tab_index]
            .pane_tree
            .clone()
            .split(pane, axis, new_pane, 0.5);
        let Ok(mut candidate) = candidate else {
            self.fail(validation_failure()).await;
            return;
        };
        let Some(terminal) = Self::resolve_terminal_from(&self.view_model, None) else {
            self.fail(validation_failure()).await;
            return;
        };
        match self
            .dependencies
            .sessions
            .launch_local(new_pane, terminal)
            .await
        {
            Ok(binding) => {
                let _ = candidate.replace_session(new_pane, Some(binding.id));
                self.view_model.workspace.tabs[tab_index].pane_tree = candidate;
                self.view_model.workspace.tabs[tab_index].active_pane = new_pane;
                self.view_model
                    .pane_launches
                    .insert(new_pane, PaneLaunchTarget::Local);
                self.bind(binding);
                self.workspace_changed().await;
            }
            Err(error) => self.fail(Self::session_failure(error, None)).await,
        }
    }

    pub(super) async fn workspace_changed(&mut self) {
        self.publish_view();
        self.emit(AppEvent::WorkspaceChanged(
            self.view_model.workspace.clone(),
        ))
        .await;
    }

    pub(super) fn pane_location(&self, pane: PaneId) -> Option<(usize, Option<crate::SessionId>)> {
        self.view_model
            .workspace
            .tabs
            .iter()
            .enumerate()
            .find_map(|(index, tab)| {
                tab.pane_tree
                    .session_id(pane)
                    .ok()
                    .map(|session| (index, session))
            })
    }
}

fn validation_failure() -> AppFailure {
    AppFailure::retryable(
        AppFailureCategory::Validation,
        "workspace operation is invalid",
        RecoveryAction::Retry,
    )
}
