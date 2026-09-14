use std::sync::mpsc::SyncSender;

#[cfg(feature = "test-support")]
use std::collections::BTreeMap;

use rshell_core::{
    AppSettings, CatalogMutation, CatalogOutcome, ConnectionCatalog, TerminalProfile,
};
#[cfg(feature = "test-support")]
use rshell_core::{ConnectionId, TerminalProfileId};

use crate::{DatabaseStatus, StorageError};

pub(crate) use crate::credential_journal::{CredentialCommand, CredentialReply};

pub(crate) type Reply<T> = SyncSender<Result<T, StorageError>>;

pub(crate) enum Command {
    Migrate(Reply<()>),
    SchemaVersions(Reply<Vec<i64>>),
    LoadCatalog(Reply<ConnectionCatalog>),
    Apply(Box<CatalogMutation>, Reply<CatalogOutcome>),
    LoadTerminalProfiles(Reply<Vec<TerminalProfile>>),
    SaveTerminalProfile(TerminalProfile, Reply<()>),
    LoadSettings(Reply<AppSettings>),
    SaveSettings(AppSettings, Reply<()>),
    DatabaseStatus(Reply<DatabaseStatus>),
    Credential(CredentialCommand, Reply<CredentialReply>),
    Shutdown(Reply<()>),
    #[cfg(feature = "test-support")]
    TestSchema(Reply<BTreeMap<String, String>>),
    #[cfg(feature = "test-support")]
    TestCredentialOperation(bool, bool, Reply<()>),
    #[cfg(feature = "test-support")]
    TestDeleteTerminalProfile(TerminalProfileId, Reply<()>),
    #[cfg(feature = "test-support")]
    TestDeleteConnectionOnly(ConnectionId, Reply<usize>),
    #[cfg(feature = "test-support")]
    TestCorruptConnection(ConnectionId, u8, Reply<()>),
    #[cfg(feature = "test-support")]
    InjectStatementFailure(usize, Reply<()>),
    #[cfg(feature = "test-support")]
    TestVisibleTables(Reply<Vec<u8>>),
    #[cfg(feature = "test-support")]
    TestCrash(Reply<()>),
}
