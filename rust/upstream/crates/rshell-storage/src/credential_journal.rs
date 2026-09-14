use rshell_core::{
    CatalogMutation, ConnectionCatalog, ConnectionGroup, ConnectionProfile, CredentialRef,
};
use rusqlite::Connection;
use uuid::Uuid;

use crate::{StorageError, worker::FailureInjector};

use crate::credential_transactions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CredentialOperationId(pub(crate) Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialOperationAction {
    PutNew,
    DeleteOld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialOperationState {
    Prepared,
    VaultApplied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialJournalRow {
    pub(crate) operation_id: CredentialOperationId,
    pub(crate) credential_ref: CredentialRef,
    pub(crate) action: CredentialOperationAction,
    pub(crate) state: CredentialOperationState,
    pub(crate) created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CredentialCommit {
    pub(crate) catalog: ConnectionCatalog,
    pub(crate) pending_deletes: Vec<CredentialJournalRow>,
}

pub(crate) enum CredentialCommand {
    PreparePut(CredentialRef),
    MarkApplied(CredentialOperationId),
    FinalizePut {
        operation_id: CredentialOperationId,
        mutation: Box<CatalogMutation>,
    },
    ApplyNoPut(Box<CatalogMutation>),
    List,
    Complete(CredentialOperationId),
    PrepareImport(Vec<CredentialRef>),
    ImportMarkerExists(String),
    FinalizeImport {
        operation_ids: Vec<CredentialOperationId>,
        groups: Vec<ConnectionGroup>,
        profiles: Vec<ConnectionProfile>,
        import_marker: Option<String>,
    },
}

pub(crate) enum CredentialReply {
    Operation(CredentialOperationId),
    Operations(Vec<CredentialOperationId>),
    Commit(CredentialCommit),
    Rows(Vec<CredentialJournalRow>),
    MarkerExists(bool),
    Complete,
}

pub(crate) fn execute(
    connection: &mut Connection,
    failure: &mut FailureInjector,
    command: CredentialCommand,
) -> Result<CredentialReply, StorageError> {
    match command {
        CredentialCommand::PreparePut(reference) => {
            credential_transactions::prepare(connection, failure, vec![reference]).and_then(
                |mut ids| {
                    ids.pop()
                        .map(CredentialReply::Operation)
                        .ok_or(StorageError::Corrupt)
                },
            )
        }
        CredentialCommand::PrepareImport(references) => {
            credential_transactions::prepare(connection, failure, references)
                .map(CredentialReply::Operations)
        }
        CredentialCommand::ImportMarkerExists(marker) => {
            crate::import_metadata::exists(connection, &marker).map(CredentialReply::MarkerExists)
        }
        CredentialCommand::MarkApplied(id) => {
            credential_transactions::mark_applied(connection, failure, id)?;
            Ok(CredentialReply::Complete)
        }
        CredentialCommand::FinalizePut {
            operation_id,
            mutation,
        } => credential_transactions::finalize_put(connection, failure, operation_id, *mutation)
            .map(CredentialReply::Commit),
        CredentialCommand::ApplyNoPut(mutation) => {
            credential_transactions::finalize_no_put(connection, failure, *mutation)
                .map(CredentialReply::Commit)
        }
        CredentialCommand::List => {
            credential_transactions::list(connection).map(CredentialReply::Rows)
        }
        CredentialCommand::Complete(id) => {
            credential_transactions::complete_operation(connection, failure, id)?;
            Ok(CredentialReply::Complete)
        }
        CredentialCommand::FinalizeImport {
            operation_ids,
            groups,
            profiles,
            import_marker,
        } => credential_transactions::finalize_import(
            connection,
            failure,
            operation_ids,
            groups,
            profiles,
            import_marker.as_deref(),
        )
        .map(CredentialReply::Commit),
    }
}

impl CredentialOperationAction {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::PutNew => "put_new",
            Self::DeleteOld => "delete_old",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, rusqlite::Error> {
        match value {
            "put_new" => Ok(Self::PutNew),
            "delete_old" => Ok(Self::DeleteOld),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

impl CredentialOperationState {
    pub(crate) fn parse(value: &str) -> Result<Self, rusqlite::Error> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "vault_applied" => Ok(Self::VaultApplied),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

pub(crate) fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialJournalRow> {
    let id: String = row.get(0)?;
    let action: String = row.get(2)?;
    let state: String = row.get(3)?;
    Ok(CredentialJournalRow {
        operation_id: CredentialOperationId(
            Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        ),
        credential_ref: CredentialRef(row.get(1)?),
        action: CredentialOperationAction::parse(&action)?,
        state: CredentialOperationState::parse(&state)?,
        created_at: row.get(4)?,
    })
}
