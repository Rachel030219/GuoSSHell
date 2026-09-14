use rusqlite::{Error as SqliteError, ErrorCode};
use thiserror::Error;

/// Stable storage failures that never include SQL parameters or stored values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StorageError {
    #[error("storage migration failed")]
    Migration,
    #[error("storage constraint failed")]
    Constraint,
    #[error("storage I/O failed")]
    Io,
    #[error("storage is busy")]
    Busy,
    #[error("storage is corrupt")]
    Corrupt,
    #[error("storage serialization failed")]
    Serialization,
    #[error("storage worker crashed")]
    Crashed,
    #[error("storage queue is closed")]
    QueueClosed,
}

pub(crate) fn sqlite(error: SqliteError) -> StorageError {
    match error {
        SqliteError::SqliteFailure(code, _) => match code.code {
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => StorageError::Busy,
            ErrorCode::ConstraintViolation => StorageError::Constraint,
            ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => StorageError::Corrupt,
            _ => StorageError::Io,
        },
        SqliteError::QueryReturnedNoRows
        | SqliteError::InvalidColumnType(..)
        | SqliteError::InvalidColumnIndex(..)
        | SqliteError::InvalidColumnName(..)
        | SqliteError::IntegralValueOutOfRange(..)
        | SqliteError::Utf8Error(..)
        | SqliteError::FromSqlConversionFailure(..) => StorageError::Corrupt,
        SqliteError::ToSqlConversionFailure(..) => StorageError::Serialization,
        _ => StorageError::Io,
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::{Error, ffi};

    use super::{StorageError, sqlite};

    #[test]
    fn sqlite_primary_codes_have_stable_categories() {
        for (code, expected) in [
            (ffi::SQLITE_BUSY, StorageError::Busy),
            (ffi::SQLITE_CONSTRAINT, StorageError::Constraint),
            (ffi::SQLITE_CORRUPT, StorageError::Corrupt),
        ] {
            let error = Error::SqliteFailure(ffi::Error::new(code), None);
            assert_eq!(sqlite(error), expected);
        }
    }
}
