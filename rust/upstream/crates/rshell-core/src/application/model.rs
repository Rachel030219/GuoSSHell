use std::{collections::BTreeMap, sync::Arc};

use crate::{
    AppSettings, ConnectionCatalog, ConnectionId, DisplayRecoveryNotice, ImportPreviewId,
    ImportPreviewView, PaneId, RenderFrame, SessionFailure, SessionId, SessionState,
    TerminalProfile, WorkspaceState,
};

#[derive(Debug, Clone, PartialEq)]
pub struct AppBootstrapState {
    pub catalog: ConnectionCatalog,
    pub settings: AppSettings,
    pub terminal_profiles: Vec<TerminalProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorPaneView {
    pub failure: SessionFailure,
    pub diagnostic: &'static str,
    pub host: Option<String>,
    pub timestamp_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneLaunchTarget {
    Local,
    Connection { id: ConnectionId, host: String },
}

impl PaneLaunchTarget {
    pub fn connection_id(&self) -> Option<ConnectionId> {
        match self {
            Self::Local => None,
            Self::Connection { id, .. } => Some(*id),
        }
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Connection { host, .. } => Some(host),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppViewModel {
    pub revision: u64,
    pub catalog: ConnectionCatalog,
    pub workspace: WorkspaceState,
    pub settings: AppSettings,
    pub terminal_profiles: Vec<TerminalProfile>,
    pub pending_imports: BTreeMap<ImportPreviewId, ImportPreviewView>,
    pub latest_frames: BTreeMap<SessionId, Arc<RenderFrame>>,
    pub display_recovery: BTreeMap<SessionId, DisplayRecoveryNotice>,
    pub error_panes: BTreeMap<SessionId, ErrorPaneView>,
    pub pane_launches: BTreeMap<PaneId, PaneLaunchTarget>,
    pub session_states: BTreeMap<SessionId, SessionState>,
}

impl From<AppBootstrapState> for AppViewModel {
    fn from(state: AppBootstrapState) -> Self {
        Self {
            revision: 0,
            catalog: state.catalog,
            workspace: WorkspaceState::default(),
            settings: state.settings,
            terminal_profiles: state.terminal_profiles,
            pending_imports: BTreeMap::new(),
            latest_frames: BTreeMap::new(),
            display_recovery: BTreeMap::new(),
            error_panes: BTreeMap::new(),
            pane_launches: BTreeMap::new(),
            session_states: BTreeMap::new(),
        }
    }
}
