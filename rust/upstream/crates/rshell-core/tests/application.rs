mod support;

use std::{path::PathBuf, sync::Arc, time::Duration};

use rshell_core::{
    AppError, AppEvent, AppFailureCategory, ApplicationService, AuthenticationKind,
    CatalogMutation, ConnectionProfile, CredentialOperationError, CredentialRef,
    DisplayRecoveryNotice, ExitStatus, ImportSourceKind, InteractionId, InteractionRequest,
    InteractionResponse, RecoveryAction, RenderFrame, RenderRow, RepositoryError, SessionFailure,
    SessionState, SessionUiCommand, SessionUiEvent, SplitAxis, TerminalDisplayModes,
    TerminalProfile, TerminalSize, TransportKind, UI_COMMAND_CAPACITY, UiCommand, UiPortError,
    VaultFailure,
};
use secrecy::SecretString;
use tokio::time::timeout;
use uuid::Uuid;

use support::{RecordingPorts, bootstrap_state};

async fn recv_matching(
    events: &async_channel::Receiver<AppEvent>,
    predicate: impl Fn(&AppEvent) -> bool,
) -> AppEvent {
    timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("application event timed out")
}

#[tokio::test]
async fn initialized_application_opens_local_session_without_repeating_bootstrap() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();

    assert_eq!(ports.calls(), ["session.launch_local"]);
    assert_eq!(app.initial_view_model().workspace.tabs.len(), 1);
    assert_eq!(
        app.initial_view_model().workspace.tabs[0]
            .pane_tree
            .session_ids()
            .len(),
        1
    );
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn new_local_tab_launches_before_committing_workspace_and_emits_snapshot() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    ports.clear_calls();
    let events = app.event_receiver();

    app.ui_port().try_send(UiCommand::NewLocalTab).unwrap();
    let event = recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    let AppEvent::WorkspaceChanged(workspace) = event else {
        unreachable!()
    };

    assert_eq!(ports.calls(), ["session.launch_local"]);
    assert_eq!(workspace.tabs.len(), 2);
    assert_eq!(workspace.active_tab, Some(workspace.tabs[1].id));
    assert_eq!(workspace.tabs[1].pane_tree.session_ids().len(), 1);
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_new_local_tab_keeps_prior_view_model_and_is_retryable() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let before = app.view_model();
    let events = app.event_receiver();
    ports.fail_launch(true);

    app.ui_port().try_send(UiCommand::NewLocalTab).unwrap();
    let failure = recv_matching(&events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;
    let AppEvent::OperationFailed(failure) = failure else {
        unreachable!()
    };

    assert_eq!(app.view_model(), before);
    assert!(failure.retryable);
    assert_eq!(failure.category, AppFailureCategory::Pty);
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn catalog_mutation_uses_only_credential_coordinator_port() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    ports.clear_calls();
    let events = app.event_receiver();
    let mut profile = ConnectionProfile::new("server", "example.test");
    profile.username = "alice".into();

    app.ui_port()
        .try_send(UiCommand::ApplyCatalog {
            mutation: CatalogMutation::Create(profile),
            secret: rshell_core::SecretUpdate::Unchanged,
        })
        .unwrap();
    recv_matching(&events, |event| {
        matches!(event, AppEvent::CatalogChanged(_))
    })
    .await;

    assert_eq!(ports.calls(), ["credentials.apply_catalog"]);
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_catalog_mutation_preserves_view_model_and_is_retryable() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    ports.fail_credentials(true);
    let before = app.view_model();
    let events = app.event_receiver();

    app.ui_port()
        .try_send(UiCommand::ApplyCatalog {
            mutation: CatalogMutation::Create(ConnectionProfile::new("server", "example.test")),
            secret: rshell_core::SecretUpdate::Unchanged,
        })
        .unwrap();
    let failure = recv_matching(&events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;

    assert_eq!(app.view_model(), before);
    assert!(matches!(failure, AppEvent::OperationFailed(failure) if failure.retryable));
    assert_eq!(
        ports.calls(),
        ["session.launch_local", "credentials.apply_catalog"]
    );
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn connect_reads_credential_once_moves_it_and_emits_only_redacted_values() {
    let mut bootstrap = bootstrap_state();
    let mut profile = ConnectionProfile::new("managed", "example.test");
    profile.username = "alice".into();
    profile.transport = rshell_core::TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;
    profile.credential_ref = Some(CredentialRef::new("credential-key"));
    let connection = profile.id;
    bootstrap.catalog.connections.insert(connection, profile);
    let ports = RecordingPorts::new(&bootstrap);
    ports.expect_secret("application-secret");
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let pane = app.initial_view_model().workspace.tabs[0].active_pane;
    let events = app.event_receiver();

    app.ui_port()
        .try_send(UiCommand::Connect { pane, connection })
        .unwrap();
    let event = recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;

    assert_eq!(ports.credential_reads(), 1);
    assert!(ports.secret_received());
    assert!(!format!("{event:?}").contains("application-secret"));
    assert!(!format!("{:?}", app.view_model()).contains("application-secret"));
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn retry_pane_reads_fresh_native_credential() {
    let mut bootstrap = bootstrap_state();
    let mut profile = ConnectionProfile::new("managed", "example.test");
    profile.username = "alice".into();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;
    profile.credential_ref = Some(CredentialRef::new("credential-key"));
    let connection = profile.id;
    bootstrap.catalog.connections.insert(connection, profile);
    let ports = RecordingPorts::new(&bootstrap);
    let first = SecretString::from(Uuid::new_v4().to_string());
    let second = SecretString::from(Uuid::new_v4().to_string());
    ports.set_secret(first);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let pane = app.initial_view_model().workspace.tabs[0].active_pane;
    let events = app.event_receiver();

    app.ui_port()
        .try_send(UiCommand::Connect { pane, connection })
        .unwrap();
    let first_workspace = recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    let AppEvent::WorkspaceChanged(first_workspace) = first_workspace else {
        unreachable!();
    };
    let first_session = first_workspace.tabs[0]
        .pane_tree
        .session_id(pane)
        .unwrap()
        .unwrap();

    ports.set_secret(second);
    app.ui_port().try_send(UiCommand::RetryPane(pane)).unwrap();
    let second_workspace = recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    let AppEvent::WorkspaceChanged(second_workspace) = second_workspace else {
        unreachable!();
    };
    let second_session = second_workspace.tabs[0]
        .pane_tree
        .session_id(pane)
        .unwrap()
        .unwrap();

    assert_ne!(first_session, second_session);
    assert_eq!(ports.credential_reads(), 2);
    assert!(ports.second_launch_received_replacement_secret());
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_public_key_prefetches_only_an_existing_passphrase_reference() {
    for has_reference in [false, true] {
        let mut bootstrap = bootstrap_state();
        let mut profile = ConnectionProfile::new("public-key", "example.test");
        profile.username = "alice".into();
        profile.transport = TransportKind::NativeSsh;
        profile.authentication = AuthenticationKind::PublicKey;
        profile.identity_file = Some(PathBuf::from("identity"));
        if has_reference {
            profile.credential_ref = Some(CredentialRef::new("passphrase-key"));
        }
        let connection = profile.id;
        bootstrap.catalog.connections.insert(connection, profile);
        let ports = RecordingPorts::new(&bootstrap);
        if has_reference {
            ports.expect_secret("key-passphrase");
        }
        let app = ApplicationService::start(ports.dependencies(), bootstrap)
            .await
            .unwrap();
        let pane = app.initial_view_model().workspace.tabs[0].active_pane;
        let events = app.event_receiver();

        app.ui_port()
            .try_send(UiCommand::Connect { pane, connection })
            .unwrap();
        recv_matching(&events, |event| {
            matches!(event, AppEvent::WorkspaceChanged(_))
        })
        .await;
        assert_eq!(ports.credential_reads(), usize::from(has_reference));
        assert_eq!(ports.ssh_secret_present(), Some(has_reference));
        app.shutdown().await.unwrap();
    }
}

async fn connect_without_prefetch(transport: TransportKind, authentication: AuthenticationKind) {
    let mut bootstrap = bootstrap_state();
    let mut profile = ConnectionProfile::new("no-prefetch", "example.test");
    profile.username = "alice".into();
    profile.transport = transport;
    profile.authentication = authentication;
    profile.credential_ref = Some(CredentialRef::new("stale-credential-key"));
    let connection = profile.id;
    bootstrap.catalog.connections.insert(connection, profile);
    let ports = RecordingPorts::new(&bootstrap);
    ports.expect_secret("must-not-be-read");
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let pane = app.initial_view_model().workspace.tabs[0].active_pane;
    let events = app.event_receiver();

    app.ui_port()
        .try_send(UiCommand::Connect { pane, connection })
        .unwrap();
    recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;

    assert_eq!(ports.credential_reads(), 0);
    assert_eq!(ports.ssh_secret_present(), Some(false));
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn system_openssh_never_prefetches_managed_credentials() {
    connect_without_prefetch(TransportKind::SystemOpenSsh, AuthenticationKind::Password).await;
}

#[tokio::test]
async fn native_agent_and_keyboard_interactive_never_prefetch_credentials() {
    connect_without_prefetch(TransportKind::NativeSsh, AuthenticationKind::Agent).await;
    connect_without_prefetch(
        TransportKind::NativeSsh,
        AuthenticationKind::KeyboardInteractive,
    )
    .await;
}

#[tokio::test]
async fn credential_lookup_preserves_vault_and_storage_failure_categories() {
    let mut bootstrap = bootstrap_state();
    let mut profile = ConnectionProfile::new("managed", "example.test");
    profile.username = "alice".into();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;
    profile.credential_ref = Some(CredentialRef::new("credential-key"));
    let connection = profile.id;
    bootstrap.catalog.connections.insert(connection, profile);
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let pane = app.initial_view_model().workspace.tabs[0].active_pane;
    let events = app.event_receiver();

    for (error, category) in [
        (
            CredentialOperationError::Vault(VaultFailure::Denied),
            AppFailureCategory::Vault,
        ),
        (
            CredentialOperationError::Repository(RepositoryError::Unavailable),
            AppFailureCategory::Storage,
        ),
        (
            CredentialOperationError::ReconciliationRequired,
            AppFailureCategory::Storage,
        ),
    ] {
        ports.credential_get_error(error);
        app.ui_port()
            .try_send(UiCommand::Connect { pane, connection })
            .unwrap();
        let event = recv_matching(&events, |event| {
            matches!(event, AppEvent::OperationFailed(_))
        })
        .await;
        assert!(matches!(
            event,
            AppEvent::OperationFailed(failure)
                if failure.category == category
                    && failure.action == RecoveryAction::EditConnection(connection)
        ));
    }
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_binding_forwards_events_latest_frame_and_interaction_to_same_actor() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let session = ports.latest_session();
    let interaction = InteractionId::new();
    let request = InteractionRequest::Password(rshell_core::AuthPrompt {
        id: interaction,
        label: "Password".into(),
        echo: false,
    });
    ports.send_session_event(session, SessionUiEvent::State(SessionState::Connected));
    ports.send_session_event(
        session,
        SessionUiEvent::InteractionRequired(request.clone()),
    );
    let frame = Arc::new(RenderFrame {
        generation: 7,
        size: TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        },
        viewport_top: 0,
        rows: Vec::<RenderRow>::new().into(),
        cursor: None,
        title: "test".into(),
        display_modes: Default::default(),
        alternate_screen: false,
        mouse_reporting: false,
    });
    ports.send_frame(session, Arc::clone(&frame));

    recv_matching(&events, |event| matches!(event, AppEvent::Session { session: id, event: SessionUiEvent::State(SessionState::Connected) } if *id == session)).await;
    recv_matching(&events, |event| matches!(event, AppEvent::InteractionRequired { session: id, .. } if *id == session)).await;
    let frame_event = recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                event: SessionUiEvent::Frame(_),
                ..
            }
        )
    })
    .await;
    assert!(
        matches!(frame_event, AppEvent::Session { session: id, event: SessionUiEvent::Frame(frame) } if id == session && frame.generation == 7)
    );

    let mut presentation_only = (*frame).clone();
    presentation_only.title = "selection-only-presentation-change".into();
    ports.send_frame(session, Arc::new(presentation_only));
    let presentation_event = recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                event: SessionUiEvent::Frame(frame),
                ..
            } if frame.title == "selection-only-presentation-change"
        )
    })
    .await;
    assert!(matches!(
        presentation_event,
        AppEvent::Session {
            session: id,
            event: SessionUiEvent::Frame(frame),
        } if id == session && frame.generation == 7
    ));

    app.ui_port()
        .try_send(UiCommand::Respond {
            session,
            interaction,
            response: InteractionResponse::Cancel,
        })
        .unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            if ports
                .session_commands()
                .iter()
                .any(|(id, command)| *id == session && command.contains(&interaction.0.to_string()))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let acknowledged = recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::InteractionResponded {
                session: event_session,
                interaction: event_interaction,
            } if *event_session == session && *event_interaction == interaction
        )
    })
    .await;
    assert!(matches!(
        acknowledged,
        AppEvent::InteractionResponded {
            session: event_session,
            interaction: event_interaction,
        } if event_session == session && event_interaction == interaction
    ));
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn rejected_interaction_response_emits_failure_without_acknowledgement() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let session = ports.latest_session();
    let interaction = InteractionId::new();
    ports.fail_session_commands(true);

    app.ui_port()
        .try_send(UiCommand::Respond {
            session,
            interaction,
            response: InteractionResponse::Cancel,
        })
        .unwrap();

    let mut acknowledged = false;
    let failure = timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await.unwrap() {
                AppEvent::InteractionResponded {
                    session: event_session,
                    interaction: event_interaction,
                } if event_session == session && event_interaction == interaction => {
                    acknowledged = true;
                }
                AppEvent::OperationFailed(failure) => break failure,
                _ => {}
            }
        }
    })
    .await
    .expect("response failure event");

    assert_eq!(failure.category, AppFailureCategory::Backpressure);
    assert!(
        !acknowledged,
        "rejected response must never be acknowledged"
    );
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn stale_terminal_command_is_ignored_but_stale_interaction_response_is_reported() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let stale = rshell_core::SessionId::new();

    app.ui_port()
        .try_send(UiCommand::Session {
            session: stale,
            command: SessionUiCommand::Reconnect,
        })
        .unwrap();
    app.ui_port()
        .try_send(UiCommand::SearchConnections("stale-command-barrier".into()))
        .unwrap();

    loop {
        match timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("stale command barrier timed out")
            .unwrap()
        {
            AppEvent::OperationFailed(failure) => {
                panic!("stale terminal command emitted {failure:?}")
            }
            AppEvent::SearchResults(_) => break,
            _ => {}
        }
    }

    app.ui_port()
        .try_send(UiCommand::Respond {
            session: stale,
            interaction: InteractionId::new(),
            response: InteractionResponse::Cancel,
        })
        .unwrap();
    let failure = recv_matching(&events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;
    assert!(matches!(
        failure,
        AppEvent::OperationFailed(failure)
            if failure.context == "session is no longer available"
    ));

    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn preview_cancel_and_failed_commit_preserve_storage_ownership_and_view_model() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();

    app.ui_port()
        .try_send(UiCommand::PreviewImport {
            source: ImportSourceKind::LegacyRshellJson,
            path: PathBuf::from("legacy.json"),
        })
        .unwrap();
    let preview_event =
        recv_matching(&events, |event| matches!(event, AppEvent::ImportPreview(_))).await;
    let AppEvent::ImportPreview(preview) = preview_event else {
        unreachable!()
    };
    assert_eq!(ports.pending_preview_count(), 1);
    assert_eq!(app.view_model().pending_imports.len(), 1);

    ports.fail_import(true);
    let before = app.view_model();
    app.ui_port()
        .try_send(UiCommand::CommitImport {
            preview: preview.id,
            selected: Default::default(),
        })
        .unwrap();
    let failure = recv_matching(&events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;
    assert_eq!(app.view_model(), before);
    assert!(matches!(
        failure,
        AppEvent::OperationFailed(failure)
            if failure.retryable && failure.category == AppFailureCategory::Storage
    ));

    ports.import_error(Some(rshell_core::ImportError::Vault));
    app.ui_port()
        .try_send(UiCommand::CommitImport {
            preview: preview.id,
            selected: Default::default(),
        })
        .unwrap();
    let failure = recv_matching(&events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;
    assert!(matches!(
        failure,
        AppEvent::OperationFailed(failure)
            if failure.retryable && failure.category == AppFailureCategory::Vault
    ));

    ports.import_error(None);
    app.ui_port()
        .try_send(UiCommand::CancelImport {
            preview: preview.id,
        })
        .unwrap();
    recv_matching(
        &events,
        |event| matches!(event, AppEvent::ImportCancelled(id) if *id == preview.id),
    )
    .await;
    assert_eq!(ports.pending_preview_count(), 0);
    assert!(app.view_model().pending_imports.is_empty());
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn expired_import_preview_is_removed_from_core_view() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();

    for commit in [true, false] {
        app.ui_port()
            .try_send(UiCommand::PreviewImport {
                source: ImportSourceKind::OpenSshConfig,
                path: PathBuf::from("config"),
            })
            .unwrap();
        let preview =
            recv_matching(&events, |event| matches!(event, AppEvent::ImportPreview(_))).await;
        let AppEvent::ImportPreview(preview) = preview else {
            unreachable!()
        };
        let mut view = app.view_stream();
        ports.import_error(Some(rshell_core::ImportError::PreviewExpired));
        let command = if commit {
            UiCommand::CommitImport {
                preview: preview.id,
                selected: Default::default(),
            }
        } else {
            UiCommand::CancelImport {
                preview: preview.id,
            }
        };

        app.ui_port().try_send(command).unwrap();
        let failure = recv_matching(&events, |event| {
            matches!(event, AppEvent::OperationFailed(_))
        })
        .await;
        let updated = timeout(Duration::from_secs(2), view.changed())
            .await
            .expect("expired preview must publish a view")
            .expect("view stream must remain open");

        assert!(matches!(
            failure,
            AppEvent::OperationFailed(failure) if failure.context == "import preview expired"
        ));
        assert!(!updated.pending_imports.contains_key(&preview.id));
        ports.import_error(None);
    }

    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn successful_import_commit_refreshes_catalog_and_consumes_cached_preview() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    app.ui_port()
        .try_send(UiCommand::PreviewImport {
            source: ImportSourceKind::OpenSshConfig,
            path: PathBuf::from("config"),
        })
        .unwrap();
    let preview = recv_matching(&events, |event| matches!(event, AppEvent::ImportPreview(_))).await;
    let AppEvent::ImportPreview(preview) = preview else {
        unreachable!()
    };

    app.ui_port()
        .try_send(UiCommand::CommitImport {
            preview: preview.id,
            selected: Default::default(),
        })
        .unwrap();
    recv_matching(&events, |event| {
        matches!(event, AppEvent::CatalogChanged(_))
    })
    .await;
    recv_matching(&events, |event| {
        matches!(event, AppEvent::ImportCompleted(_))
    })
    .await;

    assert!(app.view_model().pending_imports.is_empty());
    assert_eq!(ports.pending_preview_count(), 0);
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_saves_keep_prior_view_model_and_return_retry_actions() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    ports.fail_repository(true);
    let before = app.view_model();
    let mut settings = before.settings.clone();
    settings.color_scheme = rshell_core::ColorScheme::Nord;

    app.ui_port()
        .try_send(UiCommand::SaveSettings(settings))
        .unwrap();
    let failure = recv_matching(&events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;

    assert_eq!(app.view_model(), before);
    assert!(
        matches!(failure, AppEvent::OperationFailed(failure) if failure.retryable && failure.action == RecoveryAction::Retry)
    );

    let terminal = TerminalProfile {
        name: "Unsaved".into(),
        ..TerminalProfile::default()
    };
    app.ui_port()
        .try_send(UiCommand::SaveTerminalProfile(terminal))
        .unwrap();
    let profile_failure = recv_matching(&events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;
    assert_eq!(app.view_model(), before);
    assert!(matches!(profile_failure, AppEvent::OperationFailed(failure) if failure.retryable));
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_terminal_settings_are_rejected_before_the_repository_boundary() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    ports.clear_calls();
    let mut invalid = TerminalProfile::default();
    invalid.settings.font_size = f32::NAN;

    app.ui_port()
        .try_send(UiCommand::SaveTerminalProfile(invalid))
        .unwrap();
    let failure = recv_matching(&events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;

    assert!(matches!(
        failure,
        AppEvent::OperationFailed(failure)
            if failure.category == AppFailureCategory::Validation && !failure.retryable
    ));
    assert!(
        !ports
            .calls()
            .iter()
            .any(|call| call == "repository.save_terminal_profile")
    );
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn split_start_local_reconnect_close_and_close_tab_have_explicit_routing() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let first_pane = app.initial_view_model().workspace.tabs[0].active_pane;

    app.ui_port()
        .try_send(UiCommand::Split {
            pane: first_pane,
            axis: SplitAxis::Horizontal,
        })
        .unwrap();
    let split = recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    let AppEvent::WorkspaceChanged(workspace) = split else {
        unreachable!()
    };
    assert_eq!(workspace.tabs[0].pane_tree.pane_ids().len(), 2);
    let second_pane = workspace.tabs[0].active_pane;
    let second_session = workspace.tabs[0]
        .pane_tree
        .session_id(second_pane)
        .unwrap()
        .unwrap();

    app.ui_port()
        .try_send(UiCommand::StartLocal { pane: first_pane })
        .unwrap();
    recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;

    app.ui_port()
        .try_send(UiCommand::Session {
            session: second_session,
            command: SessionUiCommand::Reconnect,
        })
        .unwrap();
    app.ui_port()
        .try_send(UiCommand::ClosePane(second_pane))
        .unwrap();
    recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    let tab_id = app.view_model().workspace.tabs[0].id;
    app.ui_port().try_send(UiCommand::CloseTab(tab_id)).unwrap();
    let closed = recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    assert!(matches!(closed, AppEvent::WorkspaceChanged(workspace) if workspace.tabs.is_empty()));

    let commands = ports.session_commands();
    assert!(
        commands
            .iter()
            .any(|(id, command)| *id == second_session && command == "Reconnect")
    );
    assert!(
        ports
            .calls()
            .iter()
            .filter(|call| call.as_str() == "session.shutdown")
            .count()
            >= 2
    );
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn replaced_and_closed_sessions_drop_their_binding_forwarders() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let tab = &app.initial_view_model().workspace.tabs[0];
    let pane = tab.active_pane;
    let original = tab.pane_tree.session_id(pane).unwrap().unwrap();
    let frame = Arc::new(RenderFrame {
        generation: 1,
        size: TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        },
        viewport_top: 0,
        rows: Vec::<RenderRow>::new().into(),
        cursor: None,
        title: "old".into(),
        display_modes: Default::default(),
        alternate_screen: false,
        mouse_reporting: false,
    });
    ports.send_frame(original, frame);
    recv_matching(&events, |event| {
        matches!(event, AppEvent::Session { session, event: SessionUiEvent::Frame(_) } if *session == original)
    })
    .await;

    app.ui_port()
        .try_send(UiCommand::StartLocal { pane })
        .unwrap();
    recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    timeout(
        Duration::from_secs(2),
        ports.wait_for_binding_closed(original),
    )
    .await
    .expect("replaced session bridge remained open");
    assert!(!app.view_model().latest_frames.contains_key(&original));

    let view = app.view_model();
    let replacement = view.workspace.tabs[0]
        .pane_tree
        .session_id(pane)
        .unwrap()
        .unwrap();
    let tab_id = view.workspace.tabs[0].id;
    app.ui_port().try_send(UiCommand::CloseTab(tab_id)).unwrap();
    recv_matching(
        &events,
        |event| matches!(event, AppEvent::WorkspaceChanged(workspace) if workspace.tabs.is_empty()),
    )
    .await;
    timeout(
        Duration::from_secs(2),
        ports.wait_for_binding_closed(replacement),
    )
    .await
    .expect("closed session bridge remained open");
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn recovery_notice_is_bound_to_current_session() {
    fn frame(generation: u64) -> Arc<RenderFrame> {
        Arc::new(RenderFrame {
            generation,
            size: TerminalSize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 96,
            },
            viewport_top: 0,
            rows: Vec::<RenderRow>::new().into(),
            cursor: None,
            title: format!("frame-{generation}"),
            display_modes: Default::default(),
            alternate_screen: false,
            mouse_reporting: false,
        })
    }

    let notice = DisplayRecoveryNotice {
        interrupted_generation: 6,
        observed_generation: 7,
        modes: TerminalDisplayModes {
            alternate_screen: true,
            ..Default::default()
        },
    };
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let tab = &app.initial_view_model().workspace.tabs[0];
    let pane = tab.active_pane;
    let original = tab.pane_tree.session_id(pane).unwrap().unwrap();

    ports.send_frame(original, frame(7));
    recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                session,
                event: SessionUiEvent::Frame(frame),
            } if *session == original && frame.generation == 7
        )
    })
    .await;
    let mut view = app.view_stream();
    ports.send_frame(original, frame(6));
    match timeout(Duration::from_secs(2), view.changed()).await {
        Ok(Some(updated)) => assert_eq!(updated.latest_frames[&original].generation, 7),
        Ok(None) => panic!("view stream closed while filtering stale frames"),
        Err(_) => assert_eq!(app.view_model().latest_frames[&original].generation, 7),
    }

    ports.send_session_event(original, SessionUiEvent::RecoveryChanged(Some(notice)));
    let added = recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                session,
                event: SessionUiEvent::RecoveryChanged(Some(current)),
            } if *session == original && *current == notice
        )
    })
    .await;
    assert!(matches!(
        added,
        AppEvent::Session {
            session,
            event: SessionUiEvent::RecoveryChanged(Some(current)),
        } if session == original && current == notice
    ));
    assert_eq!(
        app.view_model().display_recovery.get(&original),
        Some(&notice)
    );

    ports.send_session_event(original, SessionUiEvent::RecoveryChanged(None));
    recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                session,
                event: SessionUiEvent::RecoveryChanged(None),
            } if *session == original
        )
    })
    .await;
    assert!(!app.view_model().display_recovery.contains_key(&original));

    ports.send_session_event(original, SessionUiEvent::RecoveryChanged(Some(notice)));
    recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                session,
                event: SessionUiEvent::RecoveryChanged(Some(current)),
            } if *session == original && *current == notice
        )
    })
    .await;
    let stale_notice = DisplayRecoveryNotice {
        observed_generation: 8,
        ..notice
    };
    ports.send_session_event(
        original,
        SessionUiEvent::RecoveryChanged(Some(stale_notice)),
    );
    app.ui_port().try_send(UiCommand::RetryPane(pane)).unwrap();
    let replacement_workspace = recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    let AppEvent::WorkspaceChanged(replacement_workspace) = replacement_workspace else {
        unreachable!()
    };
    let replacement = replacement_workspace.tabs[0]
        .pane_tree
        .session_id(pane)
        .unwrap()
        .unwrap();
    timeout(
        Duration::from_secs(2),
        ports.wait_for_binding_closed(original),
    )
    .await
    .expect("replaced session bridge remained open");
    let replacement_view = app.view_model();
    assert_ne!(replacement, original);
    assert!(!replacement_view.display_recovery.contains_key(&original));
    assert!(!replacement_view.display_recovery.contains_key(&replacement));
    assert!(!replacement_view.latest_frames.contains_key(&original));

    ports.send_session_event(replacement, SessionUiEvent::RecoveryChanged(Some(notice)));
    recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                session,
                event: SessionUiEvent::RecoveryChanged(Some(current)),
            } if *session == replacement && *current == notice
        )
    })
    .await;
    assert_eq!(
        app.view_model().display_recovery.get(&replacement),
        Some(&notice)
    );
    app.ui_port()
        .try_send(UiCommand::CloseTab(replacement_workspace.tabs[0].id))
        .unwrap();
    recv_matching(
        &events,
        |event| matches!(event, AppEvent::WorkspaceChanged(workspace) if workspace.tabs.is_empty()),
    )
    .await;
    assert!(app.view_model().display_recovery.is_empty());
    app.shutdown().await.unwrap();

    for (completion, expected_state, expected_error) in [
        (
            SessionUiEvent::Exited(ExitStatus {
                code: Some(0),
                success: true,
            }),
            SessionState::Exited,
            None,
        ),
        (
            SessionUiEvent::Failed(SessionFailure::Network),
            SessionState::Failed,
            Some(SessionFailure::Network),
        ),
        (
            SessionUiEvent::Crashed("diagnostic-secret".into()),
            SessionState::Crashed,
            Some(SessionFailure::Crashed),
        ),
    ] {
        let bootstrap = bootstrap_state();
        let ports = RecordingPorts::new(&bootstrap);
        let app = ApplicationService::start(ports.dependencies(), bootstrap)
            .await
            .unwrap();
        let events = app.event_receiver();
        let session = ports.latest_session();

        ports.send_frame(session, frame(7));
        recv_matching(&events, |event| {
            matches!(
                event,
                AppEvent::Session {
                    session: event_session,
                    event: SessionUiEvent::Frame(frame),
                } if *event_session == session && frame.generation == 7
            )
        })
        .await;
        ports.send_session_event(session, SessionUiEvent::RecoveryChanged(Some(notice)));
        recv_matching(&events, |event| {
            matches!(
                event,
                AppEvent::Session {
                    session: event_session,
                    event: SessionUiEvent::RecoveryChanged(Some(current)),
                } if *event_session == session && *current == notice
            )
        })
        .await;
        ports.send_session_event(session, completion);
        recv_matching(&events, |event| {
            matches!(
                event,
                AppEvent::Session {
                    session: event_session,
                    event: SessionUiEvent::Exited(_)
                        | SessionUiEvent::Failed(_)
                        | SessionUiEvent::Crashed(_),
                } if *event_session == session
            )
        })
        .await;
        timeout(
            Duration::from_secs(2),
            ports.wait_for_binding_closed(session),
        )
        .await
        .expect("completed session bridge remained open");

        let view = app.view_model();
        assert!(!view.display_recovery.contains_key(&session));
        assert!(!view.latest_frames.contains_key(&session));
        assert_eq!(view.session_states.get(&session), Some(&expected_state));
        assert_eq!(
            view.error_panes.get(&session).map(|pane| pane.failure),
            expected_error
        );
        app.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn shutdown_publishes_an_empty_workspace_before_shutdown_complete() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let initial = &app.initial_view_model().workspace.tabs[0];
    let initial_session = initial
        .pane_tree
        .session_id(initial.active_pane)
        .unwrap()
        .unwrap();

    app.ui_port().try_send(UiCommand::NewLocalTab).unwrap();
    let AppEvent::WorkspaceChanged(workspace) = recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await
    else {
        unreachable!()
    };
    let second_tab = &workspace.tabs[1];
    let second_session = second_tab
        .pane_tree
        .session_id(second_tab.active_pane)
        .unwrap()
        .unwrap();

    ports.send_frame(
        initial_session,
        Arc::new(RenderFrame {
            generation: 1,
            size: TerminalSize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 96,
            },
            viewport_top: 0,
            rows: Vec::<RenderRow>::new().into(),
            cursor: None,
            title: "shutdown-frame".into(),
            display_modes: Default::default(),
            alternate_screen: false,
            mouse_reporting: false,
        }),
    );
    recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                session,
                event: SessionUiEvent::Frame(_),
            } if *session == initial_session
        )
    })
    .await;
    ports.send_session_event(
        initial_session,
        SessionUiEvent::RecoveryChanged(Some(DisplayRecoveryNotice {
            interrupted_generation: 0,
            observed_generation: 1,
            modes: TerminalDisplayModes {
                alternate_screen: true,
                ..Default::default()
            },
        })),
    );
    recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                session,
                event: SessionUiEvent::RecoveryChanged(Some(_)),
            } if *session == initial_session
        )
    })
    .await;
    ports.send_session_event(
        second_session,
        SessionUiEvent::Failed(SessionFailure::Network),
    );
    recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                session,
                event: SessionUiEvent::Failed(SessionFailure::Network),
            } if *session == second_session
        )
    })
    .await;
    let before_shutdown = app.view_model();
    assert_eq!(before_shutdown.workspace.tabs.len(), 2);
    assert!(!before_shutdown.pane_launches.is_empty());
    assert!(!before_shutdown.session_states.is_empty());
    assert!(!before_shutdown.latest_frames.is_empty());
    assert!(!before_shutdown.display_recovery.is_empty());
    assert!(!before_shutdown.error_panes.is_empty());

    app.ui_port().try_send(UiCommand::Shutdown).unwrap();
    assert_eq!(
        recv_matching(&events, |event| matches!(event, AppEvent::ShutdownComplete)).await,
        AppEvent::ShutdownComplete
    );

    let shutdown_view = app.view_model();
    assert!(shutdown_view.revision > before_shutdown.revision);
    assert!(shutdown_view.workspace.tabs.is_empty());
    assert_eq!(shutdown_view.workspace.active_tab, None);
    assert!(shutdown_view.pane_launches.is_empty());
    assert!(shutdown_view.session_states.is_empty());
    assert!(shutdown_view.latest_frames.is_empty());
    assert!(shutdown_view.display_recovery.is_empty());
    assert!(shutdown_view.error_panes.is_empty());
    assert_eq!(ports.shutdowns(), 1);
    timeout(Duration::from_secs(2), app.shutdown())
        .await
        .expect("shutdown must remain bounded")
        .unwrap();
    assert_eq!(ports.shutdowns(), 1);
}

#[tokio::test]
async fn shutdown_closes_intake_cancels_previews_and_stops_sessions() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    app.ui_port()
        .try_send(UiCommand::PreviewImport {
            source: ImportSourceKind::OpenSshConfig,
            path: PathBuf::from("config"),
        })
        .unwrap();
    let preview = recv_matching(&events, |event| matches!(event, AppEvent::ImportPreview(_))).await;
    let AppEvent::ImportPreview(preview) = preview else {
        unreachable!()
    };

    app.shutdown().await.unwrap();

    assert_eq!(events.recv().await.unwrap(), AppEvent::ShutdownComplete);
    assert_eq!(ports.cancelled_previews(), [preview.id]);
    assert_eq!(ports.shutdowns(), 1);
    assert_eq!(
        app.ui_port().try_send(UiCommand::NewLocalTab),
        Err(UiPortError::Closed)
    );
    assert_eq!(
        app.ui_port().try_send(UiCommand::Shutdown),
        Err(UiPortError::Closed)
    );
}

#[test]
fn ui_queue_contract_is_bounded_to_256() {
    assert_eq!(UI_COMMAND_CAPACITY, 256);
}

#[tokio::test]
async fn full_ui_queue_returns_visible_busy_error() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    ports.block_catalog_apply();
    app.ui_port()
        .try_send(UiCommand::ApplyCatalog {
            mutation: CatalogMutation::Create(ConnectionProfile::new("blocking", "example.test")),
            secret: rshell_core::SecretUpdate::Unchanged,
        })
        .unwrap();
    ports.wait_for_catalog_apply().await;

    let commands = app.ui_port();
    for _ in 0..UI_COMMAND_CAPACITY {
        commands
            .try_send(UiCommand::SearchConnections(String::new()))
            .unwrap();
    }
    assert_eq!(
        commands.try_send(UiCommand::SearchConnections(String::new())),
        Err(UiPortError::Busy)
    );

    let events = app.event_receiver();
    let drain = tokio::spawn(async move { while events.recv().await.is_ok() {} });
    ports.release_catalog_apply();
    app.shutdown().await.unwrap();
    drop(app);
    drain.await.unwrap();
}

#[tokio::test]
async fn shutdown_completes_when_public_event_and_command_queues_are_saturated() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    ports.block_catalog_apply();
    app.ui_port()
        .try_send(UiCommand::ApplyCatalog {
            mutation: CatalogMutation::Create(ConnectionProfile::new("blocking", "example.test")),
            secret: rshell_core::SecretUpdate::Unchanged,
        })
        .unwrap();
    ports.wait_for_catalog_apply().await;
    for _ in 0..UI_COMMAND_CAPACITY {
        app.ui_port()
            .try_send(UiCommand::SearchConnections(String::new()))
            .unwrap();
    }
    ports.release_catalog_apply();
    timeout(Duration::from_secs(2), async {
        while events.len() < 256 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("public event queue did not saturate");

    timeout(Duration::from_secs(2), app.shutdown())
        .await
        .expect("shutdown remained blocked by event backpressure")
        .unwrap();
    assert_eq!(ports.shutdowns(), 1);
}

#[tokio::test]
async fn shutdown_command_bypasses_a_full_ui_queue() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    ports.block_catalog_apply();
    app.ui_port()
        .try_send(UiCommand::ApplyCatalog {
            mutation: CatalogMutation::Create(ConnectionProfile::new("blocking", "example.test")),
            secret: rshell_core::SecretUpdate::Unchanged,
        })
        .unwrap();
    ports.wait_for_catalog_apply().await;
    for _ in 0..UI_COMMAND_CAPACITY {
        app.ui_port()
            .try_send(UiCommand::SearchConnections(String::new()))
            .unwrap();
    }

    assert_eq!(app.ui_port().try_send(UiCommand::Shutdown), Ok(()));
    ports.release_catalog_apply();
    timeout(Duration::from_secs(2), app.shutdown())
        .await
        .expect("out-of-band shutdown did not complete")
        .unwrap();
    assert_eq!(ports.shutdowns(), 1);
}

#[tokio::test]
async fn shutdown_returns_session_failure_after_closing_all_application_intake() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    ports.fail_shutdown_all(SessionFailure::Crashed);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let session = ports.latest_session();

    let result = timeout(Duration::from_secs(2), app.shutdown())
        .await
        .expect("shutdown must remain bounded");

    assert_eq!(
        result,
        Err(AppError::SessionShutdown(SessionFailure::Crashed))
    );
    assert_eq!(ports.shutdowns(), 1);
    assert_eq!(
        app.ui_port().try_send(UiCommand::NewLocalTab),
        Err(UiPortError::Closed)
    );
    timeout(
        Duration::from_secs(2),
        ports.wait_for_binding_closed(session),
    )
    .await
    .expect("shutdown must abort session forwarders");
    assert_eq!(
        app.shutdown().await,
        Err(AppError::SessionShutdown(SessionFailure::Crashed))
    );
    assert_eq!(ports.shutdowns(), 1);
}

#[tokio::test]
async fn fatal_session_diagnostic_is_retained_and_redacted() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let session = ports.latest_session();
    ports.send_session_event(session, SessionUiEvent::Crashed("diagnostic-secret".into()));
    let event = recv_matching(&events, |event| {
        matches!(
            event,
            AppEvent::Session {
                event: SessionUiEvent::Crashed(_),
                ..
            }
        )
    })
    .await;

    assert!(!format!("{event:?}").contains("diagnostic-secret"));
    let pane = app.view_model().error_panes[&session].clone();
    assert_eq!(pane.failure, rshell_core::SessionFailure::Crashed);
    assert_eq!(pane.diagnostic, "session actor crashed");
    timeout(
        Duration::from_secs(2),
        ports.wait_for_binding_closed(session),
    )
    .await
    .expect("completed session bridge remained open");
    app.shutdown().await.unwrap();
}

#[test]
fn error_and_command_debug_are_redacted() {
    let failure = rshell_core::AppFailure::retryable(
        AppFailureCategory::Authentication,
        "authentication failed",
        RecoveryAction::Retry,
    );
    assert!(!format!("{failure:?}").contains("secret"));
    let command = SessionUiCommand::Paste(secrecy::SecretString::from("debug-secret"));
    assert_eq!(format!("{command:?}"), "Paste([REDACTED])");
}
