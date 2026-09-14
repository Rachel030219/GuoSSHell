use rshell_core::{ExitStatus, SessionFailure, SessionState};

use crate::{
    SessionEvent, SessionTransport, TransportEvent,
    actor::{ActorControl, SessionActor},
    display_recovery::RecoveryTransition,
};

impl SessionActor {
    pub(crate) async fn handle_transport_event(
        &mut self,
        transport: &mut dyn SessionTransport,
        result: Result<TransportEvent, crate::TransportError>,
    ) -> ActorControl {
        match result {
            Ok(TransportEvent::Connected) => {
                if self.lifecycle.state() == SessionState::Connected
                    || self.transition(SessionState::Connected)
                {
                    ActorControl::Continue
                } else {
                    ActorControl::IllegalTransition
                }
            }
            Ok(TransportEvent::AwaitingHostKey) => {
                if self.transition(SessionState::AwaitingHostKey) {
                    ActorControl::Continue
                } else {
                    ActorControl::IllegalTransition
                }
            }
            Ok(TransportEvent::AwaitingAuthentication) => {
                if self.transition(SessionState::AwaitingAuthentication) {
                    ActorControl::Continue
                } else {
                    ActorControl::IllegalTransition
                }
            }
            Ok(TransportEvent::Output(bytes)) => {
                let delta = match self.engine.advance(&bytes) {
                    Ok(delta) => delta,
                    Err(_) => return ActorControl::Failure(SessionFailure::Platform),
                };
                if !delta.outbound.is_empty()
                    && let Err(error) = transport.write(&delta.outbound).await
                {
                    return ActorControl::Failure(error.failure());
                }
                if delta.dirty {
                    self.presentation.on_output(self.engine.viewport_bounds());
                    self.frame_clock.mark_dirty();
                }
                ActorControl::Continue
            }
            Ok(TransportEvent::Eof) => ActorControl::Exit(ExitStatus {
                code: None,
                success: true,
            }),
            Ok(TransportEvent::Exit(status)) => ActorControl::Exit(status),
            Ok(TransportEvent::Failure(error)) | Err(error) => {
                ActorControl::Failure(error.failure())
            }
        }
    }

    pub(crate) fn publish_frame(&mut self) -> Result<(), crate::EngineError> {
        let viewport = self.presentation.viewport(self.engine.viewport_bounds());
        let mut frame = self
            .engine
            .render(viewport, self.presentation.selection())?;
        std::sync::Arc::make_mut(&mut frame).generation = self.presentation.next_generation()?;
        let recovery = self.recovery.observe(&frame);
        let first_frame = self.frames.borrow().is_none();
        self.frames.send_replace(Some(frame.clone()));
        if first_frame {
            let _ = self.events.send(SessionEvent::FrameReady(frame));
        }
        if let RecoveryTransition::Changed(notice) = recovery {
            let _ = self.events.send(SessionEvent::RecoveryChanged(notice));
        }
        self.frame_clock.published();
        Ok(())
    }
}
