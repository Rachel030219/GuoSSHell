use thiserror::Error;
use uuid::Uuid;

use crate::connection::PaneId;

mod state;
mod tree;

pub use state::{TabId, TabState, WorkspaceState};
pub use tree::{PaneTree, SplitAxis};

#[derive(Debug, Clone, PartialEq, Error)]
pub enum WorkspaceError {
    #[error("pane {0:?} does not exist")]
    PaneNotFound(PaneId),
    #[error("pane {0:?} already exists")]
    DuplicatePane(PaneId),
    #[error("split ratio must be in 0.1..=0.9: {0}")]
    InvalidSplitRatio(f32),
    #[error("cannot close the last pane")]
    LastPane,
    #[error("tab {0} does not exist")]
    TabNotFound(Uuid),
}
