use std::sync::Arc;

use rshell_core::{
    KeyModifiers,
    connection::{CatalogMutation, ConnectionProfile, PaneId, SessionId},
    protocol::{
        AppEvent, AuthPrompt, HostKeyDecision, InteractionId, InteractionResponse,
        KeyboardInteractivePrompt, SecretUpdate, SessionUiCommand, SessionUiEvent, UiCommand,
    },
    render::{
        CellAttributes, CellPosition, Color, MouseButton, MouseEventKind, RenderCell, RenderFrame,
        RenderRow, TerminalDisplayModes, TerminalMouseEvent, TerminalSize, Viewport,
    },
};
use secrecy::SecretString;

fn secret() -> SecretString {
    SecretString::from("fixture-secret-must-never-appear".to_owned())
}

#[test]
fn mouse_event_serde_preserves_viewport_row_and_defaults_legacy_values() {
    let event = TerminalMouseEvent {
        kind: MouseEventKind::Press,
        button: Some(MouseButton::Left),
        cell: CellPosition {
            stable_row: 101,
            column: 3,
        },
        viewport_row: 1,
        pixel_x: 24,
        pixel_y: 32,
        modifiers: KeyModifiers::default(),
    };
    let mut legacy = serde_json::to_value(event).unwrap();
    legacy.as_object_mut().unwrap().remove("viewport_row");

    assert_eq!(
        serde_json::from_value::<TerminalMouseEvent>(legacy)
            .unwrap()
            .viewport_row,
        0
    );
    assert_eq!(
        serde_json::from_value::<TerminalMouseEvent>(serde_json::to_value(event).unwrap()).unwrap(),
        event
    );
}

fn assert_redacted(value: impl std::fmt::Debug) {
    let output = format!("{value:?}");
    assert!(
        !output.contains("fixture-secret-must-never-appear"),
        "{output}"
    );
    assert!(output.contains("[REDACTED]"), "{output}");
}

#[test]
fn protocol_debug_redacts_every_secret_transport() {
    let mutation = CatalogMutation::Update(ConnectionProfile::new("server", "server.test"));
    let apply = UiCommand::ApplyCatalog {
        mutation,
        secret: SecretUpdate::Set(secret()),
    };
    let paste = SessionUiCommand::Paste(secret());
    let password = InteractionResponse::Secret(secret());
    let passphrase = InteractionResponse::Secret(secret());
    let answers = InteractionResponse::Answers(vec![secret(), secret()]);
    let respond = UiCommand::Respond {
        session: SessionId::new(),
        interaction: InteractionId::new(),
        response: InteractionResponse::Secret(secret()),
    };
    let session_respond = SessionUiCommand::Respond {
        interaction: InteractionId::new(),
        response: InteractionResponse::Answers(vec![secret()]),
    };
    let nested_paste = UiCommand::Session {
        session: SessionId::new(),
        command: SessionUiCommand::Paste(secret()),
    };
    let copied_text = SessionUiEvent::Copy("fixture-secret-must-never-appear".into());

    assert!(matches!(apply.secret_update(), Some(SecretUpdate::Set(_))));
    assert_redacted(apply);
    assert_redacted(paste);
    assert_redacted(password);
    assert_redacted(passphrase);
    assert_redacted(answers);
    assert_redacted(respond);
    assert_redacted(session_respond);
    assert_redacted(nested_paste);
    assert_redacted(copied_text);
}

#[test]
fn interaction_requests_and_non_secret_render_values_are_constructible_and_serializable() {
    let interaction = InteractionId::new();
    let prompt = KeyboardInteractivePrompt {
        id: interaction,
        name: "MFA".into(),
        instruction: "Enter verification code".into(),
        prompts: vec![AuthPrompt {
            id: interaction,
            label: "Verification code".into(),
            echo: false,
        }],
    };
    let size = TerminalSize {
        cols: 120,
        rows: 36,
        pixel_width: 1_920,
        pixel_height: 1_080,
        dpi: 144,
    };
    let frame = RenderFrame {
        generation: 4,
        size,
        viewport_top: 10,
        rows: Arc::from([RenderRow {
            stable_row: 10,
            wrapped: false,
            cells: Arc::from([RenderCell {
                text: "宽".into(),
                width: 2,
                foreground: Color::Ansi(3),
                background: Color::Default,
                attributes: CellAttributes::default(),
                selected: false,
            }]),
        }]),
        cursor: None,
        title: "safe".into(),
        display_modes: TerminalDisplayModes {
            alternate_screen: true,
            mouse_reporting: true,
            ..TerminalDisplayModes::default()
        },
        alternate_screen: true,
        mouse_reporting: true,
    };
    let event = AppEvent::Session {
        session: SessionId::new(),
        event: rshell_core::protocol::SessionUiEvent::Frame(Arc::new(frame.clone())),
    };

    assert_eq!(frame.rows[0].cells[0].width, 2);
    assert!(!frame.rows[0].cells[0].selected);
    assert!(frame.alternate_screen);
    assert!(frame.mouse_reporting);
    assert!(frame.display_modes.has_residue());
    assert_eq!(frame.alternate_screen, frame.display_modes.alternate_screen);
    assert_eq!(frame.mouse_reporting, frame.display_modes.mouse_reporting);

    let mut legacy_value = serde_json::to_value(&frame).unwrap();
    let legacy_object = legacy_value.as_object_mut().unwrap();
    legacy_object.remove("alternate_screen");
    legacy_object.remove("mouse_reporting");
    legacy_object.remove("display_modes");
    legacy_object["rows"][0]["cells"][0]
        .as_object_mut()
        .unwrap()
        .remove("selected");
    let legacy_frame: RenderFrame = serde_json::from_value(legacy_value).unwrap();
    assert!(!legacy_frame.alternate_screen);
    assert!(!legacy_frame.mouse_reporting);
    assert_eq!(legacy_frame.display_modes, TerminalDisplayModes::default());
    assert!(!legacy_frame.rows[0].cells[0].selected);
    assert_eq!(
        serde_json::from_value::<RenderFrame>(serde_json::to_value(&frame).unwrap()).unwrap(),
        frame
    );
    assert!(format!("{prompt:?}").contains("Verification code"));
    assert!(format!("{event:?}").contains("RenderFrame"));
    assert_eq!(
        Viewport {
            top_stable_row: 10,
            rows: 36
        }
        .rows,
        36
    );
    assert_eq!(HostKeyDecision::Reject, HostKeyDecision::Reject);
    assert_ne!(PaneId::new(), PaneId::new());
}

#[test]
fn interaction_acknowledgement_debug_preserves_exact_non_secret_identity() {
    let session = SessionId::new();
    let interaction = InteractionId::new();
    let event = AppEvent::InteractionResponded {
        session,
        interaction,
    };

    assert!(matches!(
        event,
        AppEvent::InteractionResponded {
            session: event_session,
            interaction: event_interaction,
        } if event_session == session && event_interaction == interaction
    ));
    let debug = format!("{event:?}");
    assert!(debug.contains(&session.0.to_string()));
    assert!(debug.contains(&interaction.0.to_string()));
}
