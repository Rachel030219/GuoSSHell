use thiserror::Error;

use crate::{
    SettingsValidationCode,
    connection::{AuthenticationKind, ConnectionId, GroupId, TransportKind},
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("connection {0:?} does not exist")]
    ConnectionNotFound(ConnectionId),
    #[error("connection {0:?} already exists")]
    DuplicateConnection(ConnectionId),
    #[error("group {0:?} does not exist")]
    GroupNotFound(GroupId),
    #[error("group {0:?} already exists")]
    DuplicateGroup(GroupId),
    #[error("group {group_id:?} creates a parent cycle through {parent_id:?}")]
    GroupCycle {
        group_id: GroupId,
        parent_id: GroupId,
    },
    #[error("group {group_id:?} is not empty")]
    GroupNotEmpty { group_id: GroupId },
    #[error("group {group_id:?} has position {position}, expected {expected}")]
    InvalidGroupPosition {
        group_id: GroupId,
        position: i64,
        expected: i64,
    },
    #[error("connection {connection_id:?} has position {position}, expected {expected}")]
    InvalidConnectionPosition {
        connection_id: ConnectionId,
        position: i64,
        expected: i64,
    },
    #[error("host must not be empty or start with '-': {host:?}")]
    InvalidHost { host: String },
    #[error("port must be in 1..=65535: {port}")]
    InvalidPort { port: u16 },
    #[error("{authentication:?} authentication is not supported by {transport:?}")]
    InvalidAuthentication {
        transport: TransportKind,
        authentication: AuthenticationKind,
    },
    #[error("public-key authentication for {connection_id:?} requires an identity file")]
    MissingIdentityFile { connection_id: ConnectionId },
    #[error("password authentication for {connection_id:?} requires a credential reference")]
    MissingCredentialRef { connection_id: ConnectionId },
    #[error("terminal override {field} is invalid: {code:?}")]
    InvalidTerminalOverride {
        field: &'static str,
        code: SettingsValidationCode,
    },
    #[error("catalog outcome does not contain a connection identifier")]
    OutcomeDoesNotContainConnection,
}
