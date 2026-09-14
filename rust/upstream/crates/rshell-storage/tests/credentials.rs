use std::{fs, path::PathBuf, sync::Arc};

use rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionGroup, ConnectionProfile, CredentialRef,
    SecretUpdate, TransportKind,
};
use rshell_storage::{
    CrashPoint, CredentialCoordinator, CredentialImportBatch, CredentialImportItem,
    CredentialOperationError, CredentialVault, MemoryCredentialVault, MemoryVaultFault,
    SqliteRepository, VaultError, VaultMutation, VaultOperation,
};
use secrecy::{ExposeSecret, SecretString};

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

fn password_profile(name: &str) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new(name, format!("{name}.example.test"));
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;
    profile.username = "operator".into();
    profile
}

fn public_key_profile(name: &str) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new(name, format!("{name}.example.test"));
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::PublicKey;
    profile.identity_file = Some(PathBuf::from("keys/id_ed25519"));
    profile
}

fn as_agent(profile: &mut ConnectionProfile) {
    profile.transport = TransportKind::SystemOpenSsh;
    profile.authentication = AuthenticationKind::Agent;
}

fn setup() -> (
    Arc<SqliteRepository>,
    Arc<MemoryCredentialVault>,
    CredentialCoordinator,
) {
    let repository = Arc::new(SqliteRepository::open_in_memory().unwrap());
    repository.migrate().unwrap();
    let vault = Arc::new(MemoryCredentialVault::new());
    let coordinator = CredentialCoordinator::new(repository.clone(), vault.clone());
    (repository, vault, coordinator)
}

fn create_with_secret(
    coordinator: &CredentialCoordinator,
    profile: ConnectionProfile,
    value: &str,
) -> (ConnectionProfile, CredentialRef) {
    let id = profile.id;
    let catalog = coordinator
        .apply_catalog(
            CatalogMutation::Create(profile),
            SecretUpdate::Set(secret(value)),
        )
        .unwrap();
    let profile = catalog.connections[&id].clone();
    let reference = profile.credential_ref.clone().unwrap();
    (profile, reference)
}

#[test]
fn set_uses_new_ref_and_unchanged_operations_never_touch_vault() {
    let (_repository, vault, coordinator) = setup();
    let mut original = password_profile("primary");
    original.credential_ref = Some(CredentialRef::new("caller-controlled"));
    let (mut stored, reference) = create_with_secret(&coordinator, original, "alpha-secret");

    assert!(reference.0.starts_with("rshell://credential/"));
    assert_ne!(reference.0, "caller-controlled");
    assert!(vault.contains(&reference));
    let fetched = coordinator.get(&reference).unwrap().unwrap();
    assert_eq!(fetched.expose_secret(), "alpha-secret");
    let before = vault.call_counts();

    let mut shared_create = password_profile("shared-create");
    let shared_id = shared_create.id;
    shared_create.credential_ref = Some(reference.clone());
    let shared_catalog = coordinator
        .apply_catalog(
            CatalogMutation::Create(shared_create),
            SecretUpdate::Unchanged,
        )
        .unwrap();
    assert_eq!(
        shared_catalog.connections[&shared_id].credential_ref,
        Some(reference.clone())
    );

    stored.name = "updated".into();
    stored.credential_ref = Some(CredentialRef::new("must-be-ignored"));
    let catalog = coordinator
        .apply_catalog(
            CatalogMutation::Update(stored.clone()),
            SecretUpdate::Unchanged,
        )
        .unwrap();
    assert_eq!(
        catalog.connections[&stored.id].credential_ref,
        Some(reference.clone())
    );

    let duplicate = coordinator
        .apply_catalog(
            CatalogMutation::Duplicate {
                source: stored.id,
                destination: None,
            },
            SecretUpdate::Unchanged,
        )
        .unwrap()
        .connections
        .values()
        .find(|profile| profile.id != stored.id && profile.id != shared_id)
        .unwrap()
        .clone();
    assert_eq!(duplicate.credential_ref, Some(reference.clone()));
    coordinator
        .apply_catalog(
            CatalogMutation::Move {
                connection: duplicate.id,
                destination: None,
                position: 0,
            },
            SecretUpdate::Unchanged,
        )
        .unwrap();
    coordinator
        .apply_catalog(
            CatalogMutation::SetTags {
                connection: duplicate.id,
                tags: ["shared".to_owned()].into_iter().collect(),
            },
            SecretUpdate::Unchanged,
        )
        .unwrap();
    let group = ConnectionGroup::new("No vault");
    let group_id = group.id;
    coordinator
        .apply_catalog(CatalogMutation::CreateGroup(group), SecretUpdate::Unchanged)
        .unwrap();
    coordinator
        .apply_catalog(
            CatalogMutation::RenameGroup {
                group: group_id,
                name: "Still no vault".into(),
            },
            SecretUpdate::Unchanged,
        )
        .unwrap();
    coordinator
        .apply_catalog(
            CatalogMutation::MoveGroup {
                group: group_id,
                parent: None,
                position: 0,
            },
            SecretUpdate::Unchanged,
        )
        .unwrap();
    coordinator
        .apply_catalog(
            CatalogMutation::DeleteGroup(group_id),
            SecretUpdate::Unchanged,
        )
        .unwrap();

    assert_eq!(vault.call_counts(), before);
}

#[test]
fn shared_ref_is_deleted_only_after_last_profile_clears_it() {
    let (_repository, vault, coordinator) = setup();
    let (first, reference) =
        create_with_secret(&coordinator, password_profile("first"), "shared-secret");
    let catalog = coordinator
        .apply_catalog(
            CatalogMutation::Duplicate {
                source: first.id,
                destination: None,
            },
            SecretUpdate::Unchanged,
        )
        .unwrap();
    let mut second = catalog
        .connections
        .values()
        .find(|profile| profile.id != first.id)
        .unwrap()
        .clone();

    let mut third = password_profile("third");
    third.credential_ref = Some(reference.clone());
    let third_id = third.id;
    let catalog = coordinator
        .apply_catalog(CatalogMutation::Create(third), SecretUpdate::Unchanged)
        .unwrap();
    let mut third = catalog.connections[&third_id].clone();

    coordinator
        .apply_catalog(CatalogMutation::Delete(first.id), SecretUpdate::Unchanged)
        .unwrap();
    assert!(vault.contains(&reference));
    assert_eq!(vault.call_counts().delete, 0);

    as_agent(&mut second);
    coordinator
        .apply_catalog(CatalogMutation::Update(second), SecretUpdate::Clear)
        .unwrap();
    assert!(vault.contains(&reference));
    assert_eq!(vault.call_counts().delete, 0);

    as_agent(&mut third);
    coordinator
        .apply_catalog(CatalogMutation::Update(third), SecretUpdate::Clear)
        .unwrap();
    assert!(!vault.contains(&reference));
    assert_eq!(vault.call_counts().delete, 1);
}

#[test]
fn invalid_secret_update_semantics_leave_catalog_and_vault_unchanged() {
    let (repository, vault, coordinator) = setup();
    let profile = password_profile("invalid");
    let before = repository.load_catalog().unwrap();

    let dangling = CredentialRef::new("credential://not-shared");
    let mut create = profile.clone();
    create.credential_ref = Some(dangling);
    assert_eq!(
        coordinator.apply_catalog(CatalogMutation::Create(create), SecretUpdate::Unchanged),
        Err(CredentialOperationError::Validation)
    );
    assert_eq!(
        coordinator.apply_catalog(
            CatalogMutation::Move {
                connection: profile.id,
                destination: None,
                position: 0
            },
            SecretUpdate::Set(secret("must-not-write")),
        ),
        Err(CredentialOperationError::Validation)
    );
    assert_eq!(
        coordinator.apply_catalog(CatalogMutation::Create(profile), SecretUpdate::Clear),
        Err(CredentialOperationError::Validation)
    );
    assert_eq!(repository.load_catalog().unwrap(), before);
    assert!(vault.is_empty());
    assert_eq!(vault.call_counts().put, 0);
}

#[test]
fn public_key_without_passphrase_creates_and_updates_through_the_coordinator() {
    let (_repository, vault, coordinator) = setup();
    let profile = public_key_profile("no-passphrase");
    let id = profile.id;

    let catalog = coordinator
        .apply_catalog(CatalogMutation::Create(profile), SecretUpdate::Unchanged)
        .expect("public-key create without passphrase must be valid");
    let mut stored = catalog.connections[&id].clone();
    assert!(stored.credential_ref.is_none());

    stored.note = "updated without passphrase".into();
    let catalog = coordinator
        .apply_catalog(CatalogMutation::Update(stored), SecretUpdate::Unchanged)
        .expect("public-key update without passphrase must be valid");
    assert!(catalog.connections[&id].credential_ref.is_none());
    assert!(vault.is_empty());
    assert_eq!(vault.call_counts().put, 0);
    assert_eq!(vault.call_counts().delete, 0);
}

#[test]
fn password_clear_fails_core_validation_without_deleting_secret() {
    let (repository, vault, coordinator) = setup();
    let (profile, reference) =
        create_with_secret(&coordinator, password_profile("password"), "keep-me");
    let before = repository.load_catalog().unwrap();

    assert_eq!(
        coordinator.apply_catalog(CatalogMutation::Update(profile), SecretUpdate::Clear),
        Err(CredentialOperationError::Validation)
    );
    assert_eq!(repository.load_catalog().unwrap(), before);
    assert!(vault.contains(&reference));
    assert_eq!(vault.call_counts().delete, 0);
}

#[test]
fn update_set_commits_new_ref_and_reconcile_retries_old_delete() {
    let (repository, vault, coordinator) = setup();
    let (mut profile, old_ref) =
        create_with_secret(&coordinator, password_profile("rotate"), "old-secret");
    profile.name = "rotated".into();
    vault.inject_fault(MemoryVaultFault::before(
        VaultOperation::Delete,
        1,
        VaultError::Unavailable,
    ));

    let committed = coordinator
        .apply_catalog(
            CatalogMutation::Update(profile.clone()),
            SecretUpdate::Set(secret("new-secret")),
        )
        .unwrap();
    let visible = repository.load_catalog().unwrap();
    assert_eq!(committed, visible);
    let new_ref = committed.connections[&profile.id]
        .credential_ref
        .clone()
        .unwrap();
    assert_ne!(new_ref, old_ref);
    assert!(vault.contains(&old_ref));
    assert!(vault.contains(&new_ref));

    let report = coordinator.reconcile().unwrap();
    assert_eq!(report.completed, 1);
    assert!(report.is_converged());
    assert!(!vault.contains(&old_ref));
    assert!(vault.contains(&new_ref));
}

#[test]
fn put_faults_leave_catalog_old_and_reconcile_cleans_known_or_unknown_result() {
    for fault in [
        MemoryVaultFault::before(VaultOperation::Put, 1, VaultError::Denied),
        MemoryVaultFault::after_mutation_result_unknown(
            VaultMutation::Put,
            1,
            VaultError::Platform,
        ),
    ] {
        let (repository, vault, coordinator) = setup();
        vault.inject_fault(fault);
        let profile = password_profile("faulted");
        assert_eq!(
            coordinator.apply_catalog(
                CatalogMutation::Create(profile.clone()),
                SecretUpdate::Set(secret("orphan-candidate")),
            ),
            Err(CredentialOperationError::Vault)
        );
        assert!(
            !repository
                .load_catalog()
                .unwrap()
                .connections
                .contains_key(&profile.id)
        );

        let report = coordinator.reconcile().unwrap();
        assert_eq!(report.completed, 1);
        assert!(report.is_converged());
        assert!(vault.is_empty());
        assert!(coordinator.reconcile().unwrap().is_converged());
    }
}

#[test]
fn unknown_delete_result_is_idempotently_completed_by_reconcile() {
    let (_repository, vault, coordinator) = setup();
    let (profile, reference) =
        create_with_secret(&coordinator, password_profile("delete"), "delete-secret");
    vault.inject_fault(MemoryVaultFault::after_mutation_result_unknown(
        VaultMutation::Delete,
        1,
        VaultError::Platform,
    ));

    let committed = coordinator
        .apply_catalog(CatalogMutation::Delete(profile.id), SecretUpdate::Unchanged)
        .unwrap();
    assert!(!committed.connections.contains_key(&profile.id));
    assert!(!vault.contains(&reference));
    let report = coordinator.reconcile().unwrap();
    assert_eq!(report.completed, 1);
    assert!(report.is_converged());
    assert_eq!(vault.call_counts().delete, 2);
}

#[test]
fn deleting_a_missing_vault_item_is_successful() {
    let (repository, vault, coordinator) = setup();
    let mut profile = password_profile("missing");
    profile.credential_ref = Some(CredentialRef::new("credential://already-missing"));
    repository
        .apply(CatalogMutation::Create(profile.clone()))
        .unwrap();

    let catalog = coordinator
        .apply_catalog(CatalogMutation::Delete(profile.id), SecretUpdate::Unchanged)
        .unwrap();
    assert!(!catalog.connections.contains_key(&profile.id));
    assert!(vault.is_empty());
    assert_eq!(vault.call_counts().delete, 1);
}

#[test]
fn reconcile_preserves_delete_old_when_catalog_references_it_again() {
    let (repository, vault, coordinator) = setup();
    let (profile, reference) =
        create_with_secret(&coordinator, password_profile("restore"), "restore-secret");
    coordinator.inject_crash_once(CrashPoint::AfterCatalogCommitBeforeCleanup);
    assert_eq!(
        coordinator.apply_catalog(CatalogMutation::Delete(profile.id), SecretUpdate::Unchanged),
        Err(CredentialOperationError::InjectedCrash(
            CrashPoint::AfterCatalogCommitBeforeCleanup
        ))
    );
    assert!(vault.contains(&reference));

    let mut restored = password_profile("restored");
    restored.credential_ref = Some(reference.clone());
    repository.apply(CatalogMutation::Create(restored)).unwrap();
    let next = CredentialCoordinator::new(repository, vault.clone());
    let report = next.reconcile().unwrap();
    assert_eq!(report.completed, 1);
    assert!(report.is_converged());
    assert!(vault.contains(&reference));
    assert_eq!(vault.call_counts().delete, 0);
}

#[test]
fn reconcile_reports_vault_failure_and_preserves_pending_journal() {
    let (_repository, vault, coordinator) = setup();
    let profile = password_profile("pending");
    coordinator.inject_crash_once(CrashPoint::AfterPrepare);
    assert_eq!(
        coordinator.apply_catalog(
            CatalogMutation::Create(profile),
            SecretUpdate::Set(secret("never-written")),
        ),
        Err(CredentialOperationError::InjectedCrash(
            CrashPoint::AfterPrepare
        ))
    );
    vault.inject_fault(MemoryVaultFault::before(
        VaultOperation::Delete,
        1,
        VaultError::Unavailable,
    ));

    let failed = coordinator.reconcile().unwrap();
    assert_eq!(failed.failed, 1);
    assert_eq!(failed.remaining, 1);
    let recovered = coordinator.reconcile().unwrap();
    assert_eq!(recovered.completed, 1);
    assert!(recovered.is_converged());
}

#[test]
fn every_put_crash_point_reconciles_to_an_old_or_new_complete_state() {
    for point in [
        CrashPoint::AfterPrepare,
        CrashPoint::AfterVaultPutBeforeState,
        CrashPoint::AfterVaultApplied,
        CrashPoint::AfterCatalogCommitBeforeCleanup,
    ] {
        let (repository, vault, coordinator) = setup();
        let profile = password_profile("crash");
        coordinator.inject_crash_once(point);
        assert_eq!(
            coordinator.apply_catalog(
                CatalogMutation::Create(profile.clone()),
                SecretUpdate::Set(secret("crash-secret")),
            ),
            Err(CredentialOperationError::InjectedCrash(point))
        );
        drop(coordinator);
        let next = CredentialCoordinator::new(repository.clone(), vault.clone());
        let report = next.reconcile().unwrap();
        assert!(report.is_converged(), "{point:?}: {report:?}");
        let visible = repository.load_catalog().unwrap();
        if point == CrashPoint::AfterCatalogCommitBeforeCleanup {
            assert!(visible.connections.contains_key(&profile.id));
            let reference = visible.connections[&profile.id]
                .credential_ref
                .as_ref()
                .unwrap();
            assert!(vault.contains(reference));
        } else {
            assert!(!visible.connections.contains_key(&profile.id));
            assert!(vault.is_empty());
        }
        assert!(next.reconcile().unwrap().is_converged());
    }
}

#[test]
fn import_failure_keeps_catalog_atomic_and_reconcile_cleans_all_prepared_refs() {
    let (repository, vault, coordinator) = setup();
    vault.inject_fault(MemoryVaultFault::before(
        VaultOperation::Put,
        2,
        VaultError::Unavailable,
    ));
    let batch = CredentialImportBatch::new(
        vec![],
        vec![
            CredentialImportItem::new(password_profile("import-one"), Some(secret("one-secret"))),
            CredentialImportItem::new(password_profile("import-two"), Some(secret("two-secret"))),
        ],
    );
    let debug = format!("{batch:?}");
    assert!(!debug.contains("one-secret"));
    assert!(!debug.contains("two-secret"));

    assert_eq!(
        coordinator.commit_import(batch),
        Err(CredentialOperationError::Vault)
    );
    assert!(repository.load_catalog().unwrap().connections.is_empty());
    assert!(!vault.is_empty());
    let report = coordinator.reconcile().unwrap();
    assert_eq!(report.completed, 2);
    assert!(report.is_converged());
    assert!(vault.is_empty());
}

#[test]
fn import_success_commits_groups_profiles_and_secrets_together() {
    let (_repository, vault, coordinator) = setup();
    let parent = ConnectionGroup::new("parent");
    let mut child = ConnectionGroup::new("child");
    child.parent_id = Some(parent.id);
    let first = password_profile("batch-one");
    let first_id = first.id;
    let second = password_profile("batch-two");
    let second_id = second.id;
    let catalog = coordinator
        .commit_import(CredentialImportBatch::new(
            vec![child, parent],
            vec![
                CredentialImportItem::new(first, Some(secret("batch-secret-one"))),
                CredentialImportItem::new(second, Some(secret("batch-secret-two"))),
            ],
        ))
        .unwrap();

    assert_eq!(catalog.groups.len(), 2);
    assert!(
        vault.contains(
            catalog.connections[&first_id]
                .credential_ref
                .as_ref()
                .unwrap()
        )
    );
    assert!(
        vault.contains(
            catalog.connections[&second_id]
                .credential_ref
                .as_ref()
                .unwrap()
        )
    );
    assert!(coordinator.reconcile().unwrap().is_converged());
}

#[test]
fn secret_never_appears_in_database_or_public_formatting() {
    let fixture = "rshell-task6-secret-93d148b6";
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("credentials.sqlite3");
    let repository = Arc::new(SqliteRepository::open(&path).unwrap());
    repository.migrate().unwrap();
    let vault = Arc::new(MemoryCredentialVault::new());
    let coordinator = CredentialCoordinator::new(repository.clone(), vault.clone());
    let (_profile, reference) = create_with_secret(&coordinator, password_profile("scan"), fixture);
    let fetched = vault.get(&reference).unwrap().unwrap();
    assert_eq!(fetched.expose_secret(), fixture);
    let imported = password_profile("scan-import");
    let imported_id = imported.id;
    let imported_catalog = coordinator
        .commit_import(CredentialImportBatch::new(
            vec![],
            vec![CredentialImportItem::new(imported, Some(secret(fixture)))],
        ))
        .unwrap();
    assert!(
        vault.contains(
            imported_catalog.connections[&imported_id]
                .credential_ref
                .as_ref()
                .unwrap()
        )
    );

    let formatted = format!(
        "{coordinator:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
        CredentialOperationError::Validation,
        CredentialOperationError::Vault,
        CredentialOperationError::Storage,
        CredentialOperationError::ReconciliationRequired,
        CredentialOperationError::InjectedCrash(CrashPoint::AfterPrepare),
        rshell_storage::ReconcileReport::default(),
        VaultError::Unavailable,
        VaultError::NoEntry,
        VaultError::Denied,
        VaultError::Platform,
    );
    assert!(!formatted.contains(fixture));
    assert_files_exclude(&path, fixture);
    repository.shutdown().unwrap();
    assert_files_exclude(&path, fixture);
}

fn assert_files_exclude(path: &std::path::Path, fixture: &str) {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        if candidate.exists() {
            let bytes = fs::read(&candidate).unwrap();
            assert!(
                !bytes
                    .windows(fixture.len())
                    .any(|window| window == fixture.as_bytes()),
                "secret found in {}",
                candidate.display()
            );
        }
    }
}

#[test]
fn memory_vault_get_fault_is_counted_and_categorized() {
    let reference = CredentialRef::new("memory://get-fault");
    let vault = MemoryCredentialVault::new();
    vault.put(&reference, &secret("memory-secret")).unwrap();
    vault.inject_fault(MemoryVaultFault::before(
        VaultOperation::Get,
        1,
        VaultError::Denied,
    ));
    assert_eq!(vault.get(&reference).unwrap_err(), VaultError::Denied);
    assert_eq!(vault.call_counts().get, 1);
    assert!(vault.contains(&reference));
}

#[cfg(feature = "test-support")]
#[test]
fn journal_states_match_each_crash_boundary() {
    for (point, expected_state) in [
        (CrashPoint::AfterPrepare, "'put_new'|'prepared'"),
        (CrashPoint::AfterVaultPutBeforeState, "'put_new'|'prepared'"),
        (CrashPoint::AfterVaultApplied, "'put_new'|'vault_applied'"),
    ] {
        let (repository, _vault, coordinator) = setup();
        coordinator.inject_crash_once(point);
        let _ = coordinator.apply_catalog(
            CatalogMutation::Create(password_profile("journal")),
            SecretUpdate::Set(secret("journal-secret")),
        );
        let visible = String::from_utf8(repository.test_visible_tables().unwrap()).unwrap();
        assert!(visible.contains(expected_state), "{point:?}: {visible}");
    }

    let (repository, _vault, coordinator) = setup();
    let (profile, _) = create_with_secret(
        &coordinator,
        password_profile("delete-journal"),
        "delete-journal-secret",
    );
    coordinator.inject_crash_once(CrashPoint::AfterCatalogCommitBeforeCleanup);
    let _ = coordinator.apply_catalog(CatalogMutation::Delete(profile.id), SecretUpdate::Unchanged);
    let visible = String::from_utf8(repository.test_visible_tables().unwrap()).unwrap();
    assert!(visible.contains("'delete_old'|'prepared'"));
    assert!(!visible.contains("'put_new'"));
}

#[cfg(feature = "test-support")]
#[test]
fn sqlite_failures_roll_back_prepare_or_finalize_without_half_catalog_state() {
    let (repository, vault, coordinator) = setup();
    let profile = password_profile("prepare-storage-failure");
    repository.inject_statement_failure_once(1).unwrap();
    assert_eq!(
        coordinator.apply_catalog(
            CatalogMutation::Create(profile.clone()),
            SecretUpdate::Set(secret("prepare-failure-secret")),
        ),
        Err(CredentialOperationError::Storage)
    );
    assert!(
        !repository
            .load_catalog()
            .unwrap()
            .connections
            .contains_key(&profile.id)
    );
    assert!(vault.is_empty());
    assert_eq!(coordinator.reconcile().unwrap().completed, 0);

    let profile = password_profile("finalize-storage-failure");
    repository.inject_statement_failure_once(3).unwrap();
    assert_eq!(
        coordinator.apply_catalog(
            CatalogMutation::Create(profile.clone()),
            SecretUpdate::Set(secret("finalize-failure-secret")),
        ),
        Err(CredentialOperationError::ReconciliationRequired)
    );
    assert!(
        !repository
            .load_catalog()
            .unwrap()
            .connections
            .contains_key(&profile.id)
    );
    assert!(!vault.is_empty());
    let report = coordinator.reconcile().unwrap();
    assert_eq!(report.completed, 1);
    assert!(report.is_converged());
    assert!(vault.is_empty());
}

#[cfg(feature = "test-support")]
#[test]
fn reconcile_returns_storage_error_when_journal_completion_fails() {
    let (repository, vault, coordinator) = setup();
    let profile = password_profile("reconcile-storage-failure");
    coordinator.inject_crash_once(CrashPoint::AfterPrepare);
    assert_eq!(
        coordinator.apply_catalog(
            CatalogMutation::Create(profile),
            SecretUpdate::Set(secret("reconcile-storage-secret")),
        ),
        Err(CredentialOperationError::InjectedCrash(
            CrashPoint::AfterPrepare
        ))
    );
    repository.inject_statement_failure_once(1).unwrap();

    assert_eq!(
        coordinator.reconcile(),
        Err(CredentialOperationError::Storage)
    );
    let pending = String::from_utf8(repository.test_visible_tables().unwrap()).unwrap();
    assert!(pending.contains("'put_new'|'prepared'"));
    assert!(vault.is_empty());

    let recovered = coordinator.reconcile().unwrap();
    assert_eq!(recovered.completed, 1);
    assert!(recovered.is_converged());
}

#[cfg(feature = "test-support")]
#[test]
fn post_commit_journal_cleanup_failure_still_returns_committed_catalog() {
    let (repository, vault, coordinator) = setup();
    let (mut profile, old_ref) = create_with_secret(
        &coordinator,
        password_profile("cleanup-storage-failure"),
        "cleanup-old-secret",
    );
    profile.name = "cleanup committed".into();
    repository.inject_statement_failure_once(8).unwrap();

    let committed = coordinator
        .apply_catalog(
            CatalogMutation::Update(profile.clone()),
            SecretUpdate::Set(secret("cleanup-new-secret")),
        )
        .unwrap();
    assert_eq!(committed, repository.load_catalog().unwrap());
    let new_ref = committed.connections[&profile.id]
        .credential_ref
        .as_ref()
        .unwrap();
    assert!(!vault.contains(&old_ref));
    assert!(vault.contains(new_ref));
    let pending = String::from_utf8(repository.test_visible_tables().unwrap()).unwrap();
    assert!(pending.contains("'delete_old'|'prepared'"));

    let report = coordinator.reconcile().unwrap();
    assert_eq!(report.completed, 1);
    assert!(report.is_converged());
}
