mod support;

use std::{sync::Arc, time::Duration};

use rshell_core::{
    AuthPrompt, CellPosition, ExitStatus, HostKeyDecision, HostKeyPrompt, InteractionId,
    InteractionRequest, InteractionResponse, KeyModifiers, MouseButton, MouseEventKind,
    SelectionRange, SessionFailure, SessionState, TerminalDisplayModes, TerminalInput,
    TerminalMouseEvent, TerminalOverrides, TerminalSettingsV1,
};
use rshell_session::{
    DefaultTerminalEngine, PresentationPolicy, SessionCommand, SessionError, SessionEvent,
    SessionLaunch, SessionManager, TransportError, TransportEvent, TransportRequest,
};

#[test]
fn copy_ready_debug_redacts_clipboard_text() {
    let event = SessionEvent::CopyReady("COPY-READY-SENSITIVE".into());
    assert!(!format!("{event:?}").contains("COPY-READY-SENSITIVE"));
}
use secrecy::SecretString;
use support::{FakeFactory, ShutdownBlocker, TransportScript, WriteBlocker};
use tokio::sync::broadcast;

const WAIT: Duration = Duration::from_secs(2);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_orders_io_resize_and_clean_shutdown() {
    let (factory, probe) = FakeFactory::new([TransportScript::events([
        TransportEvent::Connected,
        TransportEvent::Output(b"hello\r\n".to_vec()),
    ])]);
    let manager = SessionManager::new(factory);
    let (launch, engine) = support::launch(&probe);
    let mut client = manager.launch(launch).expect("launch");

    let states = collect_through_state(&mut client.events, SessionState::Connected).await;
    assert_eq!(
        states,
        [
            SessionState::Created,
            SessionState::Connecting,
            SessionState::Connected
        ]
    );

    tokio::time::timeout(WAIT, client.frames.changed())
        .await
        .expect("frame timeout")
        .expect("frame channel closed");
    let frame = client.frames.borrow_and_update().clone().expect("frame");
    assert_eq!(frame.rows[0].cells[0].text, "hello");
    assert_eq!(engine.bytes(), b"hello\r\n");

    client
        .try_command(SessionCommand::Select(SelectionRange {
            start: CellPosition {
                stable_row: 0,
                column: 0,
            },
            end: CellPosition {
                stable_row: 0,
                column: 1,
            },
            rectangular: false,
        }))
        .expect("selection accepted");
    tokio::time::timeout(WAIT, client.frames.changed())
        .await
        .expect("selection frame timeout")
        .expect("selection frame channel closed");
    let selected = client.frames.borrow_and_update().clone().expect("frame");
    assert!(selected.rows[0].cells[0].selected);

    manager
        .command(
            client.id,
            SessionCommand::Input(TerminalInput::CommittedText("typed".to_owned())),
        )
        .expect("input accepted");
    client
        .try_command(SessionCommand::Paste(SecretString::from(
            "paste-secret".to_owned(),
        )))
        .expect("paste accepted");
    client
        .try_command(SessionCommand::Resize(support::size()))
        .expect("resize accepted");

    wait_until(|| probe.writes().len() == 2).await;
    wait_until(|| {
        probe
            .log()
            .iter()
            .any(|entry| entry == "transport:resize:1")
    })
    .await;
    assert_eq!(
        probe.writes(),
        vec![(1, b"typed".to_vec()), (1, b"paste-secret".to_vec())]
    );
    let log = probe.log();
    let engine_resize = position(&log, "engine:resize");
    let transport_resize = position(&log, "transport:resize:1");
    assert!(engine_resize < transport_resize, "resize log: {log:?}");

    tokio::time::timeout(WAIT, manager.shutdown_all())
        .await
        .expect("shutdown timeout")
        .expect("shutdown");
    let terminal_states = collect_until_closed(&mut client.events).await;
    assert_eq!(
        terminal_states,
        [SessionState::Closing, SessionState::Exited]
    );
    assert_eq!(count(&probe.log(), "shutdown:1"), 1);
    assert_eq!(
        client.try_command(SessionCommand::Shutdown),
        Err(SessionError::Closed)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_shutdown_error_propagates_through_manager_shutdown() {
    let (factory, probe) = FakeFactory::new([TransportScript::events([TransportEvent::Connected])
        .with_shutdown_failure(SessionFailure::Subprocess)]);
    let manager = SessionManager::new(factory);
    let (launch, _engine) = support::launch(&probe);
    let mut client = manager.launch(launch).expect("launch");
    collect_through_state(&mut client.events, SessionState::Connected).await;

    assert_eq!(
        manager.shutdown_all().await,
        Err(SessionError::TransportShutdown(SessionFailure::Subprocess))
    );
    assert_eq!(manager.active_session_count(), 0);
    assert_eq!(manager.active_child_process_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mouse_command_is_encoded_by_the_engine_before_transport_write() {
    let (factory, probe) = FakeFactory::new([TransportScript::events([TransportEvent::Connected])]);
    let manager = SessionManager::new(factory);
    let (launch, _engine) = support::launch(&probe);
    let mut client = manager.launch(launch).expect("launch");
    collect_through_state(&mut client.events, SessionState::Connected).await;

    let event = TerminalMouseEvent {
        kind: MouseEventKind::Press,
        button: Some(MouseButton::Left),
        cell: CellPosition {
            stable_row: 101,
            column: 4,
        },
        viewport_row: 1,
        pixel_x: 36,
        pixel_y: 54,
        modifiers: KeyModifiers::default(),
    };
    client
        .try_command(SessionCommand::Mouse(event))
        .expect("mouse accepted");
    wait_until(|| !probe.writes().is_empty()).await;
    assert_eq!(probe.writes(), vec![(1, b"mouse:press:4:101:1".to_vec())]);

    manager.shutdown_all().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_writes_exactly_one_etx() {
    let (script, stream) = TransportScript::controlled();
    let (factory, probe) = FakeFactory::new([script]);
    let manager = SessionManager::new(factory);
    let mut settings = TerminalSettingsV1 {
        enable_kitty_keyboard: true,
        ..TerminalSettingsV1::default()
    };
    settings.enable_csi_u = true;
    let terminal = settings.resolve(&TerminalOverrides::default());
    let engine = DefaultTerminalEngine::new(&terminal, support::size()).unwrap();
    let launch = SessionLaunch::new(TransportRequest::new(support::size()), Box::new(engine));
    let mut client = manager.launch(launch).expect("launch");
    stream.send(TransportEvent::Connected);
    collect_through_state(&mut client.events, SessionState::Connected).await;
    stream.send(TransportEvent::Output(b"\x1b[>1u".to_vec()));
    let negotiated = next_frame(&mut client).await;
    assert!(negotiated.display_modes.enhanced_keyboard);

    client
        .try_command(SessionCommand::Interrupt)
        .expect("interrupt accepted");
    wait_until(|| !probe.writes().is_empty()).await;

    assert_eq!(probe.writes(), vec![(1, vec![0x03])]);
    assert!(
        !probe
            .writes()
            .iter()
            .any(|(_, bytes)| bytes == b"\x1b[99;5u")
    );
    manager.shutdown_all().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interruption_notice_waits_for_a_new_authoritative_frame() {
    let (script, stream) = TransportScript::controlled();
    let (factory, probe) = FakeFactory::new([script]);
    let manager = SessionManager::new(factory);
    let mut client = manager
        .launch(real_launch(PresentationPolicy::default()))
        .expect("launch");
    collect_through_state(&mut client.events, SessionState::Connected).await;

    stream.send(TransportEvent::Output(dirty_tui_output()));
    let interrupted = next_frame(&mut client).await;
    assert!(interrupted.display_modes.has_residue());

    client
        .try_command(SessionCommand::Interrupt)
        .expect("interrupt accepted");
    wait_until(|| probe.writes() == vec![(1, vec![0x03])]).await;
    assert_eq!(
        client
            .frames
            .borrow()
            .as_ref()
            .expect("interrupted frame")
            .generation,
        interrupted.generation,
        "the pre-interrupt authoritative frame cannot publish a notice"
    );
    assert_no_recovery_changed(&mut client.events).await;

    stream.send(TransportEvent::Output(
        b"\r\ninterrupt-survived\r\n".to_vec(),
    ));
    let survived = next_frame(&mut client).await;
    let notice = recv_recovery_changed(&mut client.events)
        .await
        .expect("dirty authoritative frame must create a notice");

    assert!(survived.generation > interrupted.generation);
    assert!(survived.display_modes.has_residue());
    assert!(survived.alternate_screen, "surviving TUI must not be reset");
    assert_eq!(notice.interrupted_generation, interrupted.generation);
    assert_eq!(notice.observed_generation, survived.generation);
    assert_eq!(notice.modes, survived.display_modes);

    stream.send(TransportEvent::Output(b"still-running\r\n".to_vec()));
    let later = next_frame(&mut client).await;
    assert!(later.generation > survived.generation);
    assert!(later.display_modes.has_residue());
    assert_no_recovery_changed(&mut client.events).await;

    manager.shutdown_all().await.expect("shutdown");

    let (failed_factory, failed_probe) =
        FakeFactory::new([TransportScript::events([TransportEvent::Connected])
            .with_write_failure(SessionFailure::Network)]);
    let failed_manager = SessionManager::new(failed_factory);
    let (failed_launch, _) =
        support::launch_with_display_modes(&failed_probe, dirty_display_modes());
    let mut failed = failed_manager.launch(failed_launch).expect("failed launch");
    collect_through_state(&mut failed.events, SessionState::Connected).await;
    failed
        .try_command(SessionCommand::Interrupt)
        .expect("failed interrupt accepted");
    let failed_events = recv_until_closed(&mut failed.events).await;

    assert!(
        failed_probe.writes().is_empty(),
        "failed ETX must not be recorded"
    );
    assert!(
        failed_events
            .iter()
            .any(|event| { matches!(event, SessionEvent::Failed(SessionFailure::Network)) })
    );
    assert!(
        failed_events
            .iter()
            .all(|event| { !matches!(event, SessionEvent::RecoveryChanged(Some(_))) })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_display_publishes_newer_clean_primary_frame_without_transport_write() {
    let (script, stream) = TransportScript::controlled();
    let (factory, probe) = FakeFactory::new([script]);
    let manager = SessionManager::new(factory);
    let mut settings = TerminalSettingsV1 {
        enable_kitty_keyboard: true,
        ..TerminalSettingsV1::default()
    };
    settings.scrollback_lines = 1_000;
    let terminal = settings.resolve(&TerminalOverrides::default());
    let engine = DefaultTerminalEngine::new(&terminal, support::size()).unwrap();
    let launch = SessionLaunch::new(TransportRequest::new(support::size()), Box::new(engine));
    let mut client = manager.launch(launch).expect("launch");
    stream.send(TransportEvent::Connected);
    collect_through_state(&mut client.events, SessionState::Connected).await;
    stream.send(TransportEvent::Output(
        b"primary-history\r\nprimary-tail\x1b]0;stale\x07\x1b[?1049h\x1b[>1u\x1b[?1000h\x1b[?1h\x1b[?25lalt"
            .to_vec(),
    ));
    let dirty = next_frame(&mut client).await;
    assert!(dirty.display_modes.has_residue());

    client
        .try_command(SessionCommand::ResetDisplay)
        .expect("reset accepted");
    let recovered = next_frame(&mut client).await;

    assert!(recovered.generation > dirty.generation);
    assert_eq!(recovered.display_modes, Default::default());
    assert_eq!(recovered.title, "rsHell");
    assert!(frame_contains(&recovered, "primary-tail"));
    assert!(!frame_contains(&recovered, "alt"));
    assert!(
        probe.writes().is_empty(),
        "display recovery must not write escapes"
    );
    manager.shutdown_all().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_commands_report_backpressure_without_losing_accepted_input() {
    let blocker = WriteBlocker::new();
    let (factory, probe) =
        FakeFactory::new([TransportScript::events([TransportEvent::Connected])
            .with_write_blocker(blocker.clone())]);
    let manager = SessionManager::new(factory);
    let (launch, _engine) = support::launch(&probe);
    let mut client = manager.launch(launch).expect("launch");
    collect_through_state(&mut client.events, SessionState::Connected).await;

    client
        .try_command(SessionCommand::Input(TerminalInput::CommittedText(
            "blocked".to_owned(),
        )))
        .expect("blocking command accepted");
    blocker.wait_started().await;

    for index in 0..128 {
        client
            .try_command(SessionCommand::Input(TerminalInput::CommittedText(
                format!("queued-{index}"),
            )))
            .expect("bounded command accepted");
    }
    assert_eq!(
        manager.command(
            client.id,
            SessionCommand::Input(TerminalInput::CommittedText("overflow".to_owned()))
        ),
        Err(SessionError::Backpressure)
    );

    blocker.release();
    wait_until(|| probe.writes().len() == 129).await;
    let writes = probe.writes();
    assert_eq!(writes[0].1, b"blocked");
    assert_eq!(writes[128].1, b"queued-127");
    assert!(!writes.iter().any(|(_, bytes)| bytes == b"overflow"));

    tokio::time::timeout(WAIT, manager.shutdown_all())
        .await
        .expect("shutdown timeout")
        .expect("shutdown");
}

#[tokio::test(start_paused = true)]
async fn ten_thousand_output_burst_is_latest_only_and_rate_limited() {
    let started = tokio::time::Instant::now();
    let (factory, probe) = FakeFactory::new([TransportScript::burst(10_000)]);
    let manager = SessionManager::new(factory);
    let (launch, engine) = support::launch(&probe);
    let mut client = manager.launch(launch).expect("launch");
    collect_through_state(&mut client.events, SessionState::Connected).await;
    manager
        .command(
            client.id,
            SessionCommand::Input(TerminalInput::CommittedText("priority".to_owned())),
        )
        .expect("command accepted during output burst");

    tokio::time::timeout(WAIT, async {
        loop {
            if client
                .frames
                .borrow_and_update()
                .as_ref()
                .is_some_and(|frame| frame.rows[0].cells[0].text == "line10000")
            {
                break;
            }
            client.frames.changed().await.expect("frame channel closed");
        }
    })
    .await
    .expect("final frame timeout");
    let elapsed = started.elapsed();
    let renders = engine.render_count();
    assert!(renders > 0, "burst produced no frame");
    assert!(
        renders <= 16,
        "burst produced {renders} frames over {elapsed:?}"
    );
    assert!(
        (Duration::from_millis(250)..Duration::from_millis(267)).contains(&elapsed),
        "burst completed outside the deterministic observation window: {elapsed:?}"
    );
    assert_eq!(probe.writes(), vec![(1, b"priority".to_vec())]);
    let mut frame_events = 0;
    loop {
        match tokio::time::timeout(Duration::from_millis(5), client.events.recv()).await {
            Ok(Ok(SessionEvent::FrameReady(_))) => frame_events += 1,
            Ok(Ok(_)) => {}
            Ok(Err(broadcast::error::RecvError::Lagged(skipped))) => {
                panic!("frame updates flooded broadcast by {skipped} events")
            }
            Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => break,
        }
    }
    assert_eq!(frame_events, 1, "only the first frame is broadcast");

    tokio::time::timeout(WAIT, manager.shutdown_all())
        .await
        .expect("shutdown timeout")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presentation_mutations_publish_monotonic_generations() {
    let (factory, probe) = FakeFactory::new([TransportScript::events([
        TransportEvent::Connected,
        TransportEvent::Output(b"first\r\n".to_vec()),
    ])]);
    let manager = SessionManager::new(factory);
    let (launch, _) = support::launch_fixed_generation(&probe);
    let launch = launch.with_presentation_policy(PresentationPolicy {
        scroll_on_output: true,
        scroll_on_keypress: true,
    });
    let mut client = manager.launch(launch).expect("launch");
    collect_through_state(&mut client.events, SessionState::Connected).await;

    let initial = next_frame(&mut client).await;
    assert_eq!(
        initial.generation, 1,
        "actor stamps the fixed backend frame"
    );

    client
        .try_command(SessionCommand::Select(SelectionRange {
            start: CellPosition {
                stable_row: 0,
                column: 0,
            },
            end: CellPosition {
                stable_row: 0,
                column: 1,
            },
            rectangular: false,
        }))
        .expect("selection accepted");
    let selected = next_frame(&mut client).await;
    assert!(selected.generation > initial.generation);

    client
        .try_command(SessionCommand::Scroll(-1))
        .expect("scroll accepted");
    let scrolled = next_frame(&mut client).await;
    assert!(scrolled.generation > selected.generation);

    client
        .try_command(SessionCommand::Resize(support::size()))
        .expect("resize accepted");
    let resized = next_frame(&mut client).await;
    assert!(resized.generation > scrolled.generation);

    client
        .try_command(SessionCommand::Input(TerminalInput::CommittedText(
            "key".to_owned(),
        )))
        .expect("input accepted");
    let keyed = next_frame(&mut client).await;
    assert!(keyed.generation > resized.generation);

    manager.shutdown_all().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_scrollback_publishes_fresh_frame() {
    let (factory, probe) = FakeFactory::new([TransportScript::events([
        TransportEvent::Connected,
        TransportEvent::Output(b"old-scrollback\r\nvisible".to_vec()),
    ])]);
    let manager = SessionManager::new(factory);
    let (launch, engine) = support::launch_fixed_generation(&probe);
    let mut client = manager.launch(launch).expect("launch");
    collect_through_state(&mut client.events, SessionState::Connected).await;
    let initial = next_frame(&mut client).await;
    assert!(initial.rows[0].cells[0].text.contains("visible"));

    client
        .try_command(SessionCommand::ClearScrollback)
        .expect("clear scrollback accepted");
    let cleared = next_frame(&mut client).await;

    assert!(cleared.generation > initial.generation);
    assert!(engine.bytes().is_empty());
    assert!(
        probe
            .log()
            .iter()
            .any(|entry| entry == "engine:clear_scrollback")
    );
    manager.shutdown_all().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scroll_on_keypress_alone_controls_fresh_frame_publication() {
    let (disabled_script, disabled_stream) = TransportScript::controlled();
    let (disabled_factory, disabled_probe) = FakeFactory::new([disabled_script]);
    let disabled_manager = SessionManager::new(disabled_factory);
    let (disabled_launch, _) = support::launch_fixed_generation(&disabled_probe);
    let disabled_launch = disabled_launch.with_presentation_policy(PresentationPolicy {
        scroll_on_output: false,
        scroll_on_keypress: false,
    });
    let mut disabled = disabled_manager.launch(disabled_launch).expect("launch");
    disabled_stream.send(TransportEvent::Connected);
    collect_through_state(&mut disabled.events, SessionState::Connected).await;
    disabled_stream.send(TransportEvent::Output(b"initial\r\n".to_vec()));
    let disabled_initial = next_frame(&mut disabled).await;
    disabled
        .try_command(SessionCommand::Input(TerminalInput::CommittedText(
            "no-snap".to_owned(),
        )))
        .expect("input accepted");
    wait_until(|| !disabled_probe.writes().is_empty()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), disabled.frames.changed())
            .await
            .is_err(),
        "disabled scroll_on_keypress must not publish a presentation-only frame"
    );

    let (enabled_script, enabled_stream) = TransportScript::controlled();
    let (enabled_factory, enabled_probe) = FakeFactory::new([enabled_script]);
    let enabled_manager = SessionManager::new(enabled_factory);
    let (enabled_launch, _) = support::launch_fixed_generation(&enabled_probe);
    let enabled_launch = enabled_launch.with_presentation_policy(PresentationPolicy {
        scroll_on_output: false,
        scroll_on_keypress: true,
    });
    let mut enabled = enabled_manager.launch(enabled_launch).expect("launch");
    enabled_stream.send(TransportEvent::Connected);
    collect_through_state(&mut enabled.events, SessionState::Connected).await;
    enabled_stream.send(TransportEvent::Output(b"initial\r\n".to_vec()));
    let enabled_initial = next_frame(&mut enabled).await;
    enabled
        .try_command(SessionCommand::Input(TerminalInput::CommittedText(
            "snap".to_owned(),
        )))
        .expect("input accepted");
    let snapped = next_frame(&mut enabled).await;
    assert!(snapped.generation > enabled_initial.generation);
    assert_eq!(disabled_initial.generation, 1);

    disabled_manager.shutdown_all().await.expect("shutdown");
    enabled_manager.shutdown_all().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_output_follows_bottom_and_explicit_scroll_preserves_history() {
    let (script, stream) = TransportScript::controlled();
    let (factory, _probe) = FakeFactory::new([script]);
    let manager = SessionManager::new(factory);
    let policy = PresentationPolicy {
        scroll_on_output: false,
        scroll_on_keypress: false,
    };
    let mut client = manager.launch(real_launch(policy)).expect("launch");
    stream.send(TransportEvent::Connected);
    collect_through_state(&mut client.events, SessionState::Connected).await;

    stream.send(TransportEvent::Output(numbered_lines("initial", 40)));
    let initial = next_frame(&mut client).await;
    assert!(
        initial.viewport_top > 0,
        "initial frame follows the newest rows"
    );
    assert!(frame_contains(&initial, "initial-039"));

    client
        .try_command(SessionCommand::Scroll(-5))
        .expect("scroll up accepted");
    let historical = next_frame(&mut client).await;
    assert!(historical.viewport_top < initial.viewport_top);

    stream.send(TransportEvent::Output(b"after-scroll\r\n".to_vec()));
    let anchored = next_frame(&mut client).await;
    assert_eq!(anchored.viewport_top, historical.viewport_top);

    client
        .try_command(SessionCommand::Scroll(i32::MAX))
        .expect("scroll to bottom accepted");
    let bottom = next_frame(&mut client).await;
    assert!(bottom.viewport_top > historical.viewport_top);

    stream.send(TransportEvent::Output(b"newest\r\n".to_vec()));
    let resumed = next_frame(&mut client).await;
    assert!(resumed.viewport_top > bottom.viewport_top);
    assert!(frame_contains(&resumed, "newest"));

    manager.shutdown_all().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnects_are_serial_and_duplicate_shutdown_is_idempotent() {
    let scripts = (0..3).map(|_| TransportScript::events([TransportEvent::Connected]));
    let (factory, probe) = FakeFactory::new(scripts);
    let manager = SessionManager::new(factory);
    let (launch, _engine) = support::launch(&probe);
    let mut client = manager.launch(launch).expect("launch");
    collect_through_state(&mut client.events, SessionState::Connected).await;

    client
        .try_command(SessionCommand::Reconnect)
        .expect("first reconnect");
    client
        .try_command(SessionCommand::Reconnect)
        .expect("second reconnect");
    wait_until(|| probe.log().iter().any(|entry| entry == "connect:3")).await;

    let log = probe.log();
    assert!(position(&log, "shutdown:1") < position(&log, "create:2"));
    assert!(position(&log, "create:2") < position(&log, "connect:2"));
    assert!(position(&log, "connect:2") < position(&log, "shutdown:2"));
    assert!(position(&log, "shutdown:2") < position(&log, "create:3"));
    assert!(position(&log, "create:3") < position(&log, "connect:3"));

    client
        .try_command(SessionCommand::Shutdown)
        .expect("first shutdown");
    let _ = client.try_command(SessionCommand::Shutdown);
    tokio::time::timeout(WAIT, manager.shutdown_all())
        .await
        .expect("shutdown timeout")
        .expect("shutdown");
    manager.shutdown_all().await.expect("repeated shutdown_all");

    let states = collect_until_closed(&mut client.events).await;
    assert_eq!(
        states
            .iter()
            .filter(|state| **state == SessionState::Closing)
            .count(),
        1
    );
    assert_eq!(count(&probe.log(), "shutdown:3"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dirty_reconnect_recovers_before_old_shutdown_and_new_connect() {
    let shutdown_blocker = ShutdownBlocker::new();
    let (first, first_stream) = TransportScript::controlled();
    let (factory, probe) = FakeFactory::new([
        first.with_shutdown_blocker(shutdown_blocker.clone()),
        TransportScript::events([TransportEvent::Connected]),
    ]);
    let manager = SessionManager::new(factory);
    let (launch, engine) = support::launch_with_display_modes(&probe, dirty_display_modes());
    let mut client = manager.launch(launch).expect("launch");
    let session_id = client.id;
    collect_through_state(&mut client.events, SessionState::Connected).await;

    first_stream.send(TransportEvent::Output(
        b"primary-before-reconnect\r\n".to_vec(),
    ));
    let dirty = next_frame(&mut client).await;
    assert!(dirty.display_modes.has_residue());

    client
        .try_command(SessionCommand::Reconnect)
        .expect("reconnect accepted");
    let clean = next_frame(&mut client).await;
    let notice = recv_recovery_changed(&mut client.events).await;

    assert!(clean.generation > dirty.generation);
    assert_clean_final_frame(&clean, "primary-before-reconnect");
    assert_eq!(notice, None);
    assert_eq!(
        client.id, session_id,
        "reconnect keeps the same actor client"
    );
    assert_eq!(engine.recover_display_count(), 1);
    assert_eq!(engine.bytes(), b"primary-before-reconnect\r\n");

    shutdown_blocker.wait_started().await;
    let before_replacement = probe.log();
    assert_eq!(count(&before_replacement, "create:1"), 1);
    assert_eq!(count(&before_replacement, "shutdown:1"), 1);
    assert_eq!(count(&before_replacement, "create:2"), 0);
    assert_eq!(count(&before_replacement, "connect:2"), 0);

    shutdown_blocker.release();
    wait_until(|| probe.log().iter().any(|entry| entry == "connect:2")).await;
    let log = probe.log();
    assert!(position(&log, "shutdown:1") < position(&log, "create:2"));
    assert!(position(&log, "create:2") < position(&log, "connect:2"));

    client
        .try_command(SessionCommand::Input(TerminalInput::CommittedText(
            "same-actor".to_owned(),
        )))
        .expect("replacement input accepted through original client");
    wait_until(|| probe.writes() == vec![(2, b"same-actor".to_vec())]).await;
    manager.shutdown_all().await.expect("shutdown");

    let failing_shutdown_blocker = ShutdownBlocker::new();
    let (failing_first, failing_stream) = TransportScript::controlled();
    let (failing_factory, failing_probe) = FakeFactory::new([
        failing_first.with_shutdown_blocker(failing_shutdown_blocker.clone()),
        TransportScript::events([TransportEvent::Connected]),
    ]);
    let failing_manager = SessionManager::new(failing_factory);
    let (failing_launch, _) =
        support::launch_with_recovery_failure(&failing_probe, dirty_display_modes());
    let mut failing = failing_manager
        .launch(failing_launch)
        .expect("failing launch");
    collect_through_state(&mut failing.events, SessionState::Connected).await;
    failing_stream.send(TransportEvent::Output(
        b"dirty-before-failed-reconnect\r\n".to_vec(),
    ));
    let dirty = next_frame(&mut failing).await;
    assert!(dirty.display_modes.has_residue());

    failing
        .try_command(SessionCommand::Reconnect)
        .expect("failing reconnect accepted");
    assert_eq!(
        recv_failure(&mut failing.events).await,
        SessionFailure::Platform
    );
    failing_shutdown_blocker.wait_started().await;
    wait_until(|| {
        failing_probe
            .log()
            .iter()
            .any(|entry| entry == "shutdown:1")
    })
    .await;
    let failed_log = failing_probe.log();
    assert_eq!(count(&failed_log, "create:2"), 0);
    assert_eq!(count(&failed_log, "connect:2"), 0);
    failing_shutdown_blocker.release();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_outcomes_clean_registry_and_manager_remains_usable() {
    let (factory, probe) = FakeFactory::new([
        TransportScript::events([TransportEvent::Eof]),
        TransportScript::events([TransportEvent::Exit(ExitStatus {
            code: Some(17),
            success: false,
        })]),
        TransportScript::events([TransportEvent::Failure(TransportError::new(
            SessionFailure::Network,
        ))]),
        TransportScript::events([TransportEvent::Connected]),
    ]);
    let manager = SessionManager::new(factory);

    let (launch, _) = support::launch(&probe);
    let mut eof = manager.launch(launch).expect("eof launch");
    assert_eq!(
        recv_exit(&mut eof.events).await,
        ExitStatus {
            code: None,
            success: true
        }
    );
    wait_for_event_channel_close(&mut eof.events).await;
    assert_unknown_session(&manager, eof.id);

    let (launch, _) = support::launch(&probe);
    let mut nonzero = manager.launch(launch).expect("exit launch");
    assert_eq!(
        recv_exit(&mut nonzero.events).await,
        ExitStatus {
            code: Some(17),
            success: false
        }
    );
    wait_for_event_channel_close(&mut nonzero.events).await;
    assert_unknown_session(&manager, nonzero.id);

    let (launch, _) = support::launch(&probe);
    let mut failed = manager.launch(launch).expect("failure launch");
    assert_eq!(
        recv_failure(&mut failed.events).await,
        SessionFailure::Network
    );
    wait_for_event_channel_close(&mut failed.events).await;
    assert_unknown_session(&manager, failed.id);

    let (launch, _) = support::launch(&probe);
    let mut healthy = manager.launch(launch).expect("post-cleanup launch");
    collect_through_state(&mut healthy.events, SessionState::Connected).await;

    tokio::time::timeout(WAIT, manager.shutdown_all())
        .await
        .expect("shutdown timeout")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_terminal_completion_recovers_before_event() {
    #[derive(Clone, Copy)]
    enum Completion {
        Eof,
        Exit(ExitStatus),
        TransportFailure(SessionFailure),
        Shutdown,
        ShutdownFailure(SessionFailure),
        IllegalTransition,
    }

    let cases = [
        Completion::Eof,
        Completion::Exit(ExitStatus {
            code: Some(17),
            success: false,
        }),
        Completion::TransportFailure(SessionFailure::Network),
        Completion::Shutdown,
        Completion::ShutdownFailure(SessionFailure::Subprocess),
        Completion::IllegalTransition,
    ];

    for completion in cases {
        let (script, stream) = TransportScript::controlled();
        let script = match completion {
            Completion::ShutdownFailure(failure) => script.with_shutdown_failure(failure),
            _ => script,
        };
        let (factory, _probe) = FakeFactory::new([script]);
        let manager = SessionManager::new(factory);
        let mut client = manager
            .launch(real_launch(PresentationPolicy::default()))
            .expect("launch");
        collect_through_state(&mut client.events, SessionState::Connected).await;

        stream.send(TransportEvent::Output(dirty_tui_output()));
        let dirty = next_frame(&mut client).await;
        assert!(dirty.display_modes.has_residue());

        match completion {
            Completion::Eof => stream.send(TransportEvent::Eof),
            Completion::Exit(status) => stream.send(TransportEvent::Exit(status)),
            Completion::TransportFailure(failure) => {
                stream.send(TransportEvent::Failure(TransportError::new(failure)));
            }
            Completion::Shutdown | Completion::ShutdownFailure(_) => client
                .try_command(SessionCommand::Shutdown)
                .expect("shutdown accepted"),
            Completion::IllegalTransition => {
                stream.send(TransportEvent::AwaitingHostKey);
                stream.send(TransportEvent::AwaitingHostKey);
            }
        }

        let clean = next_frame(&mut client).await;
        assert!(clean.generation > dirty.generation);
        assert_clean_final_frame(&clean, "primary-before-terminal");
        let events = recv_until_closed(&mut client.events).await;

        let recovery = events
            .iter()
            .position(|event| matches!(event, SessionEvent::RecoveryChanged(None)))
            .expect("terminal completion must clear recovery state");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::RecoveryChanged(None)))
                .count(),
            1,
            "terminal completion must publish one recovery clear"
        );
        let terminal = events
            .iter()
            .position(|event| match (completion, event) {
                (Completion::Eof | Completion::Shutdown, SessionEvent::Exited(status)) => {
                    *status
                        == ExitStatus {
                            code: None,
                            success: true,
                        }
                }
                (Completion::Exit(expected), SessionEvent::Exited(actual)) => *actual == expected,
                (
                    Completion::TransportFailure(expected) | Completion::ShutdownFailure(expected),
                    SessionEvent::Failed(actual),
                ) => *actual == expected,
                (Completion::IllegalTransition, SessionEvent::Failed(SessionFailure::Crashed)) => {
                    true
                }
                _ => false,
            })
            .unwrap_or_else(|| panic!("missing expected terminal event: {events:?}"));
        assert!(
            recovery < terminal,
            "recovery must precede the terminal lifecycle event: {events:?}"
        );
        if matches!(completion, Completion::Shutdown) {
            let closing = events
                .iter()
                .position(|event| {
                    matches!(event, SessionEvent::StateChanged(SessionState::Closing))
                })
                .expect("shutdown must enter Closing");
            assert!(
                recovery < closing,
                "recovery must precede shutdown Closing: {events:?}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn awaiting_states_preserve_clean_and_nonzero_exits() {
    let nonzero_status = ExitStatus {
        code: Some(23),
        success: false,
    };
    let (factory, probe) = FakeFactory::new([
        TransportScript::events([TransportEvent::AwaitingHostKey, TransportEvent::Eof]),
        TransportScript::events([
            TransportEvent::AwaitingAuthentication,
            TransportEvent::Exit(nonzero_status),
        ]),
    ]);
    let manager = SessionManager::new(factory);

    let (launch, _) = support::launch(&probe);
    let mut host_key = manager.launch(launch).expect("host-key launch");
    let (states, status) = recv_exit_with_states(&mut host_key.events).await;
    assert_eq!(
        states,
        [
            SessionState::Created,
            SessionState::Connecting,
            SessionState::Connected,
            SessionState::AwaitingHostKey,
            SessionState::Exited,
        ]
    );
    assert_eq!(
        status,
        ExitStatus {
            code: None,
            success: true,
        }
    );
    wait_for_event_channel_close(&mut host_key.events).await;
    assert_unknown_session(&manager, host_key.id);

    let (launch, _) = support::launch(&probe);
    let mut authentication = manager.launch(launch).expect("authentication launch");
    let (states, status) = recv_exit_with_states(&mut authentication.events).await;
    assert_eq!(
        states,
        [
            SessionState::Created,
            SessionState::Connecting,
            SessionState::Connected,
            SessionState::AwaitingAuthentication,
            SessionState::Exited,
        ]
    );
    assert_eq!(status, nonzero_status);
    wait_for_event_channel_close(&mut authentication.events).await;
    assert_unknown_session(&manager, authentication.id);
    manager.shutdown_all().await.expect("empty shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn illegal_lifecycle_transition_fails_deterministically() {
    let (factory, probe) = FakeFactory::new([TransportScript::events([
        TransportEvent::AwaitingHostKey,
        TransportEvent::AwaitingHostKey,
    ])]);
    let manager = SessionManager::new(factory);
    let (launch, _) = support::launch(&probe);
    let mut client = manager.launch(launch).expect("launch");

    assert_eq!(
        recv_failure(&mut client.events).await,
        SessionFailure::Crashed
    );
    tokio::time::timeout(WAIT, manager.shutdown_all())
        .await
        .expect("shutdown timeout")
        .expect("shutdown");
    while let Ok(Ok(event)) =
        tokio::time::timeout(Duration::from_millis(20), client.events.recv()).await
    {
        assert!(
            !matches!(event, SessionEvent::Crashed(_)),
            "illegal transitions are classified failures, not actor panics"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interaction_round_trip_is_routed_without_secret_debug_output() {
    let interaction_id = InteractionId::new();
    let host_key = InteractionRequest::HostKey(HostKeyPrompt {
        id: interaction_id,
        host: "example.test".to_owned(),
        port: 22,
        algorithm: "ssh-ed25519".to_owned(),
        sha256: "SHA256:fixture".to_owned(),
        changed: false,
    });
    let auth_id = InteractionId::new();
    let authentication = InteractionRequest::Password(AuthPrompt {
        id: auth_id,
        label: "Password".to_owned(),
        echo: false,
    });
    let (factory, probe) = FakeFactory::new([TransportScript::interacting_many([
        host_key,
        authentication,
    ])]);
    let manager = SessionManager::new(factory);
    let (launch, _) = support::launch(&probe);
    let mut client = manager.launch(launch).expect("launch");

    let mut states = Vec::new();
    loop {
        match recv(&mut client.events).await {
            SessionEvent::StateChanged(state) => states.push(state),
            SessionEvent::InteractionRequired(InteractionRequest::HostKey(prompt)) => {
                assert_eq!(prompt.id, interaction_id);
                manager
                    .command(
                        client.id,
                        SessionCommand::Respond(
                            interaction_id,
                            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore),
                        ),
                    )
                    .expect("interaction response");
            }
            SessionEvent::InteractionRequired(InteractionRequest::Password(prompt)) => {
                assert_eq!(prompt.id, auth_id);
                manager
                    .command(
                        client.id,
                        SessionCommand::Respond(
                            auth_id,
                            InteractionResponse::Secret(SecretString::from(
                                "authentication-secret".to_owned(),
                            )),
                        ),
                    )
                    .expect("authentication response");
            }
            _ => {}
        }
        if states.last() == Some(&SessionState::Connected) {
            break;
        }
    }
    assert_eq!(
        states,
        [
            SessionState::Created,
            SessionState::Connecting,
            SessionState::AwaitingHostKey,
            SessionState::AwaitingAuthentication,
            SessionState::Connected
        ]
    );

    let secret = "must-not-appear";
    let paste_debug = format!(
        "{:?}",
        SessionCommand::Paste(SecretString::from(secret.to_owned()))
    );
    let response_debug = format!(
        "{:?}",
        InteractionResponse::Secret(SecretString::from(secret.to_owned()))
    );
    assert!(!paste_debug.contains(secret));
    assert!(!response_debug.contains(secret));

    manager.shutdown_all().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_panic_is_generic_once_and_manager_launches_again() {
    let (factory, probe) = FakeFactory::new([TransportScript::panic(), TransportScript::pending()]);
    let manager = SessionManager::new(factory);
    let (launch, _) = support::launch(&probe);
    let mut crashed = manager.launch(launch).expect("panic launch");

    let events = recv_until_closed(&mut crashed.events).await;
    let recovery = events
        .iter()
        .position(|event| matches!(event, SessionEvent::RecoveryChanged(None)))
        .expect("panic supervisor must clear recovery state");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::RecoveryChanged(None)))
            .count(),
        1
    );
    let crash_state = events
        .iter()
        .position(|event| matches!(event, SessionEvent::StateChanged(SessionState::Crashed)))
        .expect("panic supervisor must publish Crashed state");
    let crash = events
        .iter()
        .position(|event| matches!(event, SessionEvent::Crashed(message) if message == "session actor crashed"))
        .expect("panic supervisor must publish generic crash");
    assert!(recovery < crash_state);
    assert!(recovery < crash);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::Crashed(_)))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::StateChanged(SessionState::Crashed)))
            .count(),
        1
    );
    assert!(
        crashed.frames.borrow().is_none(),
        "panic supervisor must not fabricate a terminal frame"
    );
    assert_unknown_session(&manager, crashed.id);

    let (launch, _) = support::launch(&probe);
    let mut healthy = manager.launch(launch).expect("second launch");
    collect_through_state(&mut healthy.events, SessionState::Connected).await;
    tokio::time::timeout(WAIT, manager.shutdown_all())
        .await
        .expect("manager hung after panic")
        .expect("shutdown");
}

async fn collect_through_state(
    receiver: &mut broadcast::Receiver<SessionEvent>,
    target: SessionState,
) -> Vec<SessionState> {
    let mut states = Vec::new();
    loop {
        if let SessionEvent::StateChanged(state) = recv(receiver).await {
            states.push(state);
            if state == target {
                return states;
            }
        }
    }
}

async fn collect_until_closed(
    receiver: &mut broadcast::Receiver<SessionEvent>,
) -> Vec<SessionState> {
    recv_until_closed(receiver)
        .await
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::StateChanged(state) => Some(state),
            _ => None,
        })
        .collect()
}

async fn recv_until_closed(receiver: &mut broadcast::Receiver<SessionEvent>) -> Vec<SessionEvent> {
    tokio::time::timeout(WAIT, async {
        let mut events = Vec::new();
        loop {
            match receiver.recv().await {
                Ok(event) => events.push(event),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    panic!("event receiver lagged by {skipped}")
                }
                Err(broadcast::error::RecvError::Closed) => return events,
            }
        }
    })
    .await
    .expect("event channel did not close")
}

async fn wait_for_event_channel_close(receiver: &mut broadcast::Receiver<SessionEvent>) {
    let _ = recv_until_closed(receiver).await;
}

async fn recv(receiver: &mut broadcast::Receiver<SessionEvent>) -> SessionEvent {
    match tokio::time::timeout(WAIT, receiver.recv())
        .await
        .expect("event timeout")
    {
        Ok(event) => event,
        Err(broadcast::error::RecvError::Lagged(skipped)) => {
            panic!("event receiver lagged by {skipped}")
        }
        Err(broadcast::error::RecvError::Closed) => panic!("event channel closed"),
    }
}

async fn recv_exit(receiver: &mut broadcast::Receiver<SessionEvent>) -> ExitStatus {
    loop {
        if let SessionEvent::Exited(status) = recv(receiver).await {
            return status;
        }
    }
}

async fn recv_exit_with_states(
    receiver: &mut broadcast::Receiver<SessionEvent>,
) -> (Vec<SessionState>, ExitStatus) {
    let mut states = Vec::new();
    loop {
        match recv(receiver).await {
            SessionEvent::StateChanged(state) => states.push(state),
            SessionEvent::Exited(status) => return (states, status),
            _ => {}
        }
    }
}

async fn recv_failure(receiver: &mut broadcast::Receiver<SessionEvent>) -> SessionFailure {
    loop {
        if let SessionEvent::Failed(failure) = recv(receiver).await {
            return failure;
        }
    }
}

async fn recv_recovery_changed(
    receiver: &mut broadcast::Receiver<SessionEvent>,
) -> Option<rshell_core::DisplayRecoveryNotice> {
    loop {
        if let SessionEvent::RecoveryChanged(notice) = recv(receiver).await {
            return notice;
        }
    }
}

async fn assert_no_recovery_changed(receiver: &mut broadcast::Receiver<SessionEvent>) {
    let result = tokio::time::timeout(Duration::from_millis(50), async {
        loop {
            match receiver.recv().await {
                Ok(SessionEvent::RecoveryChanged(notice)) => {
                    panic!("unexpected recovery transition: {notice:?}")
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    panic!("event receiver lagged by {skipped}")
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    })
    .await;
    assert!(
        result.is_err(),
        "event stream closed before observation window"
    );
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("condition timeout");
}

async fn next_frame(
    client: &mut rshell_session::SessionClient,
) -> std::sync::Arc<rshell_core::RenderFrame> {
    tokio::time::timeout(WAIT, client.frames.changed())
        .await
        .expect("frame timeout")
        .expect("frame channel closed");
    client.frames.borrow_and_update().clone().expect("frame")
}

fn real_launch(policy: PresentationPolicy) -> SessionLaunch {
    let terminal = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    let engine = DefaultTerminalEngine::new(&terminal, support::size()).expect("engine");
    SessionLaunch::new(TransportRequest::new(support::size()), Box::new(engine))
        .with_presentation_policy(policy)
}

fn numbered_lines(prefix: &str, count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| format!("{prefix}-{index:03}\r\n"))
        .collect::<String>()
        .into_bytes()
}

fn frame_contains(frame: &rshell_core::RenderFrame, expected: &str) -> bool {
    frame.rows.iter().any(|row| {
        row.cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains(expected)
    })
}

fn dirty_tui_output() -> Vec<u8> {
    let mut output = b"primary-before-terminal\r\n".to_vec();
    output.extend_from_slice(support::display_recovery::MODE_SEQUENCE);
    output
}

fn dirty_display_modes() -> TerminalDisplayModes {
    TerminalDisplayModes {
        alternate_screen: true,
        enhanced_keyboard: true,
        mouse_reporting: true,
        application_cursor: true,
        cursor_hidden: true,
        stale_title: true,
    }
}

fn assert_clean_final_frame(frame: &rshell_core::RenderFrame, primary_text: &str) {
    assert_eq!(frame.display_modes, TerminalDisplayModes::default());
    assert!(!frame.alternate_screen);
    assert!(!frame.mouse_reporting);
    assert_eq!(frame.title, "rsHell");
    assert!(frame_contains(frame, primary_text));
    assert!(!frame_contains(frame, "fixture-界-e"));
    assert!(
        !frame
            .rows
            .iter()
            .flat_map(|row| row.cells.iter())
            .any(|cell| { cell.text.contains('\u{fffd}') })
    );
}

fn assert_unknown_session(manager: &SessionManager, id: rshell_core::SessionId) {
    assert_eq!(
        manager.command(id, SessionCommand::Shutdown),
        Err(SessionError::UnknownSession)
    );
}

fn position(log: &[String], expected: &str) -> usize {
    log.iter()
        .position(|entry| entry == expected)
        .unwrap_or_else(|| panic!("missing {expected} in {log:?}"))
}

fn count(log: &[String], expected: &str) -> usize {
    log.iter()
        .filter(|entry| entry.as_str() == expected)
        .count()
}

#[test]
fn factory_is_reusable_and_publicly_object_safe() {
    fn accepts_factory(_: Arc<dyn rshell_session::TransportFactory>) {}
    let (factory, _) = FakeFactory::new([TransportScript::pending()]);
    accepts_factory(factory);
}
