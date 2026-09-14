use std::collections::BTreeSet;

use rshell_core::{
    CatalogMutation, ConnectionCatalog, ConnectionGroup, ConnectionProfile, CredentialRef,
};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::{
    StorageError, catalog, error, import_metadata, mapping, transaction, worker::FailureInjector,
};

use super::credential_journal::{
    CredentialCommit, CredentialJournalRow, CredentialOperationAction, CredentialOperationId,
    CredentialOperationState, row,
};

pub(crate) fn prepare(
    connection: &mut Connection,
    failure: &mut FailureInjector,
    references: Vec<CredentialRef>,
) -> Result<Vec<CredentialOperationId>, StorageError> {
    transaction::immediate(connection, |tx| {
        references
            .into_iter()
            .map(|reference| insert(tx, failure, reference, CredentialOperationAction::PutNew))
            .map(|row| row.map(|row| row.operation_id))
            .collect()
    })
}

pub(crate) fn mark_applied(
    connection: &mut Connection,
    failure: &mut FailureInjector,
    operation_id: CredentialOperationId,
) -> Result<(), StorageError> {
    transaction::immediate(connection, |tx| mark(tx, failure, operation_id))
}

pub(crate) fn finalize_put(
    connection: &mut Connection,
    failure: &mut FailureInjector,
    operation_id: CredentialOperationId,
    mutation: CatalogMutation,
) -> Result<CredentialCommit, StorageError> {
    transaction::immediate(connection, |tx| {
        let row = operation(tx, operation_id)?;
        if row.action != CredentialOperationAction::PutNew
            || row.state != CredentialOperationState::VaultApplied
            || !matches_put(&mutation, &row.credential_ref)
        {
            return Err(StorageError::Constraint);
        }
        let (old, catalog, _) = catalog::preview_in_transaction(tx, mutation)?;
        catalog::persist(tx, &catalog, failure)?;
        complete(tx, failure, operation_id)?;
        Ok(CredentialCommit {
            pending_deletes: enqueue_deletes(tx, failure, &old, &catalog)?,
            catalog,
        })
    })
}

pub(crate) fn finalize_no_put(
    connection: &mut Connection,
    failure: &mut FailureInjector,
    mutation: CatalogMutation,
) -> Result<CredentialCommit, StorageError> {
    transaction::immediate(connection, |tx| {
        let (old, catalog, _) = catalog::preview_in_transaction(tx, mutation)?;
        catalog::persist(tx, &catalog, failure)?;
        Ok(CredentialCommit {
            pending_deletes: enqueue_deletes(tx, failure, &old, &catalog)?,
            catalog,
        })
    })
}

pub(crate) fn finalize_import(
    connection: &mut Connection,
    failure: &mut FailureInjector,
    operation_ids: Vec<CredentialOperationId>,
    groups: Vec<ConnectionGroup>,
    profiles: Vec<ConnectionProfile>,
    import_marker: Option<&str>,
) -> Result<CredentialCommit, StorageError> {
    transaction::immediate(connection, |tx| {
        let count = operation_ids.len();
        let ids = operation_ids.into_iter().collect::<BTreeSet<_>>();
        if ids.len() != count {
            return Err(StorageError::Constraint);
        }
        let rows = ids
            .iter()
            .map(|id| operation(tx, *id))
            .collect::<Result<Vec<_>, _>>()?;
        if rows.iter().any(|row| {
            row.action != CredentialOperationAction::PutNew
                || row.state != CredentialOperationState::VaultApplied
                || !profiles
                    .iter()
                    .any(|profile| profile.credential_ref.as_ref() == Some(&row.credential_ref))
        }) {
            return Err(StorageError::Constraint);
        }
        let (old, catalog) = catalog::preview_import_in_transaction(tx, groups, profiles)?;
        catalog::persist(tx, &catalog, failure)?;
        if let Some(marker) = import_marker {
            import_metadata::insert(tx, failure, marker)?;
        }
        for id in ids {
            complete(tx, failure, id)?;
        }
        Ok(CredentialCommit {
            pending_deletes: enqueue_deletes(tx, failure, &old, &catalog)?,
            catalog,
        })
    })
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<CredentialJournalRow>, StorageError> {
    let mut statement = connection
        .prepare("SELECT operation_id, credential_ref, action, state, created_at FROM credential_operations ORDER BY created_at, operation_id")
        .map_err(error::sqlite)?;
    statement
        .query_map([], row)
        .map_err(error::sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error::sqlite)
}

pub(crate) fn complete_operation(
    connection: &mut Connection,
    failure: &mut FailureInjector,
    operation_id: CredentialOperationId,
) -> Result<(), StorageError> {
    transaction::immediate(connection, |tx| complete(tx, failure, operation_id))
}

fn operation(
    connection: &Connection,
    operation_id: CredentialOperationId,
) -> Result<CredentialJournalRow, StorageError> {
    connection
        .query_row(
            "SELECT operation_id, credential_ref, action, state, created_at FROM credential_operations WHERE operation_id=?1",
            [mapping::uuid_text(operation_id.0)],
            row,
        )
        .optional()
        .map_err(error::sqlite)?
        .ok_or(StorageError::Constraint)
}

fn insert(
    connection: &Connection,
    failure: &mut FailureInjector,
    credential_ref: CredentialRef,
    action: CredentialOperationAction,
) -> Result<CredentialJournalRow, StorageError> {
    let operation_id = CredentialOperationId(Uuid::new_v4());
    let created_at = connection
        .query_row(
            "INSERT INTO credential_operations(operation_id, credential_ref, action, state, created_at) \
             VALUES(?1, ?2, ?3, 'prepared', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) RETURNING created_at",
            params![mapping::uuid_text(operation_id.0), credential_ref.0.as_str(), action.text()],
            |row| row.get(0),
        )
        .map_err(error::sqlite)?;
    failure.after_statement()?;
    Ok(CredentialJournalRow {
        operation_id,
        credential_ref,
        action,
        state: CredentialOperationState::Prepared,
        created_at,
    })
}

fn mark(
    connection: &Connection,
    failure: &mut FailureInjector,
    operation_id: CredentialOperationId,
) -> Result<(), StorageError> {
    let changed = connection
        .execute(
            "UPDATE credential_operations SET state='vault_applied' \
             WHERE operation_id=?1 AND action='put_new' AND state='prepared'",
            [mapping::uuid_text(operation_id.0)],
        )
        .map_err(error::sqlite)?;
    failure.after_statement()?;
    (changed == 1).then_some(()).ok_or(StorageError::Constraint)
}

fn complete(
    connection: &Connection,
    failure: &mut FailureInjector,
    operation_id: CredentialOperationId,
) -> Result<(), StorageError> {
    let changed = connection
        .execute(
            "DELETE FROM credential_operations WHERE operation_id=?1",
            [mapping::uuid_text(operation_id.0)],
        )
        .map_err(error::sqlite)?;
    failure.after_statement()?;
    (changed == 1).then_some(()).ok_or(StorageError::Constraint)
}

fn enqueue_deletes(
    connection: &Connection,
    failure: &mut FailureInjector,
    old: &ConnectionCatalog,
    new: &ConnectionCatalog,
) -> Result<Vec<CredentialJournalRow>, StorageError> {
    references(old)
        .difference(&references(new))
        .cloned()
        .map(CredentialRef)
        .map(|reference| {
            insert(
                connection,
                failure,
                reference,
                CredentialOperationAction::DeleteOld,
            )
        })
        .collect()
}

fn references(catalog: &ConnectionCatalog) -> BTreeSet<String> {
    catalog
        .connections
        .values()
        .filter_map(|profile| profile.credential_ref.as_ref())
        .map(|reference| reference.0.clone())
        .collect()
}

fn matches_put(mutation: &CatalogMutation, credential_ref: &CredentialRef) -> bool {
    match mutation {
        CatalogMutation::Create(profile) | CatalogMutation::Update(profile) => {
            profile.credential_ref.as_ref() == Some(credential_ref)
        }
        _ => false,
    }
}
