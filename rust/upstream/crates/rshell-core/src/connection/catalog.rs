use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    ConnectionGroup, ConnectionId, ConnectionProfile, DomainError, GroupId,
    mutation::apply_mutation,
    validation::{matches_query, normalize_catalog, validate_catalog},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ConnectionCatalog {
    #[serde(default)]
    pub groups: BTreeMap<GroupId, ConnectionGroup>,
    #[serde(default)]
    pub connections: BTreeMap<ConnectionId, ConnectionProfile>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CatalogMutation {
    Create(ConnectionProfile),
    Update(ConnectionProfile),
    Duplicate {
        source: ConnectionId,
        destination: Option<GroupId>,
    },
    Move {
        connection: ConnectionId,
        destination: Option<GroupId>,
        position: usize,
    },
    Delete(ConnectionId),
    CreateGroup(ConnectionGroup),
    RenameGroup {
        group: GroupId,
        name: String,
    },
    MoveGroup {
        group: GroupId,
        parent: Option<GroupId>,
        position: usize,
    },
    DeleteGroup(GroupId),
    SetTags {
        connection: ConnectionId,
        tags: std::collections::BTreeSet<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogOutcome {
    Connection(ConnectionId),
    Group(GroupId),
    Updated,
    Deleted,
}

impl CatalogOutcome {
    pub fn connection_id(self) -> Result<ConnectionId, DomainError> {
        match self {
            Self::Connection(id) => Ok(id),
            Self::Group(_) | Self::Updated | Self::Deleted => {
                Err(DomainError::OutcomeDoesNotContainConnection)
            }
        }
    }
}

impl ConnectionCatalog {
    pub fn apply(&mut self, mutation: CatalogMutation) -> Result<CatalogOutcome, DomainError> {
        let mut next = self.clone();
        let outcome = apply_mutation(&mut next, mutation)?;
        normalize_catalog(&mut next);
        validate_catalog(&next)?;
        *self = next;
        Ok(outcome)
    }

    pub fn search(&self, query: &str) -> Vec<ConnectionId> {
        let query = query.trim().to_lowercase();
        let mut matches = self
            .connections
            .iter()
            .filter(|(_, profile)| matches_query(profile, &query))
            .map(|(id, profile)| (profile.group_id, profile.position, *id))
            .collect::<Vec<_>>();
        matches.sort();
        matches.into_iter().map(|(_, _, id)| id).collect()
    }

    pub fn ordered_ids(&self, group_id: Option<GroupId>) -> Vec<ConnectionId> {
        let mut profiles = self
            .connections
            .iter()
            .filter(|(_, profile)| profile.group_id == group_id)
            .map(|(id, profile)| (profile.position, *id))
            .collect::<Vec<_>>();
        profiles.sort();
        profiles.into_iter().map(|(_, id)| id).collect()
    }

    pub fn delete_group(&mut self, id: GroupId) -> Result<(), DomainError> {
        self.apply(CatalogMutation::DeleteGroup(id)).map(|_| ())
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        validate_catalog(self)
    }
}
