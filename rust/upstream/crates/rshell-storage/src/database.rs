use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    thread::{self, JoinHandle},
};

use rshell_core::{
    AppSettings, CatalogMutation, CatalogOutcome, ConnectionCatalog, TerminalProfile,
};
use rshell_platform::{create_private_file, harden_private_file};

use crate::{
    StorageError,
    command::{Command, CredentialCommand, CredentialReply},
    worker,
};

const QUEUE_CAPACITY: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseStatus {
    pub foreign_keys: bool,
    pub journal_mode: String,
    pub busy_timeout_ms: u64,
    pub private_file_is_secure: Option<bool>,
}

pub(crate) enum DatabaseSource {
    File(PathBuf),
    Memory,
}

pub struct DatabaseWorker {
    sender: SyncSender<Command>,
    join: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}

impl DatabaseWorker {
    pub(crate) fn start(source: DatabaseSource) -> Result<Self, StorageError> {
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        let (ready_sender, ready_receiver) = sync_channel(1);
        let join = thread::Builder::new()
            .name("rshell-storage".into())
            .spawn(move || worker::run(source, receiver, ready_sender))
            .map_err(|_| StorageError::Io)?;
        let worker = Self {
            sender,
            join: Mutex::new(Some(join)),
            closed: AtomicBool::new(false),
        };
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(worker),
            Ok(Err(error)) => {
                worker.join_thread();
                Err(error)
            }
            Err(_) => {
                worker.join_thread();
                Err(StorageError::Crashed)
            }
        }
    }

    pub(crate) fn request<T>(
        &self,
        build: impl FnOnce(SyncSender<Result<T, StorageError>>) -> Command,
    ) -> Result<T, StorageError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(StorageError::QueueClosed);
        }
        let (reply, receive) = sync_channel(1);
        self.sender
            .send(build(reply))
            .map_err(|_| StorageError::Crashed)?;
        receive.recv().map_err(|_| StorageError::Crashed)?
    }

    pub fn shutdown(&self) -> Result<(), StorageError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let (reply, receive) = sync_channel(1);
        let result = self
            .sender
            .send(Command::Shutdown(reply))
            .map_err(|_| StorageError::Crashed)
            .and_then(|()| receive.recv().map_err(|_| StorageError::Crashed)?);
        self.join_thread();
        result
    }

    fn join_thread(&self) {
        if let Some(join) = self
            .join
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = join.join();
        }
    }
}

impl Drop for DatabaseWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub struct SqliteRepository {
    pub(crate) worker: DatabaseWorker,
}

impl SqliteRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|_| StorageError::Io)?;
        }
        if path.exists() {
            harden_private_file(path).map_err(|_| StorageError::Io)?;
        } else if create_private_file(path).is_err() {
            if path.exists() {
                harden_private_file(path).map_err(|_| StorageError::Io)?;
            } else {
                return Err(StorageError::Io);
            }
        }
        Ok(Self {
            worker: DatabaseWorker::start(DatabaseSource::File(path.to_path_buf()))?,
        })
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        Ok(Self {
            worker: DatabaseWorker::start(DatabaseSource::Memory)?,
        })
    }

    pub fn migrate(&self) -> Result<(), StorageError> {
        self.worker.request(Command::Migrate)
    }

    pub fn schema_versions(&self) -> Result<Vec<i64>, StorageError> {
        self.worker.request(Command::SchemaVersions)
    }

    pub fn load_catalog(&self) -> Result<ConnectionCatalog, StorageError> {
        self.worker.request(Command::LoadCatalog)
    }

    pub fn apply(&self, mutation: CatalogMutation) -> Result<CatalogOutcome, StorageError> {
        self.worker
            .request(|reply| Command::Apply(Box::new(mutation), reply))
    }

    pub fn load_terminal_profiles(&self) -> Result<Vec<TerminalProfile>, StorageError> {
        self.worker.request(Command::LoadTerminalProfiles)
    }

    pub fn save_terminal_profile(&self, profile: TerminalProfile) -> Result<(), StorageError> {
        self.worker
            .request(|reply| Command::SaveTerminalProfile(profile, reply))
    }

    pub fn load_settings(&self) -> Result<AppSettings, StorageError> {
        self.worker.request(Command::LoadSettings)
    }

    pub fn save_settings(&self, settings: AppSettings) -> Result<(), StorageError> {
        self.worker
            .request(|reply| Command::SaveSettings(settings, reply))
    }

    pub fn database_status(&self) -> Result<DatabaseStatus, StorageError> {
        self.worker.request(Command::DatabaseStatus)
    }

    pub(crate) fn credential_operation(
        &self,
        command: CredentialCommand,
    ) -> Result<CredentialReply, StorageError> {
        self.worker
            .request(|reply| Command::Credential(command, reply))
    }

    pub fn shutdown(&self) -> Result<(), StorageError> {
        self.worker.shutdown()
    }
}
