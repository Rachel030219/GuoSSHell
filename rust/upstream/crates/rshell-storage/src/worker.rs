use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, SyncSender},
    time::Duration,
};

use rusqlite::Connection;

use crate::{
    DatabaseStatus, StorageError, catalog, command::Command, database::DatabaseSource, error,
    migrations, profiles,
};

pub(crate) struct FailureInjector {
    #[cfg(feature = "test-support")]
    remaining: Option<usize>,
}

impl FailureInjector {
    fn new() -> Self {
        Self {
            #[cfg(feature = "test-support")]
            remaining: None,
        }
    }

    pub(crate) fn after_statement(&mut self) -> Result<(), StorageError> {
        #[cfg(feature = "test-support")]
        if let Some(remaining) = self.remaining.as_mut() {
            if *remaining <= 1 {
                self.remaining = None;
                return Err(StorageError::Constraint);
            }
            *remaining -= 1;
        }
        Ok(())
    }

    #[cfg(feature = "test-support")]
    fn arm(&mut self, statement: usize) -> Result<(), StorageError> {
        if statement == 0 {
            return Err(StorageError::Constraint);
        }
        self.remaining = Some(statement);
        Ok(())
    }
}

pub(crate) fn run(
    source: DatabaseSource,
    receiver: Receiver<Command>,
    ready: SyncSender<Result<(), StorageError>>,
) {
    let (mut connection, path) = match open(source) {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }
    let mut failure = FailureInjector::new();
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Migrate(reply) => send(reply, migrations::migrate(&mut connection)),
            Command::SchemaVersions(reply) => send(reply, migrations::versions(&connection)),
            Command::LoadCatalog(reply) => send(reply, catalog::load(&connection)),
            Command::Apply(mutation, reply) => {
                send(
                    reply,
                    catalog::apply(&mut connection, &mut failure, *mutation),
                );
            }
            Command::LoadTerminalProfiles(reply) => {
                send(reply, profiles::load_profiles(&connection));
            }
            Command::SaveTerminalProfile(profile, reply) => {
                send(reply, profiles::save_profile(&mut connection, profile));
            }
            Command::LoadSettings(reply) => send(reply, profiles::load_settings(&connection)),
            Command::SaveSettings(settings, reply) => {
                send(reply, profiles::save_settings(&mut connection, settings));
            }
            Command::DatabaseStatus(reply) => send(reply, status(&connection, path.as_deref())),
            Command::Credential(command, reply) => send(
                reply,
                crate::credential_journal::execute(&mut connection, &mut failure, command),
            ),
            Command::Shutdown(reply) => {
                send(reply, checkpoint(&connection, path.is_some()));
                break;
            }
            #[cfg(feature = "test-support")]
            Command::TestSchema(reply) => {
                send(reply, crate::test_support::schema(&connection));
            }
            #[cfg(feature = "test-support")]
            Command::TestCredentialOperation(action, state, reply) => {
                send(
                    reply,
                    crate::test_support::credential_operation(&mut connection, action, state),
                );
            }
            #[cfg(feature = "test-support")]
            Command::TestDeleteTerminalProfile(id, reply) => {
                send(
                    reply,
                    crate::test_support::delete_terminal_profile(&mut connection, id),
                );
            }
            #[cfg(feature = "test-support")]
            Command::TestDeleteConnectionOnly(id, reply) => {
                send(
                    reply,
                    crate::test_support::delete_connection_only(&mut connection, id),
                );
            }
            #[cfg(feature = "test-support")]
            Command::TestCorruptConnection(id, kind, reply) => {
                send(
                    reply,
                    crate::test_support::corrupt_connection(&mut connection, id, kind),
                );
            }
            #[cfg(feature = "test-support")]
            Command::InjectStatementFailure(statement, reply) => {
                send(reply, failure.arm(statement));
            }
            #[cfg(feature = "test-support")]
            Command::TestVisibleTables(reply) => {
                send(reply, crate::test_support::visible_tables(&connection));
            }
            #[cfg(feature = "test-support")]
            Command::TestCrash(_reply) => panic!("injected storage worker crash"),
        }
    }
}

fn open(source: DatabaseSource) -> Result<(Connection, Option<PathBuf>), StorageError> {
    let (connection, path, file) = match source {
        DatabaseSource::File(path) => (
            Connection::open(&path).map_err(error::sqlite)?,
            Some(path),
            true,
        ),
        DatabaseSource::Memory => (
            Connection::open_in_memory().map_err(error::sqlite)?,
            None,
            false,
        ),
    };
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(error::sqlite)?;
    connection
        .busy_timeout(Duration::from_millis(5_000))
        .map_err(error::sqlite)?;
    if file {
        connection
            .query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
            .map_err(error::sqlite)?;
    }
    Ok((connection, path))
}

fn status(
    connection: &Connection,
    path: Option<&std::path::Path>,
) -> Result<DatabaseStatus, StorageError> {
    let foreign_keys = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
        .map_err(error::sqlite)?
        == 1;
    let journal_mode = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(error::sqlite)?;
    let busy_timeout_ms = connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, u64>(0))
        .map_err(error::sqlite)?;
    let private_file_is_secure = path
        .map(rshell_platform::private_file_is_secure)
        .transpose()
        .map_err(|_| StorageError::Io)?;
    Ok(DatabaseStatus {
        foreign_keys,
        journal_mode,
        busy_timeout_ms,
        private_file_is_secure,
    })
}

fn checkpoint(connection: &Connection, file: bool) -> Result<(), StorageError> {
    if file {
        connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .map_err(error::sqlite)?;
    }
    Ok(())
}

fn send<T>(reply: SyncSender<Result<T, StorageError>>, result: Result<T, StorageError>) {
    let _ = reply.send(result);
}
