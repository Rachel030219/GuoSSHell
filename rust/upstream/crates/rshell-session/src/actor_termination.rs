use rshell_core::{ExitStatus, SessionFailure, SessionState};

use crate::{EngineError, SessionError, SessionEvent, SessionTransport, actor::SessionActor};

impl SessionActor {
    pub(crate) fn prepare_final_presentation(&mut self) -> Result<u64, EngineError> {
        self.engine.recover_display()?;
        self.recovery.clear();
        self.presentation
            .on_display_recovery(self.engine.viewport_bounds());
        self.frame_clock.mark_dirty();
        self.publish_frame()?;
        let _ = self.events.send(SessionEvent::RecoveryChanged(None));
        Ok(self.presentation.generation())
    }

    pub(crate) async fn shutdown(
        &mut self,
        transport: &mut dyn SessionTransport,
    ) -> Result<(), SessionError> {
        let presentation = self.prepare_final_presentation();
        let closing = presentation.is_ok() && self.transition(SessionState::Closing);
        let shutdown = transport.shutdown().await;
        if presentation.is_err() {
            self.fail_after_prepared(SessionFailure::Platform);
            return shutdown.map_err(|error| SessionError::TransportShutdown(error.failure()));
        }
        if !closing {
            self.fail_after_prepared(SessionFailure::Crashed);
            return shutdown.map_err(|error| SessionError::TransportShutdown(error.failure()));
        }
        match shutdown {
            Ok(()) => self.lifecycle.exit(
                ExitStatus {
                    code: None,
                    success: true,
                },
                &self.events,
            ),
            Err(error) => {
                self.lifecycle.fail(error.failure(), &self.events);
                return Err(SessionError::TransportShutdown(error.failure()));
            }
        }
        Ok(())
    }

    pub(crate) async fn exit(
        &mut self,
        transport: &mut dyn SessionTransport,
        status: ExitStatus,
    ) -> Result<(), SessionError> {
        let shutdown = transport.shutdown().await;
        if !self.prepare_or_fail() {
            return shutdown.map_err(|error| SessionError::TransportShutdown(error.failure()));
        }
        match shutdown {
            Ok(()) => self.lifecycle.exit(status, &self.events),
            Err(error) => {
                self.lifecycle.fail(error.failure(), &self.events);
                return Err(SessionError::TransportShutdown(error.failure()));
            }
        }
        Ok(())
    }

    pub(crate) async fn fail(
        &mut self,
        transport: &mut dyn SessionTransport,
        failure: SessionFailure,
    ) -> Result<(), SessionError> {
        let shutdown = transport.shutdown().await;
        if self.prepare_or_fail() {
            self.lifecycle.fail(failure, &self.events);
        }
        shutdown.map_err(|error| SessionError::TransportShutdown(error.failure()))
    }

    pub(crate) async fn illegal(
        &mut self,
        transport: &mut dyn SessionTransport,
    ) -> Result<(), SessionError> {
        let shutdown = transport.shutdown().await;
        if self.prepare_or_fail() {
            self.lifecycle.illegal(&self.events);
        }
        shutdown.map_err(|error| SessionError::TransportShutdown(error.failure()))
    }

    pub(crate) fn fail_after_prepared(&mut self, failure: SessionFailure) {
        self.lifecycle.fail(failure, &self.events);
    }

    fn prepare_or_fail(&mut self) -> bool {
        if self.prepare_final_presentation().is_ok() {
            true
        } else {
            self.lifecycle.fail(SessionFailure::Platform, &self.events);
            false
        }
    }
}
