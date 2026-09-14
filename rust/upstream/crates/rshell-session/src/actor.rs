use std::{collections::VecDeque, sync::Arc};

use rshell_core::{SessionFailure, SessionId, SessionState};
use tokio::sync::{broadcast, mpsc, watch};

use crate::{
    COMMAND_CAPACITY, InteractionBroker, SessionCommand, SessionEvent, SessionTransport,
    TerminalEngine, TransportFactory, TransportRequest,
    actor_process::{clear_stopped_child_process, record_child_process},
    display_recovery::DisplayRecoveryTracker,
    frame_clock::FrameClock,
    lifecycle::Lifecycle,
    manager::ChildProcessRegistry,
    presentation::{PresentationPolicy, PresentationState},
    transport::{interaction_channel, interaction_state},
};

pub(crate) struct ActorChannels {
    pub(crate) commands: mpsc::Receiver<SessionCommand>,
    pub(crate) events: broadcast::Sender<SessionEvent>,
    pub(crate) frames: watch::Sender<Option<Arc<rshell_core::RenderFrame>>>,
}

pub(crate) struct SessionActor {
    pub(crate) id: SessionId,
    pub(crate) factory: Arc<dyn TransportFactory>,
    pub(crate) request: TransportRequest,
    pub(crate) engine: Box<dyn TerminalEngine>,
    pub(crate) commands: mpsc::Receiver<SessionCommand>,
    pub(crate) events: broadcast::Sender<SessionEvent>,
    pub(crate) frames: watch::Sender<Option<Arc<rshell_core::RenderFrame>>>,
    pub(crate) interactions: InteractionBroker,
    pub(crate) interaction_rx:
        mpsc::Receiver<(rshell_core::InteractionId, rshell_core::InteractionRequest)>,
    pub(crate) deferred: VecDeque<SessionCommand>,
    pub(crate) lifecycle: Lifecycle,
    pub(crate) presentation: PresentationState,
    pub(crate) recovery: DisplayRecoveryTracker,
    pub(crate) frame_clock: FrameClock,
    pub(crate) child_processes: ChildProcessRegistry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActorControl {
    Continue,
    Reconnect,
    Shutdown,
    Exit(rshell_core::ExitStatus),
    Failure(SessionFailure),
    IllegalTransition,
}

enum ConnectOutcome {
    Connected,
    Shutdown,
    Failure(SessionFailure),
    IllegalTransition,
}

impl SessionActor {
    pub(crate) fn new(
        id: SessionId,
        factory: Arc<dyn TransportFactory>,
        request: TransportRequest,
        engine: Box<dyn TerminalEngine>,
        presentation_policy: PresentationPolicy,
        channels: ActorChannels,
        child_processes: ChildProcessRegistry,
    ) -> Self {
        let (interactions, interaction_rx) = interaction_channel();
        let presentation = PresentationState::new(request.initial_size(), presentation_policy);
        let lifecycle = Lifecycle::new(&channels.events);
        Self {
            id,
            factory,
            request,
            engine,
            commands: channels.commands,
            events: channels.events,
            frames: channels.frames,
            interactions,
            interaction_rx,
            deferred: VecDeque::new(),
            lifecycle,
            presentation,
            recovery: DisplayRecoveryTracker::default(),
            frame_clock: FrameClock::default(),
            child_processes,
        }
    }

    pub(crate) async fn run(mut self) -> Result<(), crate::SessionError> {
        if !self.transition(SessionState::Connecting) {
            self.lifecycle.illegal(&self.events);
            return Err(crate::SessionError::ActorJoin);
        }
        let mut transport = match self.factory.create(&self.request) {
            Ok(transport) => transport,
            Err(error) => {
                self.lifecycle.fail(error.failure(), &self.events);
                return Ok(());
            }
        };

        loop {
            match self.connect(&mut *transport).await {
                ConnectOutcome::Connected => {
                    record_child_process(
                        self.id,
                        &self.child_processes,
                        transport.child_process_id(),
                    );
                }
                ConnectOutcome::Shutdown => {
                    return self.shutdown(&mut *transport).await;
                }
                ConnectOutcome::Failure(failure) => {
                    return self.fail(&mut *transport, failure).await;
                }
                ConnectOutcome::IllegalTransition => {
                    return self.illegal(&mut *transport).await;
                }
            }

            match self.connected(&mut *transport).await {
                ActorControl::Reconnect => {
                    if self.prepare_final_presentation().is_err() {
                        self.fail_after_prepared(SessionFailure::Platform);
                        let shutdown = transport.shutdown().await;
                        return shutdown.map_err(|error| {
                            crate::SessionError::TransportShutdown(error.failure())
                        });
                    }
                    if !self.transition(SessionState::Reconnecting) {
                        return self.illegal(&mut *transport).await;
                    }
                    if let Err(error) = transport.shutdown().await {
                        self.fail_after_prepared(error.failure());
                        return Err(crate::SessionError::TransportShutdown(error.failure()));
                    }
                    if let Err(error) = clear_stopped_child_process(self.id, &self.child_processes)
                    {
                        self.fail_after_prepared(SessionFailure::Subprocess);
                        return Err(error);
                    }
                    let replacement = match self.factory.create(&self.request) {
                        Ok(replacement) => replacement,
                        Err(error) => {
                            self.fail_after_prepared(error.failure());
                            return Ok(());
                        }
                    };
                    transport = replacement;
                    if !self.transition(SessionState::Connecting) {
                        return self.illegal(&mut *transport).await;
                    }
                }
                ActorControl::Shutdown => {
                    return self.shutdown(&mut *transport).await;
                }
                ActorControl::Exit(status) => {
                    return self.exit(&mut *transport, status).await;
                }
                ActorControl::Failure(failure) => {
                    return self.fail(&mut *transport, failure).await;
                }
                ActorControl::IllegalTransition => {
                    return self.illegal(&mut *transport).await;
                }
                ActorControl::Continue => {}
            }
        }
    }

    async fn connect(&mut self, transport: &mut dyn SessionTransport) -> ConnectOutcome {
        let request = self.request.clone();
        let connect = transport.connect(&request, self.interactions.clone());
        tokio::pin!(connect);
        loop {
            tokio::select! {
                biased;
                command = self.commands.recv(), if self.deferred.len() < COMMAND_CAPACITY => match command {
                    Some(SessionCommand::Respond(id, response)) => {
                        if let Err(error) = self.interactions.respond(id, response) {
                            return ConnectOutcome::Failure(error.failure());
                        }
                    }
                    Some(SessionCommand::Shutdown) | None => return ConnectOutcome::Shutdown,
                    Some(command) => self.deferred.push_back(command),
                },
                interaction = self.interaction_rx.recv() => {
                    if let Some((_, request)) = interaction
                        && !self.route_interaction(request)
                    {
                        return ConnectOutcome::IllegalTransition;
                    }
                }
                result = &mut connect => {
                    return match result {
                        Ok(()) if self.transition(SessionState::Connected) => ConnectOutcome::Connected,
                        Ok(()) => ConnectOutcome::IllegalTransition,
                        Err(error) => ConnectOutcome::Failure(error.failure()),
                    };
                }
            }
        }
    }

    async fn connected(&mut self, transport: &mut dyn SessionTransport) -> ActorControl {
        loop {
            if let Some(command) = self.deferred.pop_front() {
                let control = self.handle_command(transport, command).await;
                if control != ActorControl::Continue {
                    return control;
                }
                continue;
            }
            let deadline = self.frame_clock.deadline();
            tokio::select! {
                biased;
                command = self.commands.recv() => {
                    let Some(command) = command else { return ActorControl::Shutdown; };
                    let control = self.handle_command(transport, command).await;
                    if control != ActorControl::Continue { return control; }
                }
                interaction = self.interaction_rx.recv() => {
                    if let Some((_, request)) = interaction
                        && !self.route_interaction(request)
                    {
                        return ActorControl::IllegalTransition;
                    }
                }
                _ = tokio::time::sleep_until(deadline), if self.frame_clock.is_dirty() => {
                    if self.publish_frame().is_err() {
                        return ActorControl::Failure(SessionFailure::Platform);
                    }
                }
                result = transport.next_event() => {
                    let control = self.handle_transport_event(transport, result).await;
                    if control != ActorControl::Continue { return control; }
                }
            }
        }
    }

    pub(crate) fn transition(&mut self, next: SessionState) -> bool {
        self.lifecycle.transition(next, &self.events)
    }

    fn route_interaction(&mut self, request: rshell_core::InteractionRequest) -> bool {
        if !self.transition(interaction_state(&request)) {
            return false;
        }
        let _ = self.events.send(SessionEvent::InteractionRequired(request));
        true
    }
}
