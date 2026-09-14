use std::collections::BTreeMap;

use rshell_core::{ConnectionId, TerminalProfileId};
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::{SqliteRepository, StorageError, command::Command, error, mapping, transaction};

#[doc(hidden)]
pub fn inject_next_credential_reference(reference: rshell_core::CredentialRef) {
    crate::credential_mutation::set_next_reference(reference);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestCredentialValue {
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestConnectionCorruption {
    UnknownTransport,
    UnsupportedOverridesVersion,
}

impl SqliteRepository {
    #[doc(hidden)]
    pub fn test_schema(&self) -> Result<BTreeMap<String, String>, StorageError> {
        self.worker.request(Command::TestSchema)
    }

    #[doc(hidden)]
    pub fn test_credential_operation(
        &self,
        action: TestCredentialValue,
        state: TestCredentialValue,
    ) -> Result<(), StorageError> {
        self.worker.request(|reply| {
            Command::TestCredentialOperation(
                action == TestCredentialValue::Valid,
                state == TestCredentialValue::Valid,
                reply,
            )
        })
    }

    #[doc(hidden)]
    pub fn test_delete_terminal_profile(&self, id: TerminalProfileId) -> Result<(), StorageError> {
        self.worker
            .request(|reply| Command::TestDeleteTerminalProfile(id, reply))
    }

    #[doc(hidden)]
    pub fn test_delete_connection_only(&self, id: ConnectionId) -> Result<usize, StorageError> {
        self.worker
            .request(|reply| Command::TestDeleteConnectionOnly(id, reply))
    }

    #[doc(hidden)]
    pub fn test_corrupt_connection(
        &self,
        id: ConnectionId,
        corruption: TestConnectionCorruption,
    ) -> Result<(), StorageError> {
        let kind = match corruption {
            TestConnectionCorruption::UnknownTransport => 1,
            TestConnectionCorruption::UnsupportedOverridesVersion => 2,
        };
        self.worker
            .request(|reply| Command::TestCorruptConnection(id, kind, reply))
    }

    #[doc(hidden)]
    pub fn inject_statement_failure_once(&self, statement: usize) -> Result<(), StorageError> {
        self.worker
            .request(|reply| Command::InjectStatementFailure(statement, reply))
    }

    #[doc(hidden)]
    pub fn test_visible_tables(&self) -> Result<Vec<u8>, StorageError> {
        self.worker.request(Command::TestVisibleTables)
    }

    #[doc(hidden)]
    pub fn test_crash_worker(&self) -> Result<(), StorageError> {
        self.worker.request(Command::TestCrash)
    }
}

pub(crate) fn schema(connection: &Connection) -> Result<BTreeMap<String, String>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT name, sql FROM sqlite_master WHERE type IN ('table', 'index') \
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(error::sqlite)?;
    let mut rows = statement.query([]).map_err(error::sqlite)?;
    let mut schema = BTreeMap::new();
    while let Some(row) = rows.next().map_err(error::sqlite)? {
        schema.insert(
            row.get(0).map_err(error::sqlite)?,
            row.get(1).map_err(error::sqlite)?,
        );
    }
    Ok(schema)
}

pub(crate) fn credential_operation(
    connection: &mut Connection,
    valid_action: bool,
    valid_state: bool,
) -> Result<(), StorageError> {
    transaction::immediate(connection, |transaction| {
        transaction
            .execute(
                "INSERT INTO credential_operations(\
                 operation_id, credential_ref, action, state, created_at) \
                 VALUES(?1, 'credential://test', ?2, ?3, 'test-time')",
                params![
                    mapping::uuid_text(Uuid::new_v4()),
                    if valid_action { "put_new" } else { "invalid" },
                    if valid_state { "prepared" } else { "invalid" },
                ],
            )
            .map_err(error::sqlite)?;
        Ok(())
    })
}

pub(crate) fn delete_terminal_profile(
    connection: &mut Connection,
    id: TerminalProfileId,
) -> Result<(), StorageError> {
    transaction::immediate(connection, |transaction| {
        transaction
            .execute(
                "DELETE FROM terminal_profiles WHERE id=?1",
                [mapping::uuid_text(id.0)],
            )
            .map_err(error::sqlite)?;
        Ok(())
    })
}

pub(crate) fn delete_connection_only(
    connection: &mut Connection,
    id: ConnectionId,
) -> Result<usize, StorageError> {
    transaction::immediate(connection, |transaction| {
        let id = mapping::uuid_text(id.0);
        transaction
            .execute("DELETE FROM connections WHERE id=?1", [&id])
            .map_err(error::sqlite)?;
        transaction
            .query_row(
                "SELECT COUNT(*) FROM connection_tags WHERE connection_id=?1",
                [&id],
                |row| row.get(0),
            )
            .map_err(error::sqlite)
    })
}

pub(crate) fn corrupt_connection(
    connection: &mut Connection,
    id: ConnectionId,
    kind: u8,
) -> Result<(), StorageError> {
    let sql = match kind {
        1 => "UPDATE connections SET transport='future_transport' WHERE id=?1",
        2 => "UPDATE connections SET terminal_overrides_json='{\"version\":2}' WHERE id=?1",
        _ => return Err(StorageError::Constraint),
    };
    transaction::immediate(connection, |transaction| {
        transaction
            .execute(sql, [mapping::uuid_text(id.0)])
            .map_err(error::sqlite)?;
        Ok(())
    })
}

pub(crate) fn visible_tables(connection: &Connection) -> Result<Vec<u8>, StorageError> {
    const QUERIES: &[(&str, &str)] = &[
        (
            "schema_migrations",
            "SELECT quote(version)||'|'||quote(applied_at) FROM schema_migrations ORDER BY version",
        ),
        (
            "connection_groups",
            "SELECT quote(id)||'|'||ifnull(quote(parent_id),'NULL')||'|'||quote(name)||'|'||quote(position) FROM connection_groups ORDER BY id",
        ),
        (
            "connections",
            "SELECT quote(id)||'|'||ifnull(quote(group_id),'NULL')||'|'||quote(name)||'|'||quote(host)||'|'||quote(port)||'|'||quote(username)||'|'||quote(transport)||'|'||quote(authentication)||'|'||ifnull(quote(credential_ref),'NULL')||'|'||ifnull(quote(identity_file),'NULL')||'|'||quote(host_key_policy)||'|'||ifnull(quote(remote_command),'NULL')||'|'||quote(note)||'|'||quote(position)||'|'||ifnull(quote(terminal_profile_id),'NULL')||'|'||quote(terminal_overrides_json) FROM connections ORDER BY id",
        ),
        (
            "connection_tags",
            "SELECT quote(connection_id)||'|'||quote(tag) FROM connection_tags ORDER BY connection_id, tag",
        ),
        (
            "terminal_profiles",
            "SELECT quote(id)||'|'||quote(name)||'|'||quote(settings_json) FROM terminal_profiles ORDER BY id",
        ),
        (
            "app_settings",
            "SELECT quote(singleton)||'|'||quote(default_terminal_profile)||'|'||quote(color_scheme)||'|'||quote(key_bindings_json) FROM app_settings ORDER BY singleton",
        ),
        (
            "app_setting_values",
            "SELECT quote(key)||'|'||quote(value) FROM app_setting_values ORDER BY key",
        ),
        (
            "credential_operations",
            "SELECT quote(operation_id)||'|'||quote(credential_ref)||'|'||quote(action)||'|'||quote(state)||'|'||quote(created_at) FROM credential_operations ORDER BY operation_id",
        ),
    ];
    let mut output = Vec::new();
    for (table, query) in QUERIES {
        output.extend_from_slice(table.as_bytes());
        output.push(b'\n');
        let mut statement = connection.prepare(query).map_err(error::sqlite)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(error::sqlite)?;
        for row in rows {
            output.extend_from_slice(row.map_err(error::sqlite)?.as_bytes());
            output.push(b'\n');
        }
    }
    Ok(output)
}
