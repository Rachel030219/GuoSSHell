use std::{collections::BTreeSet, path::PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::terminal::TerminalOverrides;

macro_rules! uuid_newtype {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

uuid_newtype!(ConnectionId);
uuid_newtype!(GroupId);
uuid_newtype!(TerminalProfileId);
uuid_newtype!(SessionId);
uuid_newtype!(PaneId);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialRef(pub String);

impl CredentialRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl From<String> for CredentialRef {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CredentialRef {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    #[default]
    SystemOpenSsh,
    NativeSsh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationKind {
    Password,
    PublicKey,
    #[default]
    Agent,
    KeyboardInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyPolicy {
    #[default]
    Strict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: ConnectionId,
    pub group_id: Option<GroupId>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub transport: TransportKind,
    pub authentication: AuthenticationKind,
    pub credential_ref: Option<CredentialRef>,
    pub identity_file: Option<PathBuf>,
    pub host_key_policy: HostKeyPolicy,
    pub remote_command: Option<String>,
    pub note: String,
    pub tags: BTreeSet<String>,
    pub position: i64,
    #[serde(default)]
    pub terminal_profile_id: Option<TerminalProfileId>,
    #[serde(default)]
    pub terminal_overrides: TerminalOverrides,
}

impl ConnectionProfile {
    pub fn new(name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: ConnectionId::new(),
            group_id: None,
            name: name.into(),
            host: host.into(),
            port: 22,
            username: String::new(),
            transport: TransportKind::SystemOpenSsh,
            authentication: AuthenticationKind::Agent,
            credential_ref: None,
            identity_file: None,
            host_key_policy: HostKeyPolicy::Strict,
            remote_command: None,
            note: String::new(),
            tags: BTreeSet::new(),
            position: i64::MAX,
            terminal_profile_id: None,
            terminal_overrides: TerminalOverrides::default(),
        }
    }
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self::new("New connection", "")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionGroup {
    pub id: GroupId,
    pub parent_id: Option<GroupId>,
    pub name: String,
    pub position: i64,
}

impl ConnectionGroup {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: GroupId::new(),
            parent_id: None,
            name: name.into(),
            position: i64::MAX,
        }
    }
}

impl Default for ConnectionGroup {
    fn default() -> Self {
        Self::new("New group")
    }
}
