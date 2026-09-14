//! SQLite-backed persistence for rsHell.

mod catalog;
mod catalog_delete;
mod command;
mod credential_import;
mod credential_journal;
mod credential_mutation;
mod credential_transactions;
mod credential_types;
mod credentials;
mod database;
mod error;
mod import;
mod import_metadata;
mod mapping;
mod memory_vault;
mod migrations;
pub mod ports;
mod profiles;
#[cfg(feature = "test-support")]
mod test_support;
mod transaction;
mod vault;
mod worker;

pub use credentials::{
    CrashPoint, CredentialCoordinator, CredentialImportBatch, CredentialImportItem,
    CredentialOperationError, ReconcileReport,
};
pub use database::{DatabaseStatus, DatabaseWorker, SqliteRepository};
pub use error::StorageError;
pub use import::{
    ImportConnectionCandidate, ImportError, ImportPreview, ImportReport, ImportWarning,
    LegacyJsonImporter, OpenSshCandidate, OpenSshConfigImporter, OpenSshPreview,
};
#[cfg(feature = "test-support")]
pub use test_support::{
    TestConnectionCorruption, TestCredentialValue, inject_next_credential_reference,
};
pub use vault::{
    CredentialVault, MemoryCredentialVault, MemoryVaultCallCounts, MemoryVaultFault,
    SYSTEM_CREDENTIAL_SERVICE, SystemCredentialVault, VaultError, VaultMutation, VaultOperation,
};
