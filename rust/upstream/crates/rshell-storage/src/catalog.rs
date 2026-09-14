use std::collections::BTreeSet;

use rshell_core::{
    CatalogMutation, CatalogOutcome, ConnectionCatalog, ConnectionGroup, ConnectionProfile,
    CredentialRef,
};
use rusqlite::{Connection, params};

use crate::{StorageError, catalog_delete, error, mapping, transaction, worker::FailureInjector};

pub(crate) fn load(connection: &Connection) -> Result<ConnectionCatalog, StorageError> {
    let mut catalog = ConnectionCatalog::default();
    let mut statement = connection
        .prepare("SELECT id, parent_id, name, position FROM connection_groups")
        .map_err(error::sqlite)?;
    let mut rows = statement.query([]).map_err(error::sqlite)?;
    while let Some(row) = rows.next().map_err(error::sqlite)? {
        let id_text: String = row.get(0).map_err(error::sqlite)?;
        let parent_text: Option<String> = row.get(1).map_err(error::sqlite)?;
        let id = mapping::group_id(&id_text)?;
        let parent_id = parent_text.as_deref().map(mapping::group_id).transpose()?;
        let group = ConnectionGroup {
            id,
            parent_id,
            name: row.get(2).map_err(error::sqlite)?,
            position: row.get(3).map_err(error::sqlite)?,
        };
        if catalog.groups.insert(id, group).is_some() {
            return Err(StorageError::Corrupt);
        }
    }
    drop(rows);
    drop(statement);

    let mut statement = connection
        .prepare(
            "SELECT id, group_id, name, host, port, username, transport, authentication, \
             credential_ref, identity_file, host_key_policy, remote_command, note, position, \
             terminal_profile_id, terminal_overrides_json FROM connections",
        )
        .map_err(error::sqlite)?;
    let mut rows = statement.query([]).map_err(error::sqlite)?;
    while let Some(row) = rows.next().map_err(error::sqlite)? {
        let id_text: String = row.get(0).map_err(error::sqlite)?;
        let group_text: Option<String> = row.get(1).map_err(error::sqlite)?;
        let port: i64 = row.get(4).map_err(error::sqlite)?;
        let transport: String = row.get(6).map_err(error::sqlite)?;
        let authentication: String = row.get(7).map_err(error::sqlite)?;
        let credential_ref: Option<String> = row.get(8).map_err(error::sqlite)?;
        let identity_file: Option<String> = row.get(9).map_err(error::sqlite)?;
        let host_key_policy: String = row.get(10).map_err(error::sqlite)?;
        let terminal_profile: Option<String> = row.get(14).map_err(error::sqlite)?;
        let overrides_json: String = row.get(15).map_err(error::sqlite)?;
        let id = mapping::connection_id(&id_text)?;
        let profile = ConnectionProfile {
            id,
            group_id: group_text.as_deref().map(mapping::group_id).transpose()?,
            name: row.get(2).map_err(error::sqlite)?,
            host: row.get(3).map_err(error::sqlite)?,
            port: u16::try_from(port).map_err(|_| StorageError::Corrupt)?,
            username: row.get(5).map_err(error::sqlite)?,
            transport: mapping::transport(&transport)?,
            authentication: mapping::authentication(&authentication)?,
            credential_ref: credential_ref.map(CredentialRef),
            identity_file: identity_file.map(mapping::stored_path),
            host_key_policy: mapping::host_key(&host_key_policy)?,
            remote_command: row.get(11).map_err(error::sqlite)?,
            note: row.get(12).map_err(error::sqlite)?,
            tags: BTreeSet::new(),
            position: row.get(13).map_err(error::sqlite)?,
            terminal_profile_id: terminal_profile
                .as_deref()
                .map(mapping::profile_id)
                .transpose()?,
            terminal_overrides: mapping::overrides(&overrides_json)?,
        };
        if catalog.connections.insert(id, profile).is_some() {
            return Err(StorageError::Corrupt);
        }
    }
    drop(rows);
    drop(statement);

    let mut statement = connection
        .prepare("SELECT connection_id, tag FROM connection_tags")
        .map_err(error::sqlite)?;
    let mut rows = statement.query([]).map_err(error::sqlite)?;
    while let Some(row) = rows.next().map_err(error::sqlite)? {
        let id: String = row.get(0).map_err(error::sqlite)?;
        let tag: String = row.get(1).map_err(error::sqlite)?;
        catalog
            .connections
            .get_mut(&mapping::connection_id(&id)?)
            .ok_or(StorageError::Corrupt)?
            .tags
            .insert(tag);
    }
    catalog.validate().map_err(|_| StorageError::Corrupt)?;
    Ok(catalog)
}

pub(crate) fn apply(
    connection: &mut Connection,
    failure: &mut FailureInjector,
    mutation: CatalogMutation,
) -> Result<CatalogOutcome, StorageError> {
    transaction::immediate(connection, |transaction| {
        apply_in_transaction(transaction, failure, mutation)
    })
}

/// Task 6 can compose this with credential journal writes inside one worker transaction.
pub(crate) fn apply_in_transaction(
    connection: &Connection,
    failure: &mut FailureInjector,
    mutation: CatalogMutation,
) -> Result<CatalogOutcome, StorageError> {
    let (_, catalog, outcome) = preview_in_transaction(connection, mutation)?;
    persist(connection, &catalog, failure)?;
    Ok(outcome)
}

pub(crate) fn preview_in_transaction(
    connection: &Connection,
    mutation: CatalogMutation,
) -> Result<(ConnectionCatalog, ConnectionCatalog, CatalogOutcome), StorageError> {
    let previous = load(connection)?;
    let mut next = previous.clone();
    let outcome = next.apply(mutation).map_err(|_| StorageError::Constraint)?;
    Ok((previous, next, outcome))
}

pub(crate) fn preview_import_in_transaction(
    connection: &Connection,
    groups: Vec<ConnectionGroup>,
    profiles: Vec<ConnectionProfile>,
) -> Result<(ConnectionCatalog, ConnectionCatalog), StorageError> {
    let previous = load(connection)?;
    let mut next = previous.clone();
    for group in groups {
        next.apply(CatalogMutation::CreateGroup(group))
            .map_err(|_| StorageError::Constraint)?;
    }
    for profile in profiles {
        next.apply(CatalogMutation::Create(profile))
            .map_err(|_| StorageError::Constraint)?;
    }
    Ok((previous, next))
}

pub(crate) fn persist(
    connection: &Connection,
    catalog: &ConnectionCatalog,
    failure: &mut FailureInjector,
) -> Result<(), StorageError> {
    execute(connection, "DELETE FROM connection_tags", [], failure)?;
    execute(connection, "DELETE FROM connections", [], failure)?;
    catalog_delete::groups_leaf_first(connection, failure)?;

    let mut remaining = catalog.groups.clone();
    let mut inserted = BTreeSet::new();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter(|(_, group)| group.parent_id.is_none_or(|id| inserted.contains(&id)))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return Err(StorageError::Corrupt);
        }
        for id in ready {
            let group = remaining.remove(&id).ok_or(StorageError::Corrupt)?;
            let parent = group.parent_id.map(|value| mapping::uuid_text(value.0));
            execute(
                connection,
                "INSERT INTO connection_groups(id, parent_id, name, position) \
                 VALUES(?1, ?2, ?3, ?4)",
                params![
                    mapping::uuid_text(group.id.0),
                    parent,
                    group.name,
                    group.position
                ],
                failure,
            )?;
            inserted.insert(id);
        }
    }

    for profile in catalog.connections.values() {
        let group = profile.group_id.map(|value| mapping::uuid_text(value.0));
        let credential = profile
            .credential_ref
            .as_ref()
            .map(|value| value.0.as_str());
        let identity = profile
            .identity_file
            .as_deref()
            .map(mapping::path_text)
            .transpose()?;
        let terminal_profile = profile
            .terminal_profile_id
            .map(|value| mapping::uuid_text(value.0));
        let overrides = mapping::overrides_json(&profile.terminal_overrides)?;
        execute(
            connection,
            "INSERT INTO connections(id, group_id, name, host, port, username, transport, \
             authentication, credential_ref, identity_file, host_key_policy, remote_command, \
             note, position, terminal_profile_id, terminal_overrides_json) \
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                mapping::uuid_text(profile.id.0),
                group,
                profile.name,
                profile.host,
                i64::from(profile.port),
                profile.username,
                mapping::transport_text(profile.transport),
                mapping::authentication_text(profile.authentication),
                credential,
                identity,
                mapping::host_key_text(profile.host_key_policy),
                profile.remote_command,
                profile.note,
                profile.position,
                terminal_profile,
                overrides,
            ],
            failure,
        )?;
        for tag in &profile.tags {
            execute(
                connection,
                "INSERT INTO connection_tags(connection_id, tag) VALUES(?1, ?2)",
                params![mapping::uuid_text(profile.id.0), tag],
                failure,
            )?;
        }
    }
    Ok(())
}

fn execute<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    parameters: P,
    failure: &mut FailureInjector,
) -> Result<(), StorageError> {
    connection.execute(sql, parameters).map_err(error::sqlite)?;
    failure.after_statement()
}
