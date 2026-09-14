use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use rshell_core::{
    AuthenticationKind, ConnectionProfile, PaneId, SessionPort, SessionState, SessionUiCommand,
    SessionUiEvent, TerminalProfile, TerminalSize, TransportKind,
};
use rshell_session::{
    AuthPlan, InteractionBroker, KnownHostsVerifier, SessionManager, SessionTransport,
    TransportCapabilities, TransportError, TransportEvent, TransportFactory, TransportRequest,
    ports::SessionPortAdapter,
};

struct RecordingFactory {
    creates: Arc<AtomicUsize>,
}

impl TransportFactory for RecordingFactory {
    fn create(
        &self,
        _request: &TransportRequest,
    ) -> Result<Box<dyn SessionTransport>, TransportError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(PendingTransport))
    }
}

struct PendingTransport;

#[async_trait]
impl SessionTransport for PendingTransport {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities::default()
    }

    async fn connect(
        &mut self,
        _request: &TransportRequest,
        _interactions: InteractionBroker,
    ) -> Result<(), TransportError> {
        Ok(())
    }

    async fn next_event(&mut self) -> Result<TransportEvent, TransportError> {
        pending().await
    }

    async fn write(&mut self, _bytes: &[u8]) -> Result<(), TransportError> {
        Ok(())
    }

    async fn resize(&mut self, _size: TerminalSize) -> Result<(), TransportError> {
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

#[tokio::test]
async fn session_port_adapter_exposes_bounded_binding_and_routes_commands() {
    let creates = Arc::new(AtomicUsize::new(0));
    let manager = Arc::new(SessionManager::new(Arc::new(RecordingFactory {
        creates: Arc::clone(&creates),
    })));
    let known_hosts = tempfile::tempdir().unwrap().path().join("known_hosts");
    let adapter = SessionPortAdapter::new(manager, KnownHostsVerifier::new(known_hosts));
    let terminal = TerminalProfile::default()
        .settings
        .resolve(&Default::default());

    let binding = adapter.launch_local(PaneId::new(), terminal).await.unwrap();
    let event = binding.events.recv().await.unwrap();
    assert!(matches!(
        event,
        SessionUiEvent::State(SessionState::Created | SessionState::Connecting)
    ));
    adapter
        .command(binding.id, SessionUiCommand::Interrupt)
        .await
        .unwrap();
    adapter
        .command(binding.id, SessionUiCommand::ResetDisplay)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if matches!(
                binding.events.recv().await,
                Ok(SessionUiEvent::RecoveryChanged(None))
            ) {
                return;
            }
        }
    })
    .await
    .expect("reset recovery event must cross the port boundary");
    adapter
        .command(binding.id, SessionUiCommand::Reconnect)
        .await
        .unwrap();
    adapter.shutdown_all().await.unwrap();
    assert!(creates.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn single_shutdown_completes_before_a_fresh_actor_is_launched() {
    let creates = Arc::new(AtomicUsize::new(0));
    let manager = Arc::new(SessionManager::new(Arc::new(RecordingFactory {
        creates: Arc::clone(&creates),
    })));
    let known_hosts = tempfile::tempdir().unwrap().path().join("known_hosts");
    let adapter =
        SessionPortAdapter::new(Arc::clone(&manager), KnownHostsVerifier::new(known_hosts));
    let terminal = TerminalProfile::default()
        .settings
        .resolve(&Default::default());
    let old = adapter
        .launch_local(PaneId::new(), terminal.clone())
        .await
        .unwrap();

    adapter.shutdown(old.id).await.unwrap();
    assert_eq!(manager.active_session_count(), 0);
    let fresh = adapter.launch_local(PaneId::new(), terminal).await.unwrap();

    assert_ne!(old.id, fresh.id);
    assert_eq!(manager.active_session_count(), 1);
    adapter.shutdown_all().await.unwrap();
    assert_eq!(manager.active_session_count(), 0);
    assert_eq!(creates.load(Ordering::SeqCst), 2);
}

#[test]
fn native_auth_plans_consume_application_secret_without_vault_access() {
    let mut password = ConnectionProfile::new("password", "example.test");
    password.transport = TransportKind::NativeSsh;
    password.authentication = AuthenticationKind::Password;
    let plan = AuthPlan::from_secret(
        &password,
        Some(secrecy::SecretString::from("adapter-secret")),
    )
    .unwrap();
    assert_eq!(plan.kind(), AuthenticationKind::Password);
    assert!(!format!("{plan:?}").contains("adapter-secret"));

    let mut agent = password;
    agent.authentication = AuthenticationKind::Agent;
    assert_eq!(
        AuthPlan::from_secret(&agent, None).unwrap().kind(),
        AuthenticationKind::Agent
    );

    let mut keyboard = agent;
    keyboard.authentication = AuthenticationKind::KeyboardInteractive;
    assert_eq!(
        AuthPlan::from_secret(&keyboard, None).unwrap().kind(),
        AuthenticationKind::KeyboardInteractive
    );
}
