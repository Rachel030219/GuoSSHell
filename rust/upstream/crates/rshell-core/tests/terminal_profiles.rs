use rshell_core::{
    SettingsValidationCode, TerminalKeyAction, TerminalSendSequence,
    connection::{ConnectionProfile, TerminalOverrides},
    parse_terminal_key_action,
    terminal::{
        AppSettings, ColorScheme, KeyBinding, KeyCode, KeyModifiers, TerminalProfile,
        TerminalSettingsV1,
    },
    validate_terminal_overrides, validate_terminal_settings,
};

#[test]
fn explicit_terminal_overrides_reject_invalid_values_without_changing_resolution_compatibility() {
    let duplicate = KeyBinding {
        code: KeyCode::F(2),
        modifiers: KeyModifiers::default(),
        action: "new_tab".into(),
    };
    let cases = [
        (
            TerminalOverrides {
                terminal_type: Some("  ".into()),
                ..Default::default()
            },
            SettingsValidationCode::Blank,
        ),
        (
            TerminalOverrides {
                initial_cols: Some(0),
                ..Default::default()
            },
            SettingsValidationCode::OutOfRange,
        ),
        (
            TerminalOverrides {
                font_size: Some(f32::NAN),
                ..Default::default()
            },
            SettingsValidationCode::NonFinite,
        ),
        (
            TerminalOverrides {
                key_bindings: Some(vec![duplicate.clone(), duplicate]),
                ..Default::default()
            },
            SettingsValidationCode::DuplicateBinding,
        ),
    ];

    for (overrides, expected) in cases {
        assert_eq!(
            validate_terminal_overrides(&overrides).unwrap_err().code,
            expected
        );
    }
}

#[test]
fn overrides_merge_over_global_and_clamp_terminal_values() {
    let settings = TerminalSettingsV1 {
        terminal_type: " custom-term ".into(),
        initial_cols: 0,
        initial_rows: 1_000,
        scrollback_lines: 0,
        font_family: "Iosevka".into(),
        font_size: 100.0,
        color_scheme: ColorScheme::Nord,
        key_bindings: vec![KeyBinding {
            code: KeyCode::F(2),
            modifiers: KeyModifiers::default(),
            action: "new_tab".into(),
        }],
        ..TerminalSettingsV1::default()
    };
    let overrides = TerminalOverrides {
        initial_cols: Some(1_000),
        initial_rows: Some(0),
        scrollback_lines: Some(1),
        font_family: Some("Consolas".into()),
        font_size: Some(80.0),
        color_scheme: Some(ColorScheme::Dracula),
        ..TerminalOverrides::default()
    };

    let resolved = settings.resolve(&overrides);

    assert_eq!(resolved.terminal_type, "custom-term");
    assert_eq!((resolved.cols, resolved.rows), (999, 1));
    assert_eq!(resolved.scrollback_lines, 100);
    assert_eq!(resolved.font_family, "Consolas");
    assert_eq!(resolved.font_size, 72.0);
    assert_eq!(resolved.color_scheme, ColorScheme::Dracula);
    assert_eq!(resolved.key_bindings, settings.key_bindings);
}

#[test]
fn settings_round_trip_always_contains_version_one() {
    let settings = TerminalSettingsV1::default();
    let serialized = serde_json::to_value(&settings).unwrap();

    assert_eq!(serialized["version"], 1);
    assert_eq!(
        serde_json::from_value::<TerminalSettingsV1>(serialized).unwrap(),
        settings
    );
    let mut unsupported = serde_json::to_value(TerminalSettingsV1::default()).unwrap();
    unsupported["version"] = serde_json::json!(2);
    assert!(serde_json::from_value::<TerminalSettingsV1>(unsupported).is_err());
}

#[test]
fn default_profile_settings_and_connection_path_are_stable() {
    let profile = TerminalProfile::p0_default();
    let app_settings = AppSettings::default();
    let connection = ConnectionProfile::new("example", "example.test");

    assert_eq!(profile.settings.terminal_type, "xterm-256color");
    assert_eq!(
        (profile.settings.initial_cols, profile.settings.initial_rows),
        (120, 36)
    );
    assert_eq!(profile.settings.scrollback_lines, 6_000);
    assert_eq!(profile.settings.font_family, "CaskaydiaCove NF Mono");
    assert_eq!(profile.settings.font_size, 18.0);
    assert!(profile.settings.left_alt_as_meta);
    assert!(profile.settings.right_alt_as_meta);
    assert!(!profile.settings.enable_csi_u);
    assert!(!profile.settings.enable_kitty_keyboard);
    assert!(profile.settings.mouse_reporting);
    assert!(profile.settings.scroll_on_output);
    assert!(!profile.settings.scroll_on_keypress);
    assert_eq!(profile.settings.answerback, "rsHell");
    assert_eq!(app_settings.default_terminal_profile, profile.id);
    assert_eq!(connection.terminal_profile_id, None);
    assert_eq!(connection.terminal_overrides, TerminalOverrides::default());
    let resolved = profile.settings.resolve(&connection.terminal_overrides);
    assert_eq!(resolved.font_family, profile.settings.font_family);
    assert_eq!(resolved.font_size, profile.settings.font_size);
}

#[test]
fn key_actions_are_closed_and_validated() {
    let accepted = [
        (
            "send:\u{1b}[3~",
            TerminalKeyAction::Send(TerminalSendSequence::Vt220Delete),
        ),
        (
            "send:\u{7f}",
            TerminalKeyAction::Send(TerminalSendSequence::Delete127),
        ),
        (
            "send:\u{8}",
            TerminalKeyAction::Send(TerminalSendSequence::Backspace8),
        ),
        ("clear_scrollback", TerminalKeyAction::ClearScrollback),
        ("new_tab", TerminalKeyAction::NewTab),
        ("split_vertical", TerminalKeyAction::SplitVertical),
    ];
    for (encoded, expected) in accepted {
        assert_eq!(parse_terminal_key_action(encoded).unwrap(), expected);
        let settings = settings_with_action(encoded);
        validate_terminal_settings(&settings).expect("closed action validates");
    }

    for invalid in [
        "",
        " ",
        "send:",
        "send:printable",
        "send:\0",
        "send:\u{1b}[A",
        "send:\u{1b}[3~extra",
        "split_horizontal",
        "copy",
        "arbitrary_action",
    ] {
        assert_eq!(
            parse_terminal_key_action(invalid).unwrap_err().code,
            SettingsValidationCode::InvalidAction,
            "{invalid:?} must be rejected"
        );
        assert_eq!(
            validate_terminal_settings(&settings_with_action(invalid))
                .unwrap_err()
                .code,
            SettingsValidationCode::InvalidAction,
            "{invalid:?} must fail persisted settings validation"
        );
    }
}

#[test]
fn profile_and_connection_bindings_shadow_app_bindings_by_exact_chord() {
    let shadowed = binding(KeyCode::F(2), KeyModifiers::default(), "clear_scrollback");
    let profile_only = binding(KeyCode::F(3), KeyModifiers::default(), "new_tab");
    let app_shadowed = binding(KeyCode::F(2), KeyModifiers::default(), "split_vertical");
    let app_only = binding(KeyCode::F(4), KeyModifiers::default(), "split_vertical");
    let settings = TerminalSettingsV1 {
        key_bindings: vec![profile_only],
        ..TerminalSettingsV1::default()
    };
    let overrides = TerminalOverrides {
        key_bindings: Some(vec![shadowed.clone()]),
        ..TerminalOverrides::default()
    };

    let resolved = settings
        .resolve(&overrides)
        .with_app_key_bindings(&[app_shadowed, app_only.clone()]);

    assert_eq!(resolved.key_bindings, vec![shadowed, app_only]);
}

fn settings_with_action(action: &str) -> TerminalSettingsV1 {
    TerminalSettingsV1 {
        key_bindings: vec![binding(KeyCode::F(2), KeyModifiers::default(), action)],
        ..TerminalSettingsV1::default()
    }
}

fn binding(code: KeyCode, modifiers: KeyModifiers, action: &str) -> KeyBinding {
    KeyBinding {
        code,
        modifiers,
        action: action.to_owned(),
    }
}
