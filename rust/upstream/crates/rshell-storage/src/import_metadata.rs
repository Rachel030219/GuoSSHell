use rusqlite::Connection;

use crate::{StorageError, error, worker::FailureInjector};

pub(crate) fn exists(connection: &Connection, marker: &str) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM app_setting_values WHERE key=?1)",
            [marker],
            |row| row.get::<_, bool>(0),
        )
        .map_err(error::sqlite)
}

pub(crate) fn insert(
    connection: &Connection,
    failure: &mut FailureInjector,
    marker: &str,
) -> Result<(), StorageError> {
    connection
        .execute(
            "INSERT INTO app_setting_values(key, value) VALUES(?1, 'legacy-json')",
            [marker],
        )
        .map_err(error::sqlite)?;
    failure.after_statement()
}
