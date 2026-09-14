use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionId, ConnectionProfile, HostKeyPolicy, KeyCode,
    TransportKind,
};
use rshell_storage::{
    CredentialCoordinator, ImportError, ImportWarning, LegacyJsonImporter, MemoryCredentialVault,
    MemoryVaultFault, SqliteRepository, VaultError, VaultOperation,
};
#[cfg(feature = "test-support")]
use sha2::{Digest, Sha256};
use uuid::Uuid;

const PROD_ID: &str = "11111111-1111-1111-1111-111111111111";
const PROD_SECRET: &str = "rshell-task7-prod-secret-a4e68b";
const FIRST_SECRET: &str = "rshell-task7-first-secret-c6f3de";
const SECOND_SECRET: &str = "rshell-task7-second-secret-f27a91";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("legacy")
        .join(name)
}

fn selected(preview: &rshell_storage::ImportPreview) -> BTreeSet<ConnectionId> {
    preview
        .connections
        .iter()
        .map(|candidate| candidate.id)
        .collect()
}

fn setup_memory() -> (
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

fn preview(importer: &LegacyJsonImporter, name: &str) -> rshell_storage::ImportPreview {
    importer.preview(fixture(name)).unwrap()
}

#[test]
fn preview_is_pure_and_commit_preserves_ids_groups_auth_and_terminal_overrides() {
    let (repository, vault, coordinator) = setup_memory();
    let importer = LegacyJsonImporter::new();
    let before = repository.load_catalog().unwrap();

    let preview = preview(&importer, "valid.json");

    assert_eq!(repository.load_catalog().unwrap(), before);
    assert_eq!(preview.connections.len(), 3);
    assert!(
        preview
            .connections
            .iter()
            .find(|candidate| candidate.id.0 == Uuid::parse_str(PROD_ID).unwrap())
            .unwrap()
            .has_secret
    );
    assert!(!format!("{preview:?}").contains(PROD_SECRET));
    assert!(
        preview
            .warnings
            .contains(&ImportWarning::KittyGraphicsDisabled)
    );
    assert!(
        preview
            .warnings
            .contains(&ImportWarning::HostKeyPolicyUpgraded)
    );

    let selected = selected(&preview);
    let report = importer.commit(&coordinator, preview, &selected).unwrap();
    assert_eq!(report.imported_connections, 3);
    assert_eq!(report.imported_groups, 1);
    assert_eq!(report.skipped_connections, 0);

    let prod_id = ConnectionId(Uuid::parse_str(PROD_ID).unwrap());
    let stored = &repository.load_catalog().unwrap().connections[&prod_id];
    assert_eq!(stored.name, "prod");
    assert_eq!(stored.host, "prod.example.test");
    assert_eq!(stored.username, "deploy");
    assert_eq!(stored.port, 2222);
    assert_eq!(stored.transport, TransportKind::NativeSsh);
    assert_eq!(stored.authentication, AuthenticationKind::Password);
    assert_eq!(stored.host_key_policy, HostKeyPolicy::Strict);
    assert_eq!(
        stored.identity_file.as_deref(),
        Some(Path::new("C:/keys/prod_ed25519"))
    );
    assert_eq!(
        stored.remote_command.as_deref(),
        Some("tmux new-session -A -s prod")
    );
    assert_eq!(stored.note, "production endpoint");
    assert_eq!(
        stored.terminal_overrides.terminal_type.as_deref(),
        Some("xterm")
    );
    assert_eq!(stored.terminal_overrides.initial_cols, Some(132));
    assert_eq!(stored.terminal_overrides.initial_rows, Some(44));
    assert_eq!(stored.terminal_overrides.scrollback_lines, Some(12_000));
    assert_eq!(stored.terminal_overrides.font_size, Some(16.0));
    assert_eq!(
        stored.terminal_overrides.color_scheme,
        Some(rshell_core::ColorScheme::Nord)
    );
    assert_eq!(stored.terminal_overrides.left_alt_as_meta, Some(false));
    assert_eq!(stored.terminal_overrides.right_alt_as_meta, Some(true));
    assert_eq!(stored.terminal_overrides.enable_csi_u, Some(true));
    assert_eq!(stored.terminal_overrides.enable_kitty_keyboard, Some(true));
    assert_eq!(stored.terminal_overrides.mouse_reporting, Some(false));
    assert_eq!(stored.terminal_overrides.scroll_on_output, Some(false));
    assert_eq!(stored.terminal_overrides.scroll_on_keypress, Some(true));
    assert_eq!(
        stored.terminal_overrides.answerback.as_deref(),
        Some("legacy-answerback")
    );
    assert!(
        stored
            .terminal_overrides
            .key_bindings
            .as_ref()
            .unwrap()
            .iter()
            .any(|binding| binding.code == KeyCode::Delete)
    );
    assert!(
        stored
            .terminal_overrides
            .key_bindings
            .as_ref()
            .unwrap()
            .iter()
            .any(|binding| binding.code == KeyCode::Backspace)
    );
    assert!(vault.contains(stored.credential_ref.as_ref().unwrap()));

    let catalog = repository.load_catalog().unwrap();
    let key = &catalog.connections
        [&ConnectionId(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())];
    assert_eq!(key.transport, TransportKind::NativeSsh);
    assert_eq!(key.authentication, AuthenticationKind::PublicKey);
    assert_eq!(key.port, 22);
    let agent = &catalog.connections
        [&ConnectionId(Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap())];
    assert_eq!(agent.transport, TransportKind::SystemOpenSsh);
    assert_eq!(agent.authentication, AuthenticationKind::Agent);
    assert_eq!(agent.port, 22);
    assert_eq!(
        catalog.groups[&stored.group_id.unwrap()].id.0,
        Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap()
    );
}

#[test]
fn selected_subset_imports_only_selected_connections_and_required_groups() {
    let (repository, _vault, coordinator) = setup_memory();
    let importer = LegacyJsonImporter::new();
    let preview = preview(&importer, "valid.json");
    let selected = [ConnectionId(Uuid::parse_str(PROD_ID).unwrap())]
        .into_iter()
        .collect();

    let report = importer.commit(&coordinator, preview, &selected).unwrap();

    assert_eq!(report.imported_connections, 1);
    assert_eq!(report.imported_groups, 1);
    assert_eq!(repository.load_catalog().unwrap().connections.len(), 1);
}

#[test]
fn corrupt_or_missing_primary_recovers_from_sibling_backup_without_mutating_files() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("connections.json");
    let backup = temp.path().join("connections.json.bak");
    fs::copy(fixture("corrupt.json"), &primary).unwrap();
    fs::copy(fixture("connections.json.bak"), &backup).unwrap();
    let before_primary = fs::read(&primary).unwrap();
    let before_backup = fs::read(&backup).unwrap();

    let preview = LegacyJsonImporter::new().preview(&primary).unwrap();

    assert_eq!(preview.connections.len(), 1);
    assert!(
        preview
            .warnings
            .contains(&ImportWarning::RecoveredFromBackup)
    );
    assert_eq!(fs::read(&primary).unwrap(), before_primary);
    assert_eq!(fs::read(&backup).unwrap(), before_backup);

    fs::remove_file(&primary).unwrap();
    let missing_primary = LegacyJsonImporter::new().preview(&primary).unwrap();
    assert!(
        missing_primary
            .warnings
            .contains(&ImportWarning::RecoveredFromBackup)
    );
    let recovered = &missing_primary.connections[0].profile;
    assert_eq!(recovered.transport, TransportKind::NativeSsh);
    assert_eq!(
        recovered.authentication,
        AuthenticationKind::KeyboardInteractive
    );
}

#[test]
fn authentication_mapping_covers_every_legacy_backend_case() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("connections.json");
    fs::write(
        &source,
        r#"{
          "folders": [],
          "connections": [
            {"id":"88888888-8888-8888-8888-888888888881","name":"password","host":"password.example","backend":"system_open_ssh","password":"test-only-password"},
            {"id":"88888888-8888-8888-8888-888888888882","name":"system-key","host":"system-key.example","backend":"system_open_ssh","identity_file":"id_system"},
            {"id":"88888888-8888-8888-8888-888888888883","name":"native-key","host":"native-key.example","backend":"wez_term_ssh","identity_file":"id_native"},
            {"id":"88888888-8888-8888-8888-888888888884","name":"system-agent","host":"system-agent.example","backend":"system_open_ssh"},
            {"id":"88888888-8888-8888-8888-888888888885","name":"native-keyboard","host":"native-keyboard.example","backend":"wez_term_ssh"}
          ]
        }"#,
    )
    .unwrap();

    let preview = LegacyJsonImporter::new().preview(source).unwrap();
    let authentication = preview
        .connections
        .iter()
        .map(|candidate| {
            (
                candidate.profile.transport,
                candidate.profile.authentication,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        authentication,
        vec![
            (TransportKind::NativeSsh, AuthenticationKind::Password),
            (TransportKind::SystemOpenSsh, AuthenticationKind::PublicKey),
            (TransportKind::NativeSsh, AuthenticationKind::PublicKey),
            (TransportKind::SystemOpenSsh, AuthenticationKind::Agent),
            (
                TransportKind::NativeSsh,
                AuthenticationKind::KeyboardInteractive
            ),
        ]
    );
}

#[test]
fn invalid_primary_and_backup_return_a_pure_error() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("connections.json");
    fs::copy(fixture("corrupt.json"), &primary).unwrap();
    fs::copy(
        fixture("corrupt.json"),
        temp.path().join("connections.json.bak"),
    )
    .unwrap();

    assert!(matches!(
        LegacyJsonImporter::new().preview(&primary),
        Err(ImportError::NoUsableSource)
    ));
}

#[test]
fn same_source_fingerprint_cannot_be_imported_twice() {
    let (repository, _vault, coordinator) = setup_memory();
    let importer = LegacyJsonImporter::new();
    let first = preview(&importer, "valid.json");
    let first_selected = selected(&first);
    importer
        .commit(&coordinator, first, &first_selected)
        .unwrap();
    let before = repository.load_catalog().unwrap();
    let second = preview(&importer, "valid.json");
    let second_selected = selected(&second);

    assert!(matches!(
        importer.commit(&coordinator, second, &second_selected),
        Err(ImportError::AlreadyImported)
    ));
    assert_eq!(repository.load_catalog().unwrap(), before);
}

#[test]
fn id_conflicts_are_rejected_before_any_vault_write() {
    let (repository, vault, coordinator) = setup_memory();
    let importer = LegacyJsonImporter::new();
    let mut existing = ConnectionProfile::new("existing", "existing.example.test");
    existing.id = ConnectionId(Uuid::parse_str(PROD_ID).unwrap());
    repository.apply(CatalogMutation::Create(existing)).unwrap();
    let import = preview(&importer, "valid.json");
    let selected = selected(&import);

    assert!(matches!(
        importer.commit(&coordinator, import, &selected),
        Err(ImportError::IdConflict)
    ));
    assert_eq!(vault.call_counts().put, 0);
}

#[test]
fn bad_uuid_and_port_are_rejected_during_pure_preview() {
    let temp = tempfile::tempdir().unwrap();
    let invalid_uuid = temp.path().join("invalid-uuid.json");
    fs::write(
        &invalid_uuid,
        r#"{"folders":[],"connections":[{"id":"not-a-uuid","name":"bad","host":"bad.example"}]}"#,
    )
    .unwrap();
    let invalid_port = temp.path().join("invalid-port.json");
    fs::write(
        &invalid_port,
        r#"{"folders":[],"connections":[{"id":"77777777-7777-7777-7777-777777777777","name":"bad","host":"bad.example","port":70000}]}"#,
    )
    .unwrap();
    let importer = LegacyJsonImporter::new();

    assert!(matches!(
        importer.preview(&invalid_uuid),
        Err(ImportError::InvalidUuid)
    ));
    assert!(matches!(
        importer.preview(&invalid_port),
        Err(ImportError::InvalidPort)
    ));
}

#[test]
fn second_secret_vault_failure_rolls_back_entire_import_after_reconcile() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("import-rollback.sqlite3");
    let repository = Arc::new(SqliteRepository::open(&database).unwrap());
    repository.migrate().unwrap();
    let vault = Arc::new(MemoryCredentialVault::new());
    let coordinator = CredentialCoordinator::new(repository.clone(), vault.clone());
    vault.inject_fault(MemoryVaultFault::before(
        VaultOperation::Put,
        2,
        VaultError::Unavailable,
    ));
    let importer = LegacyJsonImporter::new();
    let imported = preview(&importer, "plaintext-password.json");
    let selected = selected(&imported);
    let before = repository.load_catalog().unwrap();

    assert!(matches!(
        importer.commit(&coordinator, imported, &selected),
        Err(ImportError::Credential(
            rshell_storage::CredentialOperationError::Vault
        ))
    ));
    assert_eq!(repository.load_catalog().unwrap(), before);
    drop(coordinator);
    repository.shutdown().unwrap();
    drop(repository);

    let reopened = Arc::new(SqliteRepository::open(&database).unwrap());
    reopened.migrate().unwrap();
    let restarted = CredentialCoordinator::new(reopened.clone(), vault.clone());
    assert!(restarted.reconcile().unwrap().is_converged());
    assert_eq!(reopened.load_catalog().unwrap(), before);
    assert!(vault.is_empty());
    reopened.shutdown().unwrap();
}

#[cfg(feature = "test-support")]
#[test]
fn injected_database_failure_leaves_visible_catalog_unchanged_and_reconciles_orphans() {
    let (repository, vault, coordinator) = setup_memory();
    let importer = LegacyJsonImporter::new();
    let imported = preview(&importer, "plaintext-password.json");
    let selected = selected(&imported);
    let before = repository.load_catalog().unwrap();
    repository.inject_statement_failure_once(5).unwrap();

    assert!(matches!(
        importer.commit(&coordinator, imported, &selected),
        Err(ImportError::Credential(
            rshell_storage::CredentialOperationError::ReconciliationRequired
        ))
    ));
    assert_eq!(repository.load_catalog().unwrap(), before);
    assert!(
        CredentialCoordinator::new(repository.clone(), vault.clone())
            .reconcile()
            .unwrap()
            .is_converged()
    );
    assert_eq!(repository.load_catalog().unwrap(), before);
    assert!(vault.is_empty());
}

#[cfg(feature = "test-support")]
#[test]
fn legacy_import_persists_the_exact_source_sha256_as_an_app_setting_key() {
    let (repository, _vault, coordinator) = setup_memory();
    let importer = LegacyJsonImporter::new();
    let imported = preview(&importer, "valid.json");
    let selected = selected(&imported);
    let digest = Sha256::digest(fs::read(fixture("valid.json")).unwrap());
    let key = format!(
        "import.legacy.sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );

    importer.commit(&coordinator, imported, &selected).unwrap();

    let visible = String::from_utf8(repository.test_visible_tables().unwrap()).unwrap();
    assert!(visible.contains(&key));
    assert!(visible.contains("'legacy-json'"));
}

#[test]
fn plaintext_secrets_are_absent_from_database_wal_shm_and_public_output() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("storage.sqlite3");
    let repository = Arc::new(SqliteRepository::open(&database).unwrap());
    repository.migrate().unwrap();
    let vault = Arc::new(MemoryCredentialVault::new());
    let coordinator = CredentialCoordinator::new(repository.clone(), vault);
    let importer = LegacyJsonImporter::new();
    let imported = preview(&importer, "plaintext-password.json");
    let selected = selected(&imported);
    let preview_output = format!("{imported:?}");
    assert!(!preview_output.contains(FIRST_SECRET));
    assert!(!preview_output.contains(SECOND_SECRET));

    importer.commit(&coordinator, imported, &selected).unwrap();
    repository.shutdown().unwrap();
    for candidate in [
        database.clone(),
        PathBuf::from(format!("{}-wal", database.display())),
        PathBuf::from(format!("{}-shm", database.display())),
    ] {
        if candidate.exists() {
            let bytes = fs::read(&candidate).unwrap();
            for secret in [FIRST_SECRET, SECOND_SECRET] {
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|window| window == secret.as_bytes())
                );
            }
        }
    }
}
