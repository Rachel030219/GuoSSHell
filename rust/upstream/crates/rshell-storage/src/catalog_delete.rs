use rusqlite::Connection;

use crate::{StorageError, error, worker::FailureInjector};

pub(crate) fn groups_leaf_first(
    connection: &Connection,
    failure: &mut FailureInjector,
) -> Result<(), StorageError> {
    loop {
        let remaining = connection
            .query_row("SELECT COUNT(*) FROM connection_groups", [], |row| {
                row.get::<_, usize>(0)
            })
            .map_err(error::sqlite)?;
        if remaining == 0 {
            return Ok(());
        }
        let deleted = connection
            .execute(
                "DELETE FROM connection_groups WHERE id NOT IN (\
                 SELECT parent_id FROM connection_groups WHERE parent_id IS NOT NULL)",
                [],
            )
            .map_err(error::sqlite)?;
        failure.after_statement()?;
        if deleted == 0 {
            return Err(StorageError::Corrupt);
        }
    }
}
