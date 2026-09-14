use rshell_core::ImportError;

use crate::{
    CredentialOperationError as StorageCredentialError, ImportError as StorageImportError,
};

pub(super) fn map_import(error: StorageImportError) -> ImportError {
    match error {
        StorageImportError::Io | StorageImportError::NoUsableSource => ImportError::Read,
        StorageImportError::InvalidJson
        | StorageImportError::IncludeCycle
        | StorageImportError::IncludeDepth => ImportError::Parse,
        StorageImportError::InvalidUuid
        | StorageImportError::InvalidPort
        | StorageImportError::InvalidConnection
        | StorageImportError::InvalidSelection => ImportError::Validation,
        StorageImportError::IdConflict => ImportError::Conflict,
        StorageImportError::AlreadyImported => ImportError::AlreadyImported,
        StorageImportError::Credential(error) => map_credential(error),
    }
}

fn map_credential(error: StorageCredentialError) -> ImportError {
    match error {
        StorageCredentialError::Validation => ImportError::Validation,
        StorageCredentialError::Conflict => ImportError::Conflict,
        StorageCredentialError::AlreadyImported => ImportError::AlreadyImported,
        StorageCredentialError::Vault => ImportError::Vault,
        StorageCredentialError::Storage => ImportError::Storage,
        StorageCredentialError::ReconciliationRequired
        | StorageCredentialError::InjectedCrash(_) => ImportError::ReconciliationRequired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CrashPoint;

    #[test]
    fn storage_import_errors_map_to_stable_core_categories() {
        let cases = [
            (StorageImportError::Io, ImportError::Read),
            (StorageImportError::NoUsableSource, ImportError::Read),
            (StorageImportError::InvalidJson, ImportError::Parse),
            (StorageImportError::IncludeCycle, ImportError::Parse),
            (StorageImportError::IncludeDepth, ImportError::Parse),
            (StorageImportError::InvalidUuid, ImportError::Validation),
            (StorageImportError::InvalidPort, ImportError::Validation),
            (
                StorageImportError::InvalidConnection,
                ImportError::Validation,
            ),
            (
                StorageImportError::InvalidSelection,
                ImportError::Validation,
            ),
            (StorageImportError::IdConflict, ImportError::Conflict),
            (
                StorageImportError::AlreadyImported,
                ImportError::AlreadyImported,
            ),
            (
                StorageImportError::Credential(StorageCredentialError::Vault),
                ImportError::Vault,
            ),
            (
                StorageImportError::Credential(StorageCredentialError::Storage),
                ImportError::Storage,
            ),
            (
                StorageImportError::Credential(StorageCredentialError::ReconciliationRequired),
                ImportError::ReconciliationRequired,
            ),
            (
                StorageImportError::Credential(StorageCredentialError::InjectedCrash(
                    CrashPoint::AfterPrepare,
                )),
                ImportError::ReconciliationRequired,
            ),
        ];
        for (source, expected) in cases {
            assert_eq!(map_import(source), expected);
        }
    }
}
