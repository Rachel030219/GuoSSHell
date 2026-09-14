use serde::{Deserialize, Serialize};

use crate::{
    connection::{PaneId, SessionId},
    workspace::WorkspaceError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaneTree {
    Leaf {
        pane_id: PaneId,
        session_id: Option<SessionId>,
    },
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<PaneTree>,
        second: Box<PaneTree>,
    },
}

impl PaneTree {
    pub fn leaf(pane_id: PaneId) -> Self {
        Self::Leaf {
            pane_id,
            session_id: None,
        }
    }

    pub fn with_session(pane_id: PaneId, session_id: SessionId) -> Self {
        Self::Leaf {
            pane_id,
            session_id: Some(session_id),
        }
    }

    pub fn split(
        mut self,
        pane_id: PaneId,
        axis: SplitAxis,
        new_pane_id: PaneId,
        ratio: f32,
    ) -> Result<Self, WorkspaceError> {
        if !(0.1..=0.9).contains(&ratio) || !ratio.is_finite() {
            return Err(WorkspaceError::InvalidSplitRatio(ratio));
        }
        if self.contains_pane(new_pane_id) {
            return Err(WorkspaceError::DuplicatePane(new_pane_id));
        }
        if self.split_at(pane_id, axis, new_pane_id, ratio) {
            Ok(self)
        } else {
            Err(WorkspaceError::PaneNotFound(pane_id))
        }
    }

    pub fn close(self, pane_id: PaneId) -> Result<Self, WorkspaceError> {
        match self.close_inner(pane_id)? {
            CloseResult::Removed => Err(WorkspaceError::LastPane),
            CloseResult::Tree(tree) => Ok(tree),
        }
    }

    pub fn find_pane(&self, pane_id: PaneId) -> Result<&Self, WorkspaceError> {
        match self {
            Self::Leaf {
                pane_id: current, ..
            } if *current == pane_id => Ok(self),
            Self::Leaf { .. } => Err(WorkspaceError::PaneNotFound(pane_id)),
            Self::Split { first, second, .. } => first
                .find_pane(pane_id)
                .or_else(|_| second.find_pane(pane_id)),
        }
    }

    pub fn session_id(&self, pane_id: PaneId) -> Result<Option<SessionId>, WorkspaceError> {
        match self.find_pane(pane_id)? {
            Self::Leaf { session_id, .. } => Ok(*session_id),
            Self::Split { .. } => unreachable!("find_pane only returns leaves"),
        }
    }

    pub fn replace_session(
        &mut self,
        pane_id: PaneId,
        session_id: Option<SessionId>,
    ) -> Result<Option<SessionId>, WorkspaceError> {
        match self {
            Self::Leaf {
                pane_id: current,
                session_id: current_session,
            } if *current == pane_id => Ok(std::mem::replace(current_session, session_id)),
            Self::Leaf { .. } => Err(WorkspaceError::PaneNotFound(pane_id)),
            Self::Split { first, second, .. } => first
                .replace_session(pane_id, session_id)
                .or_else(|_| second.replace_session(pane_id, session_id)),
        }
    }

    pub fn replace(self, pane_id: PaneId, replacement: PaneTree) -> Result<Self, WorkspaceError> {
        match self {
            Self::Leaf {
                pane_id: current, ..
            } if current == pane_id => Ok(replacement),
            Self::Leaf { .. } => Err(WorkspaceError::PaneNotFound(pane_id)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } if first.contains_pane(pane_id) => Ok(Self::Split {
                axis,
                ratio,
                first: Box::new(first.replace(pane_id, replacement)?),
                second,
            }),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => Ok(Self::Split {
                axis,
                ratio,
                first,
                second: Box::new(second.replace(pane_id, replacement)?),
            }),
        }
    }

    pub fn contains_pane(&self, pane_id: PaneId) -> bool {
        self.find_pane(pane_id).is_ok()
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut pane_ids = Vec::new();
        self.visit_leaves(&mut |pane_id, _| pane_ids.push(pane_id));
        pane_ids
    }

    pub fn session_ids(&self) -> Vec<SessionId> {
        let mut session_ids = Vec::new();
        self.visit_leaves(&mut |_, session_id| {
            if let Some(session_id) = session_id {
                session_ids.push(session_id);
            }
        });
        session_ids
    }

    pub fn visit_leaves(&self, visitor: &mut impl FnMut(PaneId, Option<SessionId>)) {
        match self {
            Self::Leaf {
                pane_id,
                session_id,
            } => visitor(*pane_id, *session_id),
            Self::Split { first, second, .. } => {
                first.visit_leaves(visitor);
                second.visit_leaves(visitor);
            }
        }
    }

    fn split_at(
        &mut self,
        pane_id: PaneId,
        axis: SplitAxis,
        new_pane_id: PaneId,
        ratio: f32,
    ) -> bool {
        match self {
            Self::Leaf {
                pane_id: current, ..
            } if *current == pane_id => {
                let existing = std::mem::replace(self, Self::leaf(new_pane_id));
                *self = Self::Split {
                    axis,
                    ratio,
                    first: Box::new(existing),
                    second: Box::new(Self::leaf(new_pane_id)),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.split_at(pane_id, axis, new_pane_id, ratio)
                    || second.split_at(pane_id, axis, new_pane_id, ratio)
            }
        }
    }

    fn close_inner(self, pane_id: PaneId) -> Result<CloseResult, WorkspaceError> {
        match self {
            Self::Leaf {
                pane_id: current, ..
            } if current == pane_id => Ok(CloseResult::Removed),
            Self::Leaf { .. } => Err(WorkspaceError::PaneNotFound(pane_id)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } if first.contains_pane(pane_id) => match first.close_inner(pane_id)? {
                CloseResult::Removed => Ok(CloseResult::Tree(*second)),
                CloseResult::Tree(first) => Ok(CloseResult::Tree(Self::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second,
                })),
            },
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match second.close_inner(pane_id)? {
                CloseResult::Removed => Ok(CloseResult::Tree(*first)),
                CloseResult::Tree(second) => Ok(CloseResult::Tree(Self::Split {
                    axis,
                    ratio,
                    first,
                    second: Box::new(second),
                })),
            },
        }
    }
}

enum CloseResult {
    Removed,
    Tree(PaneTree),
}
