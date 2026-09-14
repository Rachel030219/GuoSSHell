#[cfg(feature = "test-support")]
use std::collections::BTreeSet;

use rshell_core::{
    AppSettings, ColorScheme, KeyBinding, KeyCode, KeyModifiers, TerminalProfile,
    TerminalSettingsV1,
};
use rshell_storage::{SqliteRepository, StorageError};

#[test]
fn migration_is_monotonic_idempotent_and_seeds_defaults() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    assert_eq!(repository.schema_versions().unwrap(), Vec::<i64>::new());

    repository.migrate().unwrap();
    repository.migrate().unwrap();

    assert_eq!(repository.schema_versions().unwrap(), vec![1, 2]);
    assert_eq!(
        repository.load_terminal_profiles().unwrap(),
        vec![TerminalProfile::p0_default()]
    );
    assert_eq!(repository.load_settings().unwrap(), AppSettings::default());
    repository.shutdown().unwrap();
}

#[test]
fn profile_and_settings_json_round_trip_all_versioned_fields() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    repository.migrate().unwrap();
    let profile = TerminalProfile {
        name: "Unicode 配置".into(),
        settings: TerminalSettingsV1 {
            terminal_type: "screen-256color".into(),
            initial_cols: 132,
            initial_rows: 44,
            scrollback_lines: 42_000,
            font_family: "Cascadia Mono".into(),
            font_size: 13.5,
            color_scheme: ColorScheme::TokyoNight,
            key_bindings: vec![KeyBinding {
                code: KeyCode::F(6),
                modifiers: KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
                action: "split_vertical".into(),
            }],
            left_alt_as_meta: false,
            right_alt_as_meta: true,
            enable_csi_u: true,
            enable_kitty_keyboard: true,
            mouse_reporting: false,
            scroll_on_output: false,
            scroll_on_keypress: true,
            answerback: "custom-answer".into(),
            ..TerminalSettingsV1::default()
        },
        ..TerminalProfile::default()
    };
    repository.save_terminal_profile(profile.clone()).unwrap();
    let settings = AppSettings {
        default_terminal_profile: profile.id,
        color_scheme: ColorScheme::GruvboxDark,
        key_bindings: profile.settings.key_bindings.clone(),
    };
    repository.save_settings(settings.clone()).unwrap();

    assert!(
        repository
            .load_terminal_profiles()
            .unwrap()
            .contains(&profile)
    );
    assert_eq!(repository.load_settings().unwrap(), settings);
    repository.shutdown().unwrap();
}

#[test]
fn reopening_preserves_explicit_old_default_font_instead_of_reseeding_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("font-profile.sqlite3");
    let repository = SqliteRepository::open(&path).unwrap();
    repository.migrate().unwrap();
    let saved = TerminalProfile {
        settings: TerminalSettingsV1 {
            font_family: "Cascadia Mono".into(),
            font_size: 15.0,
            ..TerminalSettingsV1::default()
        },
        ..TerminalProfile::default()
    };
    repository.save_terminal_profile(saved.clone()).unwrap();
    repository.shutdown().unwrap();

    let reopened = SqliteRepository::open(&path).unwrap();
    reopened.migrate().unwrap();
    assert_eq!(reopened.load_terminal_profiles().unwrap(), vec![saved]);
    reopened.shutdown().unwrap();
}

#[test]
fn file_database_uses_required_pragmas_and_private_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested").join("catalog.sqlite3");
    let repository = SqliteRepository::open(&path).unwrap();
    repository.migrate().unwrap();

    let status = repository.database_status().unwrap();
    assert!(status.foreign_keys);
    assert_eq!(status.busy_timeout_ms, 5_000);
    assert_eq!(status.journal_mode, "wal");
    assert_eq!(status.private_file_is_secure, Some(true));
    repository.shutdown().unwrap();
}

#[test]
fn in_memory_database_reports_connection_pragmas() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    let status = repository.database_status().unwrap();
    assert!(status.foreign_keys);
    assert_eq!(status.busy_timeout_ms, 5_000);
    assert_eq!(status.journal_mode, "memory");
    assert_eq!(status.private_file_is_secure, None);
    repository.shutdown().unwrap();
}

#[test]
fn shutdown_is_explicit_and_idempotent() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    repository.shutdown().unwrap();
    repository.shutdown().unwrap();
    assert_eq!(repository.schema_versions(), Err(StorageError::QueueClosed));
}

#[cfg(feature = "test-support")]
#[test]
fn schema_contains_task_five_tables_indexes_and_checks() {
    use rshell_storage::TestCredentialValue;

    let repository = SqliteRepository::open_in_memory().unwrap();
    repository.migrate().unwrap();
    let schema = repository.test_schema().unwrap();
    let names = schema.keys().cloned().collect::<BTreeSet<_>>();
    for required in [
        "app_settings",
        "app_setting_values",
        "connection_groups",
        "connection_tags",
        "connections",
        "credential_operations",
        "idx_connection_groups_parent_position",
        "idx_connection_tags_tag",
        "idx_connections_group_position",
        "idx_connections_search",
        "schema_migrations",
        "terminal_profiles",
    ] {
        assert!(names.contains(required), "missing schema object {required}");
    }
    let operations = &schema["credential_operations"];
    assert!(operations.contains("put_new"));
    assert!(operations.contains("delete_old"));
    assert!(operations.contains("prepared"));
    assert!(operations.contains("vault_applied"));
    let connections = &schema["connections"];
    for column in [
        "id",
        "group_id",
        "name",
        "host",
        "port",
        "username",
        "transport",
        "authentication",
        "credential_ref",
        "identity_file",
        "host_key_policy",
        "remote_command",
        "note",
        "position",
        "terminal_profile_id",
        "terminal_overrides_json",
    ] {
        assert!(
            connections.contains(column),
            "missing connection column {column}"
        );
    }
    assert!(
        repository
            .test_credential_operation(TestCredentialValue::Valid, TestCredentialValue::Valid)
            .is_ok()
    );
    assert!(
        repository
            .test_credential_operation(TestCredentialValue::Invalid, TestCredentialValue::Valid)
            .is_err()
    );
    assert!(
        repository
            .test_credential_operation(TestCredentialValue::Valid, TestCredentialValue::Invalid)
            .is_err()
    );
    repository.shutdown().unwrap();
}

#[cfg(feature = "test-support")]
#[test]
fn worker_panic_and_disconnect_are_reported_as_crashed() {
    let repository = SqliteRepository::open_in_memory().unwrap();
    assert_eq!(repository.test_crash_worker(), Err(StorageError::Crashed));
    assert_eq!(repository.schema_versions(), Err(StorageError::Crashed));
}
