use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    connection::PaneId,
    workspace::{PaneTree, WorkspaceError},
};

pub type TabId = Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabState {
    pub id: TabId,
    pub title: String,
    pub pane_tree: PaneTree,
    pub active_pane: PaneId,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub tabs: Vec<TabState>,
    pub active_tab: Option<TabId>,
}

impl WorkspaceState {
    pub fn tab(&self, id: TabId) -> Result<&TabState, WorkspaceError> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or(WorkspaceError::TabNotFound(id))
    }

    pub fn tab_mut(&mut self, id: TabId) -> Result<&mut TabState, WorkspaceError> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .ok_or(WorkspaceError::TabNotFound(id))
    }

    pub fn active_tab(&self) -> Option<&TabState> {
        self.active_tab
            .and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
    }
}
