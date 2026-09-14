use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::{StorageError, error};

/// Keeps every composable storage operation inside the worker-owned connection.
pub(crate) fn immediate<T>(
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error::sqlite)?;
    let result = operation(&transaction)?;
    transaction.commit().map_err(error::sqlite)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::immediate;
    use crate::StorageError;

    #[test]
    fn failed_immediate_operation_rolls_back() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute("CREATE TABLE sample(value INTEGER)", [])
            .unwrap();
        let result = immediate(&mut connection, |transaction| {
            transaction
                .execute("INSERT INTO sample VALUES(1)", [])
                .unwrap();
            Err::<(), _>(StorageError::Constraint)
        });
        assert_eq!(result, Err(StorageError::Constraint));
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM sample", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
