#[allow(dead_code)]
mod support;

use std::{sync::Arc, time::Duration};

use rshell_core::{
    AppEvent, ApplicationService, PaneLaunchTarget, RenderFrame, SessionFailure, SessionState,
    SessionUiEvent, SplitAxis, TerminalSize, UiCommand,
};
use support::{RecordingPorts, bootstrap_state};
use tokio::time::timeout;

#[tokio::test]
async fn retry_uses_fresh_actor_after_old_shutdown_and_clears_stale_state() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let tab = &app.initial_view_model().workspace.tabs[0];
    let pane = tab.active_pane;
    let old = tab.pane_tree.session_id(pane).unwrap().unwrap();
    ports.send_session_event(
        old,
        SessionUiEvent::Failed(rshell_core::SessionFailure::Network),
    );
    recv_matching(&events, |event| {
        matches!(event, AppEvent::Session { session, event: SessionUiEvent::Failed(_) } if *session == old)
    })
    .await;
    ports.clear_calls();

    app.ui_port().try_send(UiCommand::RetryPane(pane)).unwrap();
    recv_matching(&events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    let view = app.view_model();
    let new = view.workspace.tabs[0]
        .pane_tree
        .session_id(pane)
        .unwrap()
        .unwrap();

    assert_ne!(old, new);
    assert_eq!(
        ports.calls(),
        ["session.shutdown", "session.launch_local"],
        "old shutdown must complete before launch"
    );
    assert!(!ports.is_session_live(old));
    assert!(ports.is_session_live(new));
    assert!(!view.latest_frames.contains_key(&old));
    assert!(!view.error_panes.contains_key(&old));
    assert!(!view.session_states.contains_key(&old));

    ports.send_frame(old, frame(99));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!app.view_model().latest_frames.contains_key(&old));
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn close_leaf_waits_for_shutdown_and_last_leaf_closes_tab() {
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
    let split = recv_workspace(&events).await;
    let second_pane = split.tabs[0].active_pane;
    let second_session = split.tabs[0]
        .pane_tree
        .session_id(second_pane)
        .unwrap()
        .unwrap();
    ports.clear_calls();

    app.ui_port()
        .try_send(UiCommand::ClosePane(second_pane))
        .unwrap();
    let collapsed = recv_workspace(&events).await;
    assert_eq!(ports.calls(), ["session.shutdown"]);
    assert!(!ports.is_session_live(second_session));
    assert_eq!(collapsed.tabs[0].pane_tree.pane_ids(), [first_pane]);

    let first_session = collapsed.tabs[0]
        .pane_tree
        .session_id(first_pane)
        .unwrap()
        .unwrap();
    ports.clear_calls();
    app.ui_port()
        .try_send(UiCommand::ClosePane(first_pane))
        .unwrap();
    let closed = recv_workspace(&events).await;
    assert!(closed.tabs.is_empty());
    assert_eq!(ports.calls(), ["session.shutdown"]);
    assert!(!ports.is_session_live(first_session));
    assert_eq!(ports.live_session_count(), 0);
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn close_tab_shuts_every_contained_session_before_removing_tab() {
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
            axis: SplitAxis::Vertical,
        })
        .unwrap();
    let split = recv_workspace(&events).await;
    let sessions = split.tabs[0].pane_tree.session_ids();
    let tab = split.tabs[0].id;
    ports.clear_calls();

    app.ui_port().try_send(UiCommand::CloseTab(tab)).unwrap();
    let closed = recv_workspace(&events).await;
    assert!(closed.tabs.is_empty());
    assert_eq!(ports.calls(), ["session.shutdown", "session.shutdown"]);
    assert!(
        sessions
            .iter()
            .all(|session| !ports.is_session_live(*session))
    );
    assert_eq!(ports.live_session_count(), 0);
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn close_pane_shutdown_failure_preserves_the_live_leaf_until_retry() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let first = app.initial_view_model().workspace.tabs[0].active_pane;
    app.ui_port()
        .try_send(UiCommand::Split {
            pane: first,
            axis: SplitAxis::Horizontal,
        })
        .unwrap();
    let split = recv_workspace(&events).await;
    let pane = split.tabs[0].active_pane;
    let session = split.tabs[0].pane_tree.session_id(pane).unwrap().unwrap();
    ports.send_frame(session, frame(41));
    recv_matching(&events, |event| {
        matches!(event, AppEvent::Session { session: candidate, event: SessionUiEvent::Frame(frame) } if *candidate == session && frame.generation == 41)
    })
    .await;
    let before = app.view_model();
    ports.fail_shutdown_for(session, SessionFailure::Timeout);
    ports.clear_calls();

    app.ui_port().try_send(UiCommand::ClosePane(pane)).unwrap();
    let failure = recv_failure(&events).await;
    assert_eq!(failure.context, "session operation failed");
    assert_single_failure(&events).await;
    let retained = app.view_model();
    assert_eq!(retained.workspace, before.workspace);
    assert_eq!(
        retained.pane_launches.get(&pane),
        Some(&PaneLaunchTarget::Local)
    );
    assert_eq!(retained.latest_frames.get(&session).unwrap().generation, 41);
    assert!(ports.is_session_live(session));
    assert_eq!(ports.calls(), ["session.shutdown"]);

    ports.clear_shutdown_failure(session);
    app.ui_port().try_send(UiCommand::ClosePane(pane)).unwrap();
    let collapsed = recv_workspace(&events).await;
    assert_eq!(collapsed.tabs[0].pane_tree.pane_ids(), [first]);
    assert!(!ports.is_session_live(session));
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn retry_shutdown_failure_keeps_target_error_and_actor_then_recovers() {
    let bootstrap = bootstrap_state();
    let ports = RecordingPorts::new(&bootstrap);
    let app = ApplicationService::start(ports.dependencies(), bootstrap)
        .await
        .unwrap();
    let events = app.event_receiver();
    let tab = &app.initial_view_model().workspace.tabs[0];
    let pane = tab.active_pane;
    let session = tab.pane_tree.session_id(pane).unwrap().unwrap();
    ports.send_session_event(session, SessionUiEvent::Failed(SessionFailure::Network));
    recv_matching(&events, |event| {
        matches!(event, AppEvent::Session { session: candidate, event: SessionUiEvent::Failed(_) } if *candidate == session)
    })
    .await;
    let before = app.view_model();
    ports.fail_shutdown_for(session, SessionFailure::Network);
    ports.clear_calls();

    app.ui_port().try_send(UiCommand::RetryPane(pane)).unwrap();
    let failure = recv_failure(&events).await;
    assert_eq!(failure.context, "session operation failed");
    assert_single_failure(&events).await;
    let retained = app.view_model();
    assert_eq!(retained.workspace, before.workspace);
    assert_eq!(retained.pane_launches, before.pane_launches);
    assert_eq!(retained.error_panes, before.error_panes);
    assert_eq!(retained.session_states, before.session_states);
    assert!(ports.is_session_live(session));
    assert_eq!(ports.calls(), ["session.shutdown"]);

    ports.clear_shutdown_failure(session);
    app.ui_port().try_send(UiCommand::RetryPane(pane)).unwrap();
    let retried = recv_workspace(&events).await;
    let fresh = retried.tabs[0].pane_tree.session_id(pane).unwrap().unwrap();
    assert_ne!(fresh, session);
    assert!(!ports.is_session_live(session));
    assert!(ports.is_session_live(fresh));
    app.shutdown().await.unwrap();
}

#[tokio::test]
async fn close_tab_late_shutdown_failure_marks_stopped_panes_without_collapsing() {
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
            axis: SplitAxis::Vertical,
        })
        .unwrap();
    let split = recv_workspace(&events).await;
    let second_pane = split.tabs[0].active_pane;
    let sessions = split.tabs[0].pane_tree.session_ids();
    let first_session = split.tabs[0]
        .pane_tree
        .session_id(first_pane)
        .unwrap()
        .unwrap();
    let second_session = split.tabs[0]
        .pane_tree
        .session_id(second_pane)
        .unwrap()
        .unwrap();
    assert_eq!(sessions, [first_session, second_session]);
    ports.send_frame(first_session, frame(51));
    recv_matching(&events, |event| {
        matches!(event, AppEvent::Session { session, event: SessionUiEvent::Frame(frame) } if *session == first_session && frame.generation == 51)
    })
    .await;
    ports.send_frame(second_session, frame(52));
    recv_matching(&events, |event| {
        matches!(event, AppEvent::Session { session, event: SessionUiEvent::Frame(frame) } if *session == second_session && frame.generation == 52)
    })
    .await;
    let before = app.view_model();
    ports.fail_shutdown_for(second_session, SessionFailure::Timeout);
    ports.clear_calls();

    app.ui_port()
        .try_send(UiCommand::CloseTab(split.tabs[0].id))
        .unwrap();
    let failure = recv_failure(&events).await;
    assert_eq!(failure.context, "session operation failed");
    assert_single_failure(&events).await;
    let retained = app.view_model();
    assert_eq!(retained.workspace, before.workspace);
    assert_eq!(retained.pane_launches, before.pane_launches);
    assert_eq!(
        retained
            .latest_frames
            .get(&first_session)
            .unwrap()
            .generation,
        51
    );
    assert_eq!(
        retained
            .latest_frames
            .get(&second_session)
            .unwrap()
            .generation,
        52
    );
    assert_eq!(
        retained.session_states.get(&first_session),
        Some(&SessionState::Exited)
    );
    assert_eq!(
        retained.session_states.get(&second_session),
        before.session_states.get(&second_session)
    );
    assert!(!ports.is_session_live(first_session));
    assert!(ports.is_session_live(second_session));

    ports.clear_shutdown_failure(second_session);
    app.ui_port()
        .try_send(UiCommand::CloseTab(split.tabs[0].id))
        .unwrap();
    let closed = recv_workspace(&events).await;
    assert!(closed.tabs.is_empty());
    assert_eq!(ports.live_session_count(), 0);
    app.shutdown().await.unwrap();
}

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
        rows: Arc::from([]),
        cursor: None,
        title: "stale".into(),
        display_modes: Default::default(),
        alternate_screen: false,
        mouse_reporting: false,
    })
}

async fn recv_workspace(events: &async_channel::Receiver<AppEvent>) -> rshell_core::WorkspaceState {
    let event = recv_matching(events, |event| {
        matches!(event, AppEvent::WorkspaceChanged(_))
    })
    .await;
    let AppEvent::WorkspaceChanged(workspace) = event else {
        unreachable!()
    };
    workspace
}

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

async fn recv_failure(events: &async_channel::Receiver<AppEvent>) -> rshell_core::AppFailure {
    let event = recv_matching(events, |event| {
        matches!(event, AppEvent::OperationFailed(_))
    })
    .await;
    let AppEvent::OperationFailed(failure) = event else {
        unreachable!()
    };
    failure
}

async fn assert_single_failure(events: &async_channel::Receiver<AppEvent>) {
    let second = timeout(Duration::from_millis(25), async {
        loop {
            let event = events.recv().await.unwrap();
            if matches!(event, AppEvent::OperationFailed(_)) {
                return;
            }
        }
    })
    .await;
    assert!(second.is_err(), "shutdown failure must emit exactly once");
}

#[test]
fn all_terminal_outcomes_remain_explicit() {
    assert_eq!(
        [
            SessionState::Created,
            SessionState::Connecting,
            SessionState::AwaitingHostKey,
            SessionState::AwaitingAuthentication,
            SessionState::Connected,
            SessionState::Reconnecting,
            SessionState::Closing,
            SessionState::Exited,
            SessionState::Failed,
            SessionState::Crashed,
        ]
        .len(),
        10
    );
}
