use std::{collections::BTreeSet, fs, path::PathBuf};

use rshell_core::{
    AuthenticationKind, CatalogMutation, CatalogOutcome, ColorScheme, ConnectionGroup,
    ConnectionProfile, CredentialRef, HostKeyPolicy, TerminalOverrides, TerminalProfile,
    TransportKind,
};
use rshell_storage::SqliteRepository;

fn tags(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn rich_profile(group: rshell_core::GroupId) -> ConnectionProfile {
    ConnectionProfile {
        group_id: Some(group),
        name: "生产 gateway".into(),
        host: "gateway.example.test".into(),
        port: 2202,
        username: "deploy".into(),
        transport: TransportKind::NativeSsh,
        authentication: AuthenticationKind::Password,
        credential_ref: Some(CredentialRef::new("credential://gateway")),
        identity_file: Some(PathBuf::from("C:/用户/密钥/id_ed25519")),
        host_key_policy: HostKeyPolicy::Strict,
        remote_command: Some("tmux new-session -A -s ops".into()),
        note: "critical endpoint".into(),
        tags: tags(&["linux", "production"]),
        position: 19,
        terminal_profile_id: Some(TerminalProfile::p0_default().id),
        terminal_overrides: TerminalOverrides {
            font_family: Some("Iosevka".into()),
            font_size: Some(14.0),
            color_scheme: Some(ColorScheme::Nord),
            scroll_on_keypress: Some(true),
            ..TerminalOverrides::default()
        },
        ..ConnectionProfile::new("ignored", "ignored.test")
    }
}

fn create_group(repository: &SqliteRepository, group: ConnectionGroup) {
    assert_eq!(
        repository
            .apply(CatalogMutation::CreateGroup(group.clone()))
            .unwrap(),
        CatalogOutcome::Group(group.id)
    );
}

#[test]
fn catalog_round_trips_nested_groups_connections_tags_and_ordering() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    repository.migrate().unwrap();
    let parent = ConnectionGroup::new("Parent");
    create_group(&repository, parent.clone());
    let mut child = ConnectionGroup::new("Child");
    child.parent_id = Some(parent.id);
    child.position = 8;
    create_group(&repository, child.clone());
    let profile = rich_profile(child.id);

    assert_eq!(
        repository
            .apply(CatalogMutation::Create(profile.clone()))
            .unwrap(),
        CatalogOutcome::Connection(profile.id)
    );
    let catalog = repository.load_catalog().unwrap();
    assert_eq!(catalog.groups.len(), 2);
    assert_eq!(catalog.groups[&child.id].parent_id, Some(parent.id));
    let stored = &catalog.connections[&profile.id];
    assert_eq!(stored.name, profile.name);
    assert_eq!(stored.host, profile.host);
    assert_eq!(stored.port, profile.port);
    assert_eq!(stored.username, profile.username);
    assert_eq!(stored.transport, profile.transport);
    assert_eq!(stored.authentication, profile.authentication);
    assert_eq!(stored.credential_ref, profile.credential_ref);
    assert_eq!(stored.identity_file, profile.identity_file);
    assert_eq!(stored.host_key_policy, profile.host_key_policy);
    assert_eq!(stored.remote_command, profile.remote_command);
    assert_eq!(stored.note, profile.note);
    assert_eq!(stored.tags, profile.tags);
    assert_eq!(stored.terminal_profile_id, profile.terminal_profile_id);
    assert_eq!(stored.terminal_overrides, profile.terminal_overrides);
    assert_eq!(catalog.ordered_ids(Some(child.id)), vec![profile.id]);
    repository.shutdown().unwrap();
}

#[test]
fn repository_persists_every_catalog_mutation() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    repository.migrate().unwrap();
    let first = ConnectionGroup::new("First");
    let second = ConnectionGroup::new("Second");
    create_group(&repository, first.clone());
    create_group(&repository, second.clone());
    let mut profile = rich_profile(first.id);
    repository
        .apply(CatalogMutation::Create(profile.clone()))
        .unwrap();

    profile.name = "renamed connection".into();
    assert_eq!(
        repository
            .apply(CatalogMutation::Update(profile.clone()))
            .unwrap(),
        CatalogOutcome::Updated
    );
    let duplicate = repository
        .apply(CatalogMutation::Duplicate {
            source: profile.id,
            destination: Some(first.id),
        })
        .unwrap()
        .connection_id()
        .unwrap();
    repository
        .apply(CatalogMutation::Move {
            connection: duplicate,
            destination: Some(second.id),
            position: 0,
        })
        .unwrap();
    repository
        .apply(CatalogMutation::SetTags {
            connection: duplicate,
            tags: tags(&["copied", "moved"]),
        })
        .unwrap();
    repository
        .apply(CatalogMutation::RenameGroup {
            group: second.id,
            name: "Renamed second".into(),
        })
        .unwrap();
    repository
        .apply(CatalogMutation::MoveGroup {
            group: second.id,
            parent: Some(first.id),
            position: 0,
        })
        .unwrap();

    let catalog = repository.load_catalog().unwrap();
    assert_eq!(catalog.connections[&profile.id].name, "renamed connection");
    assert_eq!(catalog.connections[&duplicate].group_id, Some(second.id));
    assert_eq!(
        catalog.connections[&duplicate].tags,
        tags(&["copied", "moved"])
    );
    assert_eq!(catalog.groups[&second.id].name, "Renamed second");
    assert_eq!(catalog.groups[&second.id].parent_id, Some(first.id));

    repository
        .apply(CatalogMutation::Delete(duplicate))
        .unwrap();
    repository
        .apply(CatalogMutation::MoveGroup {
            group: second.id,
            parent: None,
            position: 0,
        })
        .unwrap();
    repository
        .apply(CatalogMutation::DeleteGroup(second.id))
        .unwrap();
    assert!(
        !repository
            .load_catalog()
            .unwrap()
            .groups
            .contains_key(&second.id)
    );
    repository.shutdown().unwrap();
}

#[test]
fn file_database_reopens_with_an_identical_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.sqlite3");
    let repository = SqliteRepository::open(&path).unwrap();
    repository.migrate().unwrap();
    let group = ConnectionGroup::new("Persistent");
    create_group(&repository, group.clone());
    let profile = rich_profile(group.id);
    repository
        .apply(CatalogMutation::Create(profile.clone()))
        .unwrap();
    let expected = repository.load_catalog().unwrap();
    repository.shutdown().unwrap();

    let reopened = SqliteRepository::open(&path).unwrap();
    reopened.migrate().unwrap();
    assert_eq!(reopened.load_catalog().unwrap(), expected);
    reopened.shutdown().unwrap();
}

#[test]
fn fixture_secret_never_appears_in_database_wal_or_shm() {
    let fixture_secret = "rshell-secret-fixture-7bf8f37d";
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.sqlite3");
    let repository = SqliteRepository::open(&path).unwrap();
    repository.migrate().unwrap();
    let group = ConnectionGroup::new("No secrets");
    create_group(&repository, group.clone());
    repository
        .apply(CatalogMutation::Create(rich_profile(group.id)))
        .unwrap();
    repository.shutdown().unwrap();

    for candidate in [
        path.clone(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        if candidate.exists() {
            let bytes = fs::read(&candidate).unwrap();
            assert!(
                !bytes
                    .windows(fixture_secret.len())
                    .any(|window| window == fixture_secret.as_bytes()),
                "secret found in {}",
                candidate.display()
            );
        }
    }
}

#[cfg(feature = "test-support")]
#[test]
fn foreign_key_restriction_and_tag_cascade_are_enforced() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    repository.migrate().unwrap();
    let group = ConnectionGroup::new("FK");
    create_group(&repository, group.clone());
    let profile = rich_profile(group.id);
    repository
        .apply(CatalogMutation::Create(profile.clone()))
        .unwrap();

    assert!(
        repository
            .test_delete_terminal_profile(TerminalProfile::p0_default().id)
            .is_err()
    );
    assert_eq!(
        repository.test_delete_connection_only(profile.id).unwrap(),
        0
    );
    repository.shutdown().unwrap();
}

#[cfg(feature = "test-support")]
#[test]
fn injected_second_statement_failure_rolls_back_every_visible_table() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    repository.migrate().unwrap();
    let group = ConnectionGroup::new("Atomic");
    create_group(&repository, group.clone());
    let before = repository.test_visible_tables().unwrap();
    repository.inject_statement_failure_once(2).unwrap();

    let error = repository
        .apply(CatalogMutation::Create(rich_profile(group.id)))
        .unwrap_err();

    assert_eq!(format!("{error}"), "storage constraint failed");
    assert_eq!(repository.test_visible_tables().unwrap(), before);
    repository
        .apply(CatalogMutation::Create(rich_profile(group.id)))
        .unwrap();
    repository.shutdown().unwrap();
}

#[cfg(feature = "test-support")]
#[test]
fn unknown_enum_and_override_versions_are_reported_as_corrupt() {
    use rshell_storage::{StorageError, TestConnectionCorruption};

    for corruption in [
        TestConnectionCorruption::UnknownTransport,
        TestConnectionCorruption::UnsupportedOverridesVersion,
    ] {
        let repository = SqliteRepository::open_in_memory().unwrap();
        repository.migrate().unwrap();
        let group = ConnectionGroup::new("Corrupt mapping");
        create_group(&repository, group.clone());
        let profile = rich_profile(group.id);
        repository
            .apply(CatalogMutation::Create(profile.clone()))
            .unwrap();
        repository
            .test_corrupt_connection(profile.id, corruption)
            .unwrap();
        assert_eq!(repository.load_catalog(), Err(StorageError::Corrupt));
        repository.shutdown().unwrap();
    }
}
