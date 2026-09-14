use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use rshell_core::{
    AuthenticationKind, ConnectionGroup, ConnectionId, ConnectionProfile, GroupId, HostKeyPolicy,
    TransportKind,
};
use secrecy::ExposeSecret;
use uuid::Uuid;

use super::{
    ImportConnectionCandidate, ImportError, ImportPreview, ImportWarning,
    legacy::{LegacyConnection, LegacyDocument},
    legacy_terminal::map_terminal,
};

pub(super) fn map_document(
    document: LegacyDocument,
    digest: String,
) -> Result<ImportPreview, ImportError> {
    let mut groups = Vec::with_capacity(document.folders.len());
    let mut group_ids = BTreeSet::new();
    for (position, folder) in document.folders.into_iter().enumerate() {
        let id = GroupId(parse_uuid(&folder.id)?);
        if !group_ids.insert(id) {
            return Err(ImportError::InvalidConnection);
        }
        groups.push(ConnectionGroup {
            id,
            parent_id: None,
            name: trim(folder.name),
            position: position as i64,
        });
    }

    let mut connections = Vec::with_capacity(document.connections.len());
    let mut secrets = BTreeMap::new();
    let mut connection_ids = BTreeSet::new();
    let mut warnings = Vec::new();
    for (position, connection) in document.connections.into_iter().enumerate() {
        let (candidate, secret) = map_connection(connection, position, &group_ids, &mut warnings)?;
        if !connection_ids.insert(candidate.id) {
            return Err(ImportError::InvalidConnection);
        }
        if let Some(secret) = secret {
            secrets.insert(candidate.id, secret);
        }
        connections.push(candidate);
    }
    Ok(ImportPreview::new(
        groups,
        connections,
        warnings,
        digest,
        secrets,
    ))
}

fn map_connection(
    connection: LegacyConnection,
    position: usize,
    group_ids: &BTreeSet<GroupId>,
    warnings: &mut Vec<ImportWarning>,
) -> Result<(ImportConnectionCandidate, Option<secrecy::SecretString>), ImportError> {
    let id = ConnectionId(parse_uuid(&connection.id)?);
    let group_id = connection
        .folder_id
        .as_deref()
        .map(parse_uuid)
        .transpose()?
        .map(GroupId);
    if group_id.is_some_and(|group| !group_ids.contains(&group)) {
        return Err(ImportError::InvalidConnection);
    }
    let host = trim(connection.host);
    if host.is_empty() || host.starts_with('-') {
        return Err(ImportError::InvalidConnection);
    }
    let port = match connection.port.unwrap_or(22) {
        1..=65_535 => connection.port.unwrap_or(22) as u16,
        _ => return Err(ImportError::InvalidPort),
    };
    let backend = backend(&connection.backend)?;
    let identity_file = optional_text(connection.identity_file).map(PathBuf::from);
    let secret = connection
        .password
        .filter(|value| !value.expose_secret().trim().is_empty());
    let (transport, authentication) =
        authentication(secret.is_some(), identity_file.is_some(), backend);
    if connection.accept_new_host {
        push_warning(warnings, ImportWarning::HostKeyPolicyUpgraded);
    }
    let terminal_overrides = map_terminal(connection.terminal, warnings)?;
    let profile = ConnectionProfile {
        id,
        group_id,
        name: trim(connection.name),
        host,
        port,
        username: trim(connection.user),
        transport,
        authentication,
        credential_ref: None,
        identity_file,
        host_key_policy: HostKeyPolicy::Strict,
        remote_command: optional_text(connection.remote_command),
        note: trim(connection.note),
        tags: BTreeSet::new(),
        position: position as i64,
        terminal_profile_id: None,
        terminal_overrides,
    };
    Ok((
        ImportConnectionCandidate {
            id,
            profile,
            has_secret: secret.is_some(),
        },
        secret,
    ))
}

fn authentication(
    has_secret: bool,
    has_identity: bool,
    backend: TransportKind,
) -> (TransportKind, AuthenticationKind) {
    if has_secret {
        (TransportKind::NativeSsh, AuthenticationKind::Password)
    } else if has_identity {
        (backend, AuthenticationKind::PublicKey)
    } else if backend == TransportKind::SystemOpenSsh {
        (TransportKind::SystemOpenSsh, AuthenticationKind::Agent)
    } else {
        (
            TransportKind::NativeSsh,
            AuthenticationKind::KeyboardInteractive,
        )
    }
}

fn backend(value: &str) -> Result<TransportKind, ImportError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "system_open_ssh" | "systemopenssh" => Ok(TransportKind::SystemOpenSsh),
        "wez_term_ssh" | "wezterm_ssh" | "weztermssh" => Ok(TransportKind::NativeSsh),
        _ => Err(ImportError::InvalidConnection),
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, ImportError> {
    Uuid::parse_str(value.trim()).map_err(|_| ImportError::InvalidUuid)
}

fn trim(value: String) -> String {
    value.trim().into()
}

fn optional_text(value: String) -> Option<String> {
    nonempty(value)
}

fn nonempty(value: String) -> Option<String> {
    let value = trim(value);
    (!value.is_empty()).then_some(value)
}

pub(super) fn push_warning(warnings: &mut Vec<ImportWarning>, warning: ImportWarning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
