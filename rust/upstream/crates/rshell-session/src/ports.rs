use std::sync::Arc;

use async_trait::async_trait;
use rshell_core::{
    ConnectionProfile, PaneId, ResolvedTerminalProfile, SessionBinding, SessionFailure, SessionId,
    SessionPort, SessionUiCommand, SessionUiEvent, TerminalSize, TransportKind,
};
use secrecy::SecretString;

use crate::{
    AuthPlan, KnownHostsVerifier, LocalLaunch, LocalPtyFactory, NativeFactory, SessionClient,
    SessionCommand, SessionError, SessionEvent, SessionLaunch, SessionManager, SessionTransport,
    SystemOpenSshTransport, TransportError, TransportFactory, TransportRequest,
};

const BRIDGE_CAPACITY: usize = 64;

pub struct SessionPortAdapter {
    manager: Arc<SessionManager>,
    verifier: KnownHostsVerifier,
}

impl SessionPortAdapter {
    pub fn new(manager: Arc<SessionManager>, verifier: KnownHostsVerifier) -> Self {
        Self { manager, verifier }
    }

    pub fn with_local_manager(verifier: KnownHostsVerifier) -> Self {
        Self::new(
            Arc::new(SessionManager::new(LocalPtyFactory::new(
                LocalLaunch::DefaultShell,
            ))),
            verifier,
        )
    }

    pub fn manager(&self) -> &Arc<SessionManager> {
        &self.manager
    }

    fn launch(
        &self,
        request: TransportRequest,
        terminal: &ResolvedTerminalProfile,
        factory: Option<Arc<dyn TransportFactory>>,
    ) -> Result<SessionBinding, SessionFailure> {
        let mut launch = SessionLaunch::with_default_engine(request, terminal)
            .map_err(|_| SessionFailure::Validation)?;
        if let Some(factory) = factory {
            launch = launch.with_factory(factory);
        }
        self.manager
            .launch(launch)
            .map(binding)
            .map_err(map_session_error)
    }
}

#[async_trait]
impl SessionPort for SessionPortAdapter {
    async fn launch_local(
        &self,
        _pane: PaneId,
        terminal: ResolvedTerminalProfile,
    ) -> Result<SessionBinding, SessionFailure> {
        let request = request(&terminal, size(&terminal))?;
        self.launch(request, &terminal, None)
    }

    async fn launch_ssh(
        &self,
        _pane: PaneId,
        profile: ConnectionProfile,
        terminal: ResolvedTerminalProfile,
        initial_size: TerminalSize,
        secret: Option<SecretString>,
    ) -> Result<SessionBinding, SessionFailure> {
        let request = request(&terminal, initial_size)?;
        let factory: Arc<dyn TransportFactory> = match profile.transport {
            TransportKind::SystemOpenSsh => {
                drop(secret);
                Arc::new(SystemFactory { profile })
            }
            TransportKind::NativeSsh => {
                let auth = AuthPlan::from_secret(&profile, secret)
                    .map_err(|_| SessionFailure::Authentication)?;
                Arc::new(NativeFactory::new(profile, auth, self.verifier.clone()))
            }
        };
        self.launch(request, &terminal, Some(factory))
    }

    async fn command(
        &self,
        session: SessionId,
        command: SessionUiCommand,
    ) -> Result<(), SessionFailure> {
        let command = map_command(command);
        self.manager
            .command(session, command)
            .map_err(map_session_error)
    }

    async fn shutdown(&self, session: SessionId) -> Result<(), SessionFailure> {
        self.manager
            .shutdown(session)
            .await
            .map_err(map_session_error)
    }

    async fn shutdown_all(&self) -> Result<(), SessionFailure> {
        self.manager.shutdown_all().await.map_err(map_session_error)
    }
}

struct SystemFactory {
    profile: ConnectionProfile,
}

impl TransportFactory for SystemFactory {
    fn create(
        &self,
        _request: &TransportRequest,
    ) -> Result<Box<dyn SessionTransport>, TransportError> {
        Ok(Box::new(SystemOpenSshTransport::new(self.profile.clone())))
    }
}

fn binding(mut client: SessionClient) -> SessionBinding {
    let (events, receiver) = async_channel::bounded(BRIDGE_CAPACITY);
    tokio::spawn(async move {
        loop {
            match client.events.recv().await {
                Ok(SessionEvent::FrameReady(_)) => continue,
                Ok(event) => {
                    if events.send(map_event(event)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if events
                        .send(SessionUiEvent::Failed(SessionFailure::Backpressure))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    SessionBinding {
        id: client.id,
        events: receiver,
        frames: client.frames,
    }
}

fn map_command(command: SessionUiCommand) -> SessionCommand {
    match command {
        SessionUiCommand::Interrupt => SessionCommand::Interrupt,
        SessionUiCommand::ResetDisplay => SessionCommand::ResetDisplay,
        SessionUiCommand::Input(value) => SessionCommand::Input(value),
        SessionUiCommand::Mouse(value) => SessionCommand::Mouse(value),
        SessionUiCommand::Paste(value) => SessionCommand::Paste(value),
        SessionUiCommand::Resize(value) => SessionCommand::Resize(value),
        SessionUiCommand::Scroll(value) => SessionCommand::Scroll(value),
        SessionUiCommand::Search(value) => SessionCommand::Search(value),
        SessionUiCommand::Select(value) => SessionCommand::Select(value),
        SessionUiCommand::CopySelection => SessionCommand::CopySelection,
        SessionUiCommand::ClearScrollback => SessionCommand::ClearScrollback,
        SessionUiCommand::Respond {
            interaction,
            response,
        } => SessionCommand::Respond(interaction, response),
        SessionUiCommand::Reconnect => SessionCommand::Reconnect,
        SessionUiCommand::Shutdown => SessionCommand::Shutdown,
    }
}

fn map_event(event: SessionEvent) -> SessionUiEvent {
    match event {
        SessionEvent::StateChanged(value) => SessionUiEvent::State(value),
        SessionEvent::FrameReady(value) => SessionUiEvent::Frame(value),
        SessionEvent::RecoveryChanged(value) => SessionUiEvent::RecoveryChanged(value),
        SessionEvent::SearchCompleted(value) => SessionUiEvent::Search(value),
        SessionEvent::CopyReady(value) => SessionUiEvent::Copy(value),
        SessionEvent::InteractionRequired(value) => SessionUiEvent::InteractionRequired(value),
        SessionEvent::Exited(value) => SessionUiEvent::Exited(value),
        SessionEvent::Failed(value) => SessionUiEvent::Failed(value),
        SessionEvent::Crashed(_) => SessionUiEvent::Crashed("session actor crashed".into()),
    }
}

fn request(
    terminal: &ResolvedTerminalProfile,
    size: TerminalSize,
) -> Result<TransportRequest, SessionFailure> {
    TransportRequest::new(size)
        .with_terminal_type(terminal.terminal_type.clone())
        .map_err(|error| error.failure())
}

fn size(terminal: &ResolvedTerminalProfile) -> TerminalSize {
    TerminalSize {
        cols: terminal.cols,
        rows: terminal.rows,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    }
}

fn map_session_error(error: SessionError) -> SessionFailure {
    match error {
        SessionError::Backpressure => SessionFailure::Backpressure,
        SessionError::Closed | SessionError::UnknownSession => SessionFailure::Validation,
        SessionError::RuntimeUnavailable | SessionError::ActorJoin => SessionFailure::Crashed,
        SessionError::TransportShutdown(failure) => failure,
        SessionError::ChildProcessAlive => SessionFailure::Subprocess,
    }
}
