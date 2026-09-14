use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionProfile, CredentialRef, SecretUpdate,
    TransportKind,
};
use rshell_storage::{
    CredentialCoordinator, CredentialOperationError, CredentialVault, MemoryCredentialVault,
    MemoryVaultFault, SYSTEM_CREDENTIAL_SERVICE, SqliteRepository, SystemCredentialVault,
    VaultError, VaultMutation,
};
use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

const QA_VAULT_OBSERVATION_PATH_ENV: &str = "RSHELL_P0_QA_VAULT_OBSERVATION_PATH";
const QA_VAULT_REFERENCE_ENV: &str = "RSHELL_P0_QA_VAULT_REFERENCE";
const QA_VAULT_FAILURE_SECRET_ENV: &str = "RSHELL_P0_QA_VAULT_FAILURE_SECRET";

fn parent_vault_reference() -> CredentialRef {
    let value = std::env::var(QA_VAULT_REFERENCE_ENV)
        .expect("RSHELL_P0_QA_VAULT_REFERENCE must be parent-owned");
    assert!(
        value.starts_with("rshell://credential/") && value.len() <= 128,
        "parent-owned credential reference has an invalid shape"
    );
    CredentialRef::new(value)
}

fn assert_send_sync<T: Send + Sync>() {}

fn assert_vault_api<V: CredentialVault>() {
    let _: fn(&V, &CredentialRef) -> Result<Option<SecretString>, VaultError> =
        <V as CredentialVault>::get;
    let _: fn(&V, &CredentialRef, &SecretString) -> Result<(), VaultError> =
        <V as CredentialVault>::put;
    let _: fn(&V, &CredentialRef) -> Result<(), VaultError> = <V as CredentialVault>::delete;
}

fn random_secret() -> SecretString {
    SecretString::from(format!("system-vault-smoke-{}", Uuid::new_v4()))
}

fn password_profile(name: &str) -> ConnectionProfile {
    let mut profile = ConnectionProfile::new(name, format!("{name}.example.test"));
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;
    profile.username = "operator".into();
    profile
}

fn memory_setup() -> (
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

fn assert_database_files_exclude(path: &Path, secret: &[u8]) {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        if candidate.exists() {
            let bytes = fs::read(&candidate).unwrap();
            assert!(
                !bytes.windows(secret.len()).any(|window| window == secret),
                "secret found in {}",
                candidate.display()
            );
        }
    }
}

fn qa_vault_observation_path() -> PathBuf {
    let path = std::env::var_os(QA_VAULT_OBSERVATION_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .expect("RSHELL_P0_QA_VAULT_OBSERVATION_PATH must name a new observation file");
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        assert!(
            parent.is_dir(),
            "RSHELL_P0_QA_VAULT_OBSERVATION_PATH parent directory is unavailable"
        );
    }
    assert!(
        !path.exists(),
        "RSHELL_P0_QA_VAULT_OBSERVATION_PATH must not overwrite an existing file"
    );
    path
}

fn write_vault_qa_observation(path: &Path) {
    let document = serde_json::json!({
        "version": 1,
        "generated_by": "p0_qa",
        "surface": "vault",
        "observations": [
            "vault_credential_reference",
            "vault_database_secret_scan",
            "vault_temporary_reference_zero",
            "journal_count_zero",
        ],
    });
    let bytes = serde_json::to_vec(&document).expect("vault QA observation must serialize");
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .expect("vault QA observation temp file must open");
    temporary
        .write_all(&bytes)
        .expect("vault QA observation temp file must write");
    temporary
        .flush()
        .expect("vault QA observation temp file must flush");
    temporary
        .persist_noclobber(path)
        .expect("vault QA observation must not overwrite an existing file");
}

#[test]
fn system_vault_constructor_is_thread_safe_and_performs_no_operations() {
    assert_eq!(SYSTEM_CREDENTIAL_SERVICE, "io.github.hugefiver.rshell");
    assert_send_sync::<SystemCredentialVault>();
    assert_vault_api::<SystemCredentialVault>();

    let vault = SystemCredentialVault::new();
    let _: &dyn CredentialVault = &vault;
}

#[test]
fn coordinator_keeps_secret_out_of_sqlite_when_using_memory_vault() {
    let temp = tempfile::tempdir().unwrap();
    let database_path = temp.path().join("memory-vault.sqlite3");
    let repository = Arc::new(SqliteRepository::open(&database_path).unwrap());
    repository.migrate().unwrap();
    let vault = Arc::new(MemoryCredentialVault::new());
    let coordinator = CredentialCoordinator::new(repository.clone(), vault);
    let secret = random_secret();
    let profile = password_profile("sqlite-secret-scan");
    let profile_id = profile.id;

    let catalog = coordinator
        .apply_catalog(
            CatalogMutation::Create(profile),
            SecretUpdate::Set(secret.clone()),
        )
        .unwrap();
    let credential_ref = catalog.connections[&profile_id]
        .credential_ref
        .as_ref()
        .unwrap();
    let fetched = coordinator.get(credential_ref).unwrap().unwrap();
    assert!(fetched.expose_secret() == secret.expose_secret());
    assert_database_files_exclude(&database_path, secret.expose_secret().as_bytes());

    drop(coordinator);
    repository.shutdown().unwrap();
    assert_database_files_exclude(&database_path, secret.expose_secret().as_bytes());
}

#[test]
fn memory_vault_fail_after_mutation_is_recovered_without_touching_system_vault() {
    let (repository, vault, coordinator) = memory_setup();
    let profile = password_profile("fail-after-mutation");
    vault.inject_fault(MemoryVaultFault::after_mutation_result_unknown(
        VaultMutation::Put,
        1,
        VaultError::Platform,
    ));

    assert_eq!(
        coordinator.apply_catalog(
            CatalogMutation::Create(profile.clone()),
            SecretUpdate::Set(random_secret()),
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
    assert!(!vault.is_empty());

    let report = coordinator.reconcile().unwrap();
    assert_eq!(report.completed, 1);
    assert!(report.is_converged());
    assert!(vault.is_empty());
}

#[cfg(feature = "test-support")]
#[test]
fn sqlite_finalize_failure_is_recovered_without_touching_system_vault() {
    let (repository, vault, coordinator) = memory_setup();
    let profile = password_profile("finalize-failure");
    repository.inject_statement_failure_once(3).unwrap();

    assert_eq!(
        coordinator.apply_catalog(
            CatalogMutation::Create(profile.clone()),
            SecretUpdate::Set(random_secret()),
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

struct CleanupSystemVault {
    inner: SystemCredentialVault,
    entries: Mutex<Vec<CredentialRef>>,
}

impl CleanupSystemVault {
    fn new() -> Self {
        Self {
            inner: SystemCredentialVault::new(),
            entries: Mutex::new(Vec::new()),
        }
    }

    fn remember(&self, credential_ref: &CredentialRef) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !entries.contains(credential_ref) {
            entries.push(credential_ref.clone());
        }
    }

    fn entries(&self) -> Vec<CredentialRef> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn cleanup(&self) -> Result<(), VaultError> {
        for credential_ref in self.entries() {
            self.inner.delete(&credential_ref)?;
            if self.inner.get(&credential_ref)?.is_some() {
                return Err(VaultError::Platform);
            }
        }
        Ok(())
    }
}

impl Drop for CleanupSystemVault {
    fn drop(&mut self) {
        for credential_ref in self
            .entries
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            let _ = self.inner.delete(credential_ref);
        }
    }
}

impl CredentialVault for CleanupSystemVault {
    fn get(&self, credential_ref: &CredentialRef) -> Result<Option<SecretString>, VaultError> {
        self.inner.get(credential_ref)
    }

    fn put(&self, credential_ref: &CredentialRef, value: &SecretString) -> Result<(), VaultError> {
        self.remember(credential_ref);
        self.inner.put(credential_ref, value)
    }

    fn delete(&self, credential_ref: &CredentialRef) -> Result<(), VaultError> {
        self.inner.delete(credential_ref)
    }
}

#[test]
#[ignore = "requires explicit Vault/All QA mode; touches the real system credential store"]
fn system_vault_real_os_probe_uses_coordinator_and_cleans_random_entry() {
    let observation_path = qa_vault_observation_path();
    let temp = tempfile::tempdir().unwrap();
    let database_path = temp.path().join("system-vault.sqlite3");
    let repository = Arc::new(SqliteRepository::open(&database_path).unwrap());
    repository.migrate().unwrap();
    let vault = Arc::new(CleanupSystemVault::new());
    let coordinator = CredentialCoordinator::new(repository.clone(), vault.clone());
    let secret = random_secret();
    let profile = password_profile("real-system-vault");
    let profile_id = profile.id;
    let expected_reference = parent_vault_reference();
    rshell_storage::inject_next_credential_reference(expected_reference.clone());

    let catalog = coordinator
        .apply_catalog(
            CatalogMutation::Create(profile),
            SecretUpdate::Set(secret.clone()),
        )
        .unwrap();
    let credential_ref = catalog.connections[&profile_id]
        .credential_ref
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(credential_ref, expected_reference);
    let fetched = coordinator.get(&credential_ref).unwrap().unwrap();
    assert!(fetched.expose_secret() == secret.expose_secret());
    assert_database_files_exclude(&database_path, secret.expose_secret().as_bytes());

    let catalog = coordinator
        .apply_catalog(CatalogMutation::Delete(profile_id), SecretUpdate::Unchanged)
        .unwrap();
    assert!(!catalog.connections.contains_key(&profile_id));
    assert!(coordinator.get(&credential_ref).unwrap().is_none());
    assert_database_files_exclude(&database_path, secret.expose_secret().as_bytes());

    let report = coordinator.reconcile().unwrap();
    assert!(report.is_converged());
    assert!(coordinator.get(&credential_ref).unwrap().is_none());

    drop(coordinator);
    repository.shutdown().unwrap();
    assert_database_files_exclude(&database_path, secret.expose_secret().as_bytes());
    vault.cleanup().unwrap();
    write_vault_qa_observation(&observation_path);
}

#[test]
#[ignore = "explicit QA failure-path probe; intentionally leaves the parent-ledger entry"]
fn system_vault_failure_probe_leaves_exact_parent_entry_for_harness_cleanup() {
    let credential_ref = parent_vault_reference();
    let secret = SecretString::from(
        std::env::var(QA_VAULT_FAILURE_SECRET_ENV)
            .expect("RSHELL_P0_QA_VAULT_FAILURE_SECRET must be child-only"),
    );
    let vault = SystemCredentialVault::new();
    vault.put(&credential_ref, &secret).unwrap();
    assert!(vault.get(&credential_ref).unwrap().is_some());
    panic!("intentional fail_during_vault_probe after exact parent-ledger mutation");
}

#[test]
#[ignore = "explicit parent-ledger cleanup for a real system credential entry"]
fn system_vault_cleanup_exact_parent_reference() {
    let credential_ref = parent_vault_reference();
    let vault = SystemCredentialVault::new();
    vault.delete(&credential_ref).unwrap();
    assert!(vault.get(&credential_ref).unwrap().is_none());
}
