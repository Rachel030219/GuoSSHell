use rusqlite::{Connection, OptionalExtension};

use crate::{StorageError, error, transaction};

const INITIAL: &str = include_str!("../migrations/0001_initial.sql");
const IMPORT_METADATA: &str = include_str!("../migrations/0002_import_metadata.sql");

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), StorageError> {
    transaction::immediate(connection, |transaction| {
        transaction
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations(\
                 version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            )
            .map_err(error::sqlite)?;
        let versions = versions(transaction)?;
        match versions.as_slice() {
            [] => {
                transaction.execute_batch(INITIAL).map_err(error::sqlite)?;
                record(transaction, 1)?;
                apply_import_metadata(transaction)
            }
            [1] => apply_import_metadata(transaction),
            [1, 2] => Ok(()),
            _ => Err(StorageError::Migration),
        }
    })
}

fn apply_import_metadata(connection: &Connection) -> Result<(), StorageError> {
    connection
        .execute_batch(IMPORT_METADATA)
        .map_err(error::sqlite)?;
    record(connection, 2)
}

fn record(connection: &Connection, version: i64) -> Result<(), StorageError> {
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) \
             VALUES (?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            [version],
        )
        .map_err(error::sqlite)?;
    Ok(())
}

pub(crate) fn versions(connection: &Connection) -> Result<Vec<i64>, StorageError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(error::sqlite)?
        .is_some();
    if !exists {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .map_err(error::sqlite)?;
    let rows = statement
        .query_map([], |row| row.get(0))
        .map_err(error::sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(error::sqlite)
}
