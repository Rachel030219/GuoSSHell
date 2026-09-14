use std::{collections::BTreeSet, path::PathBuf};

use rshell_core::connection::{
    AuthenticationKind, CatalogMutation, CatalogOutcome, ConnectionCatalog, ConnectionGroup,
    ConnectionProfile, CredentialRef, DomainError, TerminalOverrides, TransportKind,
};

fn tags(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn password_profile(name: &str, host: &str) -> ConnectionProfile {
    ConnectionProfile {
        name: format!("  {name}  "),
        host: format!("  {host}  "),
        username: "  deploy  ".into(),
        transport: TransportKind::NativeSsh,
        authentication: AuthenticationKind::Password,
        credential_ref: Some(CredentialRef::new("credential://deploy")),
        remote_command: Some("  echo ready  ".into()),
        tags: tags(&["  Production  ", "Linux"]),
        terminal_overrides: TerminalOverrides::default(),
        ..ConnectionProfile::new(name, host)
    }
}

fn create_group(catalog: &mut ConnectionCatalog, name: &str) -> ConnectionGroup {
    let group = ConnectionGroup::new(name);
    assert_eq!(
        catalog.apply(CatalogMutation::CreateGroup(group.clone())),
        Ok(CatalogOutcome::Group(group.id))
    );
    group
}

fn create_profile(
    catalog: &mut ConnectionCatalog,
    profile: ConnectionProfile,
) -> ConnectionProfile {
    let id = profile.id;
    assert_eq!(
        catalog
            .apply(CatalogMutation::Create(profile.clone()))
            .and_then(CatalogOutcome::connection_id),
        Ok(id)
    );
    profile
}

#[test]
fn catalog_rejects_invalid_explicit_terminal_overrides() {
    let mut catalog = ConnectionCatalog::default();
    let mut profile = ConnectionProfile::new("invalid override", "host.example.test");
    profile.terminal_overrides.answerback = Some("  ".into());

    assert!(matches!(
        catalog.apply(CatalogMutation::Create(profile)),
        Err(DomainError::InvalidTerminalOverride {
            code: rshell_core::SettingsValidationCode::Blank,
            ..
        })
    ));
    assert!(catalog.connections.is_empty());
}

#[test]
fn catalog_supports_connection_and_group_crud() {
    let mut catalog = ConnectionCatalog::default();
    let group = create_group(&mut catalog, "  Production  ");

    assert_eq!(
        catalog.apply(CatalogMutation::RenameGroup {
            group: group.id,
            name: "  Primary production  ".into(),
        }),
        Ok(CatalogOutcome::Updated)
    );

    let mut profile = password_profile("web-01", "web.example.test");
    profile.group_id = Some(group.id);
    let mut profile = create_profile(&mut catalog, profile);
    profile.name = "  web-primary  ".into();
    assert_eq!(
        catalog.apply(CatalogMutation::Update(profile.clone())),
        Ok(CatalogOutcome::Updated)
    );
    assert_eq!(catalog.connections[&profile.id].name, "web-primary");
    assert_eq!(catalog.connections[&profile.id].host, "web.example.test");
    assert_eq!(catalog.connections[&profile.id].username, "deploy");
    assert_eq!(
        catalog.connections[&profile.id].remote_command,
        Some("echo ready".into())
    );
    assert_eq!(catalog.groups[&group.id].name, "Primary production");
    assert!(CatalogOutcome::Updated.connection_id().is_err());

    assert_eq!(
        catalog.apply(CatalogMutation::Delete(profile.id)),
        Ok(CatalogOutcome::Deleted)
    );
    assert_eq!(
        catalog.apply(CatalogMutation::DeleteGroup(group.id)),
        Ok(CatalogOutcome::Deleted)
    );
    assert!(catalog.groups.is_empty());
    assert!(catalog.connections.is_empty());
}

#[test]
fn duplicate_and_move_preserve_credential_reference_and_normalize_positions() {
    let mut catalog = ConnectionCatalog::default();
    let source_group = create_group(&mut catalog, "Source");
    let target_group = create_group(&mut catalog, "Target");

    let mut source = password_profile("build", "build.example.test");
    source.group_id = Some(source_group.id);
    source.position = 80;
    let source = create_profile(&mut catalog, source);
    let credential_ref = source.credential_ref.clone();

    let copied_id = catalog
        .apply(CatalogMutation::Duplicate {
            source: source.id,
            destination: Some(source_group.id),
        })
        .and_then(CatalogOutcome::connection_id)
        .unwrap();
    let copied = &catalog.connections[&copied_id];
    assert_ne!(copied.id, source.id);
    assert_eq!(copied.name, "build copy");
    assert_eq!(copied.credential_ref, credential_ref);

    assert_eq!(
        catalog.apply(CatalogMutation::Move {
            connection: copied_id,
            destination: Some(target_group.id),
            position: 0,
        }),
        Ok(CatalogOutcome::Updated)
    );
    assert_eq!(catalog.ordered_ids(Some(source_group.id)), vec![source.id]);
    assert_eq!(catalog.ordered_ids(Some(target_group.id)), vec![copied_id]);
    assert!(
        catalog
            .connections
            .values()
            .all(|profile| profile.position == 0)
    );
}

#[test]
fn set_tags_and_unicode_search_match_normalized_fields() {
    let mut catalog = ConnectionCatalog::default();
    let mut profile = password_profile("BÜRO gateway", "MÜNCHEN.example.test");
    profile.username = "  ΔΕΛΤΑ  ".into();
    let profile = create_profile(&mut catalog, profile);

    assert_eq!(
        catalog.apply(CatalogMutation::SetTags {
            connection: profile.id,
            tags: tags(&["  PRÖD  ", "  数据库  ", "PRÖD"]),
        }),
        Ok(CatalogOutcome::Updated)
    );
    for query in ["büro", "münchen", "δελ", "pröd", "数据库"] {
        assert_eq!(
            catalog.search(query),
            vec![profile.id],
            "query {query:?} should find the connection"
        );
    }
    assert_eq!(
        catalog.connections[&profile.id].tags,
        tags(&["PRÖD", "数据库"])
    );
}

#[test]
fn ordered_ids_are_stable_and_positions_are_contiguous_per_group() {
    let mut catalog = ConnectionCatalog::default();
    let group = create_group(&mut catalog, "Ordered");
    let mut first = password_profile("first", "first.example.test");
    first.group_id = Some(group.id);
    first.position = 100;
    let first = create_profile(&mut catalog, first);

    let mut second = password_profile("second", "second.example.test");
    second.group_id = Some(group.id);
    second.position = 0;
    let second = create_profile(&mut catalog, second);

    assert_eq!(
        catalog.ordered_ids(Some(group.id)),
        vec![second.id, first.id]
    );
    let positions = catalog
        .ordered_ids(Some(group.id))
        .into_iter()
        .map(|id| catalog.connections[&id].position)
        .collect::<Vec<_>>();
    assert_eq!(positions, vec![0_i64, 1]);
}

#[test]
fn validation_rejects_invalid_hosts_ports_and_authentication() {
    let mut catalog = ConnectionCatalog::default();

    let invalid_host = password_profile("bad-host", " -forbidden.example.test");
    assert!(matches!(
        catalog.apply(CatalogMutation::Create(invalid_host)),
        Err(DomainError::InvalidHost { .. })
    ));

    let mut invalid_port = password_profile("bad-port", "port.example.test");
    invalid_port.port = 0;
    assert!(matches!(
        catalog.apply(CatalogMutation::Create(invalid_port)),
        Err(DomainError::InvalidPort { port: 0 })
    ));
    assert!("65536".parse::<u16>().is_err());

    let mut keyboard_interactive = ConnectionProfile::new("keyboard", "keyboard.example.test");
    keyboard_interactive.transport = TransportKind::NativeSsh;
    keyboard_interactive.authentication = AuthenticationKind::KeyboardInteractive;
    create_profile(&mut catalog, keyboard_interactive);

    let mut public_key = ConnectionProfile::new("key", "key.example.test");
    public_key.authentication = AuthenticationKind::PublicKey;
    public_key.identity_file = Some(PathBuf::from("  C:/keys/id_ed25519  "));
    let public_key = create_profile(&mut catalog, public_key);
    assert_eq!(
        catalog.connections[&public_key.id].identity_file,
        Some(PathBuf::from("C:/keys/id_ed25519"))
    );

    let mut incompatible_authentication = password_profile("bad-auth", "auth.example.test");
    incompatible_authentication.transport = TransportKind::SystemOpenSsh;
    assert!(matches!(
        catalog.apply(CatalogMutation::Create(incompatible_authentication)),
        Err(DomainError::InvalidAuthentication { .. })
    ));

    let mut missing_identity = password_profile("bad-key", "key.example.test");
    missing_identity.authentication = AuthenticationKind::PublicKey;
    missing_identity.credential_ref = None;
    assert!(matches!(
        catalog.apply(CatalogMutation::Create(missing_identity)),
        Err(DomainError::MissingIdentityFile { .. })
    ));

    let mut missing_credential = password_profile("bad-password", "password.example.test");
    missing_credential.credential_ref = None;
    assert!(matches!(
        catalog.apply(CatalogMutation::Create(missing_credential)),
        Err(DomainError::MissingCredentialRef { .. })
    ));
}

#[test]
fn groups_cannot_cycle_or_be_deleted_while_non_empty() {
    let mut catalog = ConnectionCatalog::default();
    let parent = create_group(&mut catalog, "Parent");
    let mut child = ConnectionGroup::new("Child");
    child.parent_id = Some(parent.id);
    assert_eq!(
        catalog.apply(CatalogMutation::CreateGroup(child.clone())),
        Ok(CatalogOutcome::Group(child.id))
    );

    assert_eq!(
        catalog.apply(CatalogMutation::RenameGroup {
            group: child.id,
            name: "  Child renamed  ".into(),
        }),
        Ok(CatalogOutcome::Updated)
    );
    assert_eq!(catalog.groups[&child.id].name, "Child renamed");
    assert_eq!(
        catalog.apply(CatalogMutation::MoveGroup {
            group: child.id,
            parent: None,
            position: 0,
        }),
        Ok(CatalogOutcome::Updated)
    );
    assert_eq!(catalog.groups[&child.id].parent_id, None);
    assert_eq!(
        catalog.apply(CatalogMutation::MoveGroup {
            group: child.id,
            parent: Some(parent.id),
            position: 0,
        }),
        Ok(CatalogOutcome::Updated)
    );
    assert!(matches!(
        catalog.apply(CatalogMutation::MoveGroup {
            group: parent.id,
            parent: Some(child.id),
            position: 0,
        }),
        Err(DomainError::GroupCycle { .. })
    ));

    let mut profile = password_profile("child-host", "child.example.test");
    profile.group_id = Some(child.id);
    let profile = create_profile(&mut catalog, profile);
    assert!(matches!(
        catalog.delete_group(child.id),
        Err(DomainError::GroupNotEmpty { .. })
    ));
    assert!(matches!(
        catalog.delete_group(parent.id),
        Err(DomainError::GroupNotEmpty { .. })
    ));

    assert_eq!(
        catalog.apply(CatalogMutation::Delete(profile.id)),
        Ok(CatalogOutcome::Deleted)
    );
    assert_eq!(catalog.delete_group(child.id), Ok(()));
    assert_eq!(
        catalog.apply(CatalogMutation::DeleteGroup(parent.id)),
        Ok(CatalogOutcome::Deleted)
    );
}
