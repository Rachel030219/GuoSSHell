use std::fmt;

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

uuid_newtype!(InteractionId);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostKeyPrompt {
    pub id: InteractionId,
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub sha256: String,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPrompt {
    pub id: InteractionId,
    pub label: String,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardInteractivePrompt {
    pub id: InteractionId,
    pub name: String,
    pub instruction: String,
    pub prompts: Vec<AuthPrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InteractionRequest {
    HostKey(HostKeyPrompt),
    Password(AuthPrompt),
    PrivateKeyPassphrase(AuthPrompt),
    KeyboardInteractive(KeyboardInteractivePrompt),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyDecision {
    AcceptAndStore,
    Reject,
}

pub enum InteractionResponse {
    HostKey(HostKeyDecision),
    Secret(SecretString),
    Answers(Vec<SecretString>),
    Cancel,
}

impl fmt::Debug for InteractionResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HostKey(decision) => formatter.debug_tuple("HostKey").field(decision).finish(),
            Self::Secret(_) => formatter.write_str("Secret([REDACTED])"),
            Self::Answers(_) => formatter.write_str("Answers([REDACTED])"),
            Self::Cancel => formatter.write_str("Cancel"),
        }
    }
}

pub enum SecretUpdate {
    Unchanged,
    Set(SecretString),
    Clear,
}

impl fmt::Debug for SecretUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unchanged => formatter.write_str("Unchanged"),
            Self::Set(_) => formatter.write_str("Set([REDACTED])"),
            Self::Clear => formatter.write_str("Clear"),
        }
    }
}
