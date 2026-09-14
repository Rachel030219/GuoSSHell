mod catalog;
mod model;
mod mutation;
mod validation;

pub use crate::error::DomainError;
pub use crate::terminal::TerminalOverrides;
pub use catalog::{CatalogMutation, CatalogOutcome, ConnectionCatalog};
pub use model::{
    AuthenticationKind, ConnectionGroup, ConnectionId, ConnectionProfile, CredentialRef, GroupId,
    HostKeyPolicy, PaneId, SessionId, TerminalProfileId, TransportKind,
};
