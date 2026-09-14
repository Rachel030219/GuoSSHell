use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::{
    AuthenticationKind, ConnectionCatalog, ConnectionProfile, CredentialRef, DomainError,
    TransportKind,
};
use crate::validate_terminal_overrides;

pub(crate) fn normalize_catalog(catalog: &mut ConnectionCatalog) {
    for group in catalog.groups.values_mut() {
        group.name = group.name.trim().into();
    }
    for profile in catalog.connections.values_mut() {
        normalize_profile(profile);
    }
    normalize_group_positions(catalog);
    normalize_connection_positions(catalog);
}

pub(crate) fn validate_catalog(catalog: &ConnectionCatalog) -> Result<(), DomainError> {
    for group in catalog.groups.values() {
        if let Some(parent_id) = group.parent_id
            && !catalog.groups.contains_key(&parent_id)
        {
            return Err(DomainError::GroupNotFound(parent_id));
        }
    }
    validate_group_hierarchy(catalog)?;
    validate_group_positions(catalog)?;

    for profile in catalog.connections.values() {
        if let Some(group_id) = profile.group_id
            && !catalog.groups.contains_key(&group_id)
        {
            return Err(DomainError::GroupNotFound(group_id));
        }
        validate_profile(profile)?;
    }
    validate_connection_positions(catalog)
}

pub(crate) fn matches_query(profile: &ConnectionProfile, query: &str) -> bool {
    query.is_empty()
        || [
            profile.name.as_str(),
            profile.host.as_str(),
            profile.username.as_str(),
        ]
        .into_iter()
        .any(|value| value.to_lowercase().contains(query))
        || profile
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(query))
}

fn normalize_profile(profile: &mut ConnectionProfile) {
    profile.name = profile.name.trim().into();
    profile.host = profile.host.trim().into();
    profile.username = profile.username.trim().into();
    profile.note = profile.note.trim().into();
    profile.remote_command = profile
        .remote_command
        .take()
        .map(|command| command.trim().into())
        .filter(|command: &String| !command.is_empty());
    profile.identity_file = profile.identity_file.take().and_then(normalize_path);
    profile.credential_ref = profile
        .credential_ref
        .take()
        .map(|reference| CredentialRef(reference.0.trim().into()))
        .filter(|reference| !reference.0.is_empty());
    profile.tags = std::mem::take(&mut profile.tags)
        .into_iter()
        .map(|tag| tag.trim().into())
        .filter(|tag: &String| !tag.is_empty())
        .collect();
}

fn normalize_path(path: PathBuf) -> Option<PathBuf> {
    if let Some(text) = path.to_str() {
        let text = text.trim();
        (!text.is_empty()).then(|| PathBuf::from(text))
    } else {
        Some(path)
    }
}

fn normalize_group_positions(catalog: &mut ConnectionCatalog) {
    let parents = std::iter::once(None)
        .chain(catalog.groups.keys().copied().map(Some))
        .collect::<Vec<_>>();
    for parent in parents {
        let mut groups = catalog
            .groups
            .values()
            .filter(|group| group.parent_id == parent)
            .map(|group| (group.position, group.id))
            .collect::<Vec<_>>();
        groups.sort();
        for (position, (_, id)) in groups.into_iter().enumerate() {
            catalog.groups.get_mut(&id).unwrap().position = usize_to_i64(position);
        }
    }
}

fn normalize_connection_positions(catalog: &mut ConnectionCatalog) {
    let groups = std::iter::once(None)
        .chain(catalog.groups.keys().copied().map(Some))
        .collect::<Vec<_>>();
    for group in groups {
        let mut connections = catalog
            .connections
            .values()
            .filter(|profile| profile.group_id == group)
            .map(|profile| (profile.position, profile.id))
            .collect::<Vec<_>>();
        connections.sort();
        for (position, (_, id)) in connections.into_iter().enumerate() {
            catalog.connections.get_mut(&id).unwrap().position = usize_to_i64(position);
        }
    }
}

fn validate_group_hierarchy(catalog: &ConnectionCatalog) -> Result<(), DomainError> {
    for group in catalog.groups.values() {
        let mut seen = BTreeSet::new();
        let mut current = Some(group.id);
        while let Some(id) = current {
            if !seen.insert(id) {
                return Err(DomainError::GroupCycle {
                    group_id: group.id,
                    parent_id: id,
                });
            }
            current = catalog.groups.get(&id).and_then(|group| group.parent_id);
        }
    }
    Ok(())
}

fn validate_group_positions(catalog: &ConnectionCatalog) -> Result<(), DomainError> {
    for parent in std::iter::once(None).chain(catalog.groups.keys().copied().map(Some)) {
        let mut groups = catalog
            .groups
            .values()
            .filter(|group| group.parent_id == parent)
            .collect::<Vec<_>>();
        groups.sort_by_key(|group| (group.position, group.id));
        for (expected, group) in groups.into_iter().enumerate() {
            let expected = usize_to_i64(expected);
            if group.position != expected {
                return Err(DomainError::InvalidGroupPosition {
                    group_id: group.id,
                    position: group.position,
                    expected,
                });
            }
        }
    }
    Ok(())
}

fn validate_connection_positions(catalog: &ConnectionCatalog) -> Result<(), DomainError> {
    for group in std::iter::once(None).chain(catalog.groups.keys().copied().map(Some)) {
        let mut connections = catalog
            .connections
            .values()
            .filter(|profile| profile.group_id == group)
            .collect::<Vec<_>>();
        connections.sort_by_key(|profile| (profile.position, profile.id));
        for (expected, profile) in connections.into_iter().enumerate() {
            let expected = usize_to_i64(expected);
            if profile.position != expected {
                return Err(DomainError::InvalidConnectionPosition {
                    connection_id: profile.id,
                    position: profile.position,
                    expected,
                });
            }
        }
    }
    Ok(())
}

fn validate_profile(profile: &ConnectionProfile) -> Result<(), DomainError> {
    let host = profile.host.trim();
    if host.is_empty() || host.starts_with('-') {
        return Err(DomainError::InvalidHost {
            host: profile.host.clone(),
        });
    }
    if profile.port == 0 {
        return Err(DomainError::InvalidPort { port: profile.port });
    }
    if !matches!(
        (profile.transport, profile.authentication),
        (
            TransportKind::SystemOpenSsh,
            AuthenticationKind::Agent | AuthenticationKind::PublicKey
        ) | (
            TransportKind::NativeSsh,
            AuthenticationKind::Password
                | AuthenticationKind::PublicKey
                | AuthenticationKind::KeyboardInteractive
        )
    ) {
        return Err(DomainError::InvalidAuthentication {
            transport: profile.transport,
            authentication: profile.authentication,
        });
    }
    if profile.authentication == AuthenticationKind::PublicKey
        && !profile
            .identity_file
            .as_ref()
            .is_some_and(|path| has_path_text(path))
    {
        return Err(DomainError::MissingIdentityFile {
            connection_id: profile.id,
        });
    }
    if profile.authentication == AuthenticationKind::Password
        && profile
            .credential_ref
            .as_ref()
            .is_none_or(|reference| reference.0.trim().is_empty())
    {
        return Err(DomainError::MissingCredentialRef {
            connection_id: profile.id,
        });
    }
    validate_terminal_overrides(&profile.terminal_overrides).map_err(|error| {
        DomainError::InvalidTerminalOverride {
            field: error.field,
            code: error.code,
        }
    })?;
    Ok(())
}

fn has_path_text(path: &Path) -> bool {
    path.to_str()
        .map_or(!path.as_os_str().is_empty(), |text| !text.trim().is_empty())
}

fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
