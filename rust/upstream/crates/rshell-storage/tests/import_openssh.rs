use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use rshell_core::{AuthenticationKind, ConnectionId, TransportKind};
use rshell_storage::{
    CredentialCoordinator, CredentialOperationError, ImportError, ImportWarning,
    MemoryCredentialVault, OpenSshConfigImporter, SqliteRepository,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("openssh")
        .join(name)
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

fn selected(preview: &rshell_storage::OpenSshPreview) -> BTreeSet<ConnectionId> {
    preview
        .candidates
        .iter()
        .filter(|candidate| candidate.importable)
        .map(|candidate| candidate.id)
        .collect()
}

fn candidate<'a>(
    preview: &'a rshell_storage::OpenSshPreview,
    alias: &str,
) -> &'a rshell_storage::OpenSshCandidate {
    preview
        .candidates
        .iter()
        .find(|candidate| candidate.host_pattern == alias)
        .unwrap_or_else(|| panic!("missing {alias} candidate"))
}

#[test]
fn literal_aliases_are_importable_and_templates_are_preview_only() {
    let preview = OpenSshConfigImporter::new()
        .preview(fixture("config"))
        .unwrap();

    let production = candidate(&preview, "production");
    assert!(production.importable);
    assert_eq!(production.profile.name, "production");
    assert_eq!(production.host_name, "10.0.0.8");
    assert_eq!(production.profile.host, "10.0.0.8");
    assert_eq!(production.profile.username, "global-user");
    assert_eq!(production.profile.port, 2222);
    assert_eq!(
        production.identity_file.as_deref(),
        Some(Path::new("C:/keys/production key"))
    );
    assert_eq!(
        production.profile.authentication,
        AuthenticationKind::PublicKey
    );

    let template = candidate(&preview, "*.corp");
    assert!(!template.importable);
    assert_eq!(
        template.profile.host_key_policy,
        rshell_core::HostKeyPolicy::Strict
    );
}

#[test]
fn globals_and_matching_blocks_obey_first_obtained_value_wins() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    fs::write(
        &config,
        "User global\nPort 2200\nHost app\n User first\n Port 2222\n",
    )
    .unwrap();

    let preview = OpenSshConfigImporter::new().preview(config).unwrap();
    let app = candidate(&preview, "app");
    assert_eq!(app.profile.username, "global");
    assert_eq!(app.profile.port, 2200);
    fs::write(
        temp.path().join("matching"),
        "Host service\n User first-match\n Port 2222\nHost s*\n User later-match\n Port 2022\n",
    )
    .unwrap();
    let matching = OpenSshConfigImporter::new()
        .preview(temp.path().join("matching"))
        .unwrap();
    let service = candidate(&matching, "service");
    assert_eq!(service.profile.username, "first-match");
    assert_eq!(service.profile.port, 2222);
}

#[test]
fn relative_includes_quoted_values_and_globs_are_static_and_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    let fragments = temp.path().join("fragments");
    let nested = temp.path().join("nested");
    fs::create_dir(&fragments).unwrap();
    fs::create_dir(&nested).unwrap();
    fs::write(
        temp.path().join("config"),
        "Include included.conf included.conf nested/outer.conf fragments/*.conf\n",
    )
    .unwrap();
    fs::write(nested.join("outer.conf"), "Include inner.conf\n").unwrap();
    fs::write(
        nested.join("inner.conf"),
        "Host nested\n HostName nested.example\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("included.conf"),
        "Host quoted\n IdentityFile \"relative key\"\n",
    )
    .unwrap();
    fs::write(
        fragments.join("b.conf"),
        "Host second\n HostName second.example\n",
    )
    .unwrap();
    fs::write(
        fragments.join("a.conf"),
        "Host first\n HostName first.example\n",
    )
    .unwrap();

    let preview = OpenSshConfigImporter::new()
        .preview(temp.path().join("config"))
        .unwrap();
    assert_eq!(
        preview
            .candidates
            .iter()
            .map(|candidate| candidate.host_pattern.as_str())
            .collect::<Vec<_>>(),
        vec!["quoted", "nested", "first", "second"]
    );
    assert_eq!(
        candidate(&preview, "quoted").identity_file.as_deref(),
        Some(Path::new("relative key"))
    );
}

#[test]
fn include_cycles_and_depth_limits_are_explicit_errors() {
    assert!(matches!(
        OpenSshConfigImporter::new().preview(fixture("cycle-a.conf")),
        Err(ImportError::IncludeCycle)
    ));

    let temp = tempfile::tempdir().unwrap();
    for index in 0..=8 {
        let next = if index == 8 {
            "Host final\n HostName final.example\n".to_owned()
        } else {
            format!("Include {}.conf\n", index + 1)
        };
        fs::write(temp.path().join(format!("{index}.conf")), next).unwrap();
    }
    assert!(
        OpenSshConfigImporter::new()
            .preview(temp.path().join("0.conf"))
            .is_ok()
    );
    fs::write(temp.path().join("8.conf"), "Include 9.conf\n").unwrap();
    fs::write(temp.path().join("9.conf"), "Host too-deep\n").unwrap();
    assert!(matches!(
        OpenSshConfigImporter::new().preview(temp.path().join("0.conf")),
        Err(ImportError::IncludeDepth)
    ));
}

#[test]
fn proxy_jump_keeps_the_alias_for_system_openssh_and_warns() {
    let preview = OpenSshConfigImporter::new()
        .preview(fixture("config"))
        .unwrap();
    let candidate = candidate(&preview, "bastion-target");

    assert!(candidate.importable);
    assert_eq!(candidate.proxy_jump.as_deref(), Some("jump.example"));
    assert_eq!(candidate.profile.transport, TransportKind::SystemOpenSsh);
    assert_eq!(candidate.profile.host, "bastion-target");
    assert!(
        candidate
            .warnings
            .contains(&ImportWarning::DependsOnOpenSshConfig)
    );
}

#[test]
fn multiple_identities_and_unsupported_dynamic_directives_warn_without_execution() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    fs::write(
        &config,
        "Host multi\n IdentityFile first\n IdentityFile second\nHost dynamic\n HostName %h.example\n IdentityFile ~/.ssh/id\n ProxyCommand ignored\nHost command-only\n ProxyCommand ignored\nMatch all\n User nobody\n",
    )
    .unwrap();

    let preview = OpenSshConfigImporter::new().preview(config).unwrap();
    let multi = candidate(&preview, "multi");
    assert_eq!(multi.identity_file.as_deref(), Some(Path::new("first")));
    assert!(
        multi
            .warnings
            .contains(&ImportWarning::MultipleIdentityFiles)
    );
    let dynamic = candidate(&preview, "dynamic");
    assert!(!dynamic.importable);
    assert!(dynamic.warnings.iter().any(|warning| matches!(
        warning,
        ImportWarning::DynamicValue { directive, value }
            if directive == "HostName" && value == "%h.example"
    )));
    assert!(dynamic.warnings.iter().any(|warning| matches!(
        warning,
        ImportWarning::UnsupportedDirective { directive } if directive == "ProxyCommand"
    )));
    assert!(dynamic.warnings.iter().any(|warning| matches!(
        warning,
        ImportWarning::UnsupportedDirective { directive } if directive == "Match"
    )));
    assert!(dynamic.warnings.iter().any(|warning| matches!(
        warning,
        ImportWarning::DynamicValue { directive, value }
            if directive == "IdentityFile" && value == "~/.ssh/id"
    )));
    assert!(!candidate(&preview, "command-only").importable);
}

#[test]
fn ipv6_option_like_hosts_and_invalid_ports_have_deterministic_importability() {
    let preview = OpenSshConfigImporter::new()
        .preview(fixture("config"))
        .unwrap();
    let ipv6 = candidate(&preview, "ipv6");
    assert!(ipv6.importable);
    assert_eq!(ipv6.host_name, "2001:db8::10");
    assert_eq!(ipv6.profile.host, "2001:db8::10");
    for alias in ["option-host", "-option-alias", "zero-port", "high-port"] {
        assert!(!candidate(&preview, alias).importable, "{alias}");
    }
    assert!(
        candidate(&preview, "option-host")
            .warnings
            .contains(&ImportWarning::InvalidHost {
                host: "-unsafe".into(),
            })
    );
    assert!(
        candidate(&preview, "zero-port")
            .warnings
            .contains(&ImportWarning::InvalidPort { value: "0".into() })
    );
    assert!(
        candidate(&preview, "high-port")
            .warnings
            .contains(&ImportWarning::InvalidPort {
                value: "65536".into(),
            })
    );
}

#[test]
fn commit_consumes_the_preview_imports_selected_literals_and_never_calls_the_vault() {
    let (repository, vault, coordinator) = setup_memory();
    let preview = OpenSshConfigImporter::new()
        .preview(fixture("config"))
        .unwrap();
    let production = candidate(&preview, "production").id;

    let report = OpenSshConfigImporter::new()
        .commit(&coordinator, preview, &[production].into_iter().collect())
        .unwrap();

    assert_eq!(report.imported_connections, 1);
    assert_eq!(report.skipped_connections, 10);
    assert_eq!(repository.load_catalog().unwrap().connections.len(), 1);
    let calls = vault.call_counts();
    assert_eq!(calls.get, 0);
    assert_eq!(calls.put, 0);
    assert_eq!(calls.delete, 0);
}

#[test]
fn invalid_or_duplicate_selection_is_rejected_before_any_vault_or_storage_write() {
    let (repository, vault, coordinator) = setup_memory();
    let importer = OpenSshConfigImporter::new();
    let mut preview = importer.preview(fixture("config")).unwrap();
    let original = repository.load_catalog().unwrap();
    #[cfg(feature = "test-support")]
    let original_tables = repository.test_visible_tables().unwrap();
    let first = preview
        .candidates
        .iter()
        .position(|item| item.importable)
        .unwrap();
    let second = preview
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, item)| item.importable)
        .nth(1)
        .unwrap()
        .0;
    preview.candidates[second].id = preview.candidates[first].id;
    let duplicated_id = preview.candidates[first].id;

    assert!(matches!(
        importer.commit(
            &coordinator,
            preview,
            &[duplicated_id].into_iter().collect()
        ),
        Err(ImportError::InvalidSelection)
    ));

    let mut duplicate_name_preview = importer.preview(fixture("config")).unwrap();
    let first = duplicate_name_preview
        .candidates
        .iter()
        .position(|item| item.importable)
        .unwrap();
    let second = duplicate_name_preview
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, item)| item.importable)
        .nth(1)
        .unwrap()
        .0;
    duplicate_name_preview.candidates[second].profile.name = duplicate_name_preview.candidates
        [first]
        .profile
        .name
        .clone();
    let duplicate_names = [
        duplicate_name_preview.candidates[first].id,
        duplicate_name_preview.candidates[second].id,
    ]
    .into_iter()
    .collect();
    assert!(matches!(
        importer.commit(&coordinator, duplicate_name_preview, &duplicate_names),
        Err(ImportError::InvalidSelection)
    ));

    let unknown_preview = importer.preview(fixture("config")).unwrap();
    assert!(matches!(
        importer.commit(
            &coordinator,
            unknown_preview,
            &[ConnectionId::new()].into_iter().collect(),
        ),
        Err(ImportError::InvalidSelection)
    ));

    let template_preview = importer.preview(fixture("config")).unwrap();
    let template = candidate(&template_preview, "*.corp").id;
    assert!(matches!(
        importer.commit(
            &coordinator,
            template_preview,
            &[template].into_iter().collect()
        ),
        Err(ImportError::InvalidSelection)
    ));
    assert_eq!(repository.load_catalog().unwrap(), original);
    #[cfg(feature = "test-support")]
    assert_eq!(repository.test_visible_tables().unwrap(), original_tables);
    let calls = vault.call_counts();
    assert_eq!(calls.get, 0);
    assert_eq!(calls.put, 0);
    assert_eq!(calls.delete, 0);
}

#[test]
fn injected_database_failure_rolls_back_the_entire_static_import() {
    let (repository, vault, coordinator) = setup_memory();
    let importer = OpenSshConfigImporter::new();
    let preview = importer.preview(fixture("config")).unwrap();
    let selection = selected(&preview);
    let before = repository.load_catalog().unwrap();
    #[cfg(feature = "test-support")]
    let before_tables = repository.test_visible_tables().unwrap();
    repository.inject_statement_failure_once(1).unwrap();

    assert!(matches!(
        importer.commit(&coordinator, preview, &selection),
        Err(ImportError::Credential(
            CredentialOperationError::ReconciliationRequired
        ))
    ));
    assert_eq!(repository.load_catalog().unwrap(), before);
    #[cfg(feature = "test-support")]
    assert_eq!(repository.test_visible_tables().unwrap(), before_tables);
    let calls = vault.call_counts();
    assert_eq!(calls.get, 0);
    assert_eq!(calls.put, 0);
    assert_eq!(calls.delete, 0);
}
