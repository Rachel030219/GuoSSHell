use rshell_core::SessionFailure;
use secrecy::ExposeSecret;

use crate::{
    SessionCommand, SessionEvent, SessionTransport,
    actor::{ActorControl, SessionActor},
};

impl SessionActor {
    pub(crate) async fn handle_command(
        &mut self,
        transport: &mut dyn SessionTransport,
        command: SessionCommand,
    ) -> ActorControl {
        match command {
            SessionCommand::Interrupt => match transport.write(&[0x03]).await {
                Ok(()) => {
                    self.recovery
                        .record(self.presentation.generation(), self.engine.display_modes());
                    ActorControl::Continue
                }
                Err(error) => ActorControl::Failure(error.failure()),
            },
            SessionCommand::ResetDisplay => self.reset_display(),
            SessionCommand::Input(input) => {
                let bytes = match self.engine.encode_input(input) {
                    Ok(bytes) => bytes,
                    Err(_) => return ActorControl::Failure(SessionFailure::Platform),
                };
                match transport.write(&bytes).await {
                    Ok(()) => {
                        if self.presentation.scroll_on_keypress() {
                            self.presentation.on_input(self.engine.viewport_bounds());
                            self.frame_clock.mark_dirty();
                        }
                        ActorControl::Continue
                    }
                    Err(error) => ActorControl::Failure(error.failure()),
                }
            }
            SessionCommand::Mouse(event) => {
                let bytes = match self.engine.encode_mouse(event) {
                    Ok(bytes) => bytes,
                    Err(_) => return ActorControl::Failure(SessionFailure::Platform),
                };
                match transport.write(&bytes).await {
                    Ok(()) => ActorControl::Continue,
                    Err(error) => ActorControl::Failure(error.failure()),
                }
            }
            SessionCommand::Paste(secret) => {
                match transport.write(secret.expose_secret().as_bytes()).await {
                    Ok(()) => ActorControl::Continue,
                    Err(error) => ActorControl::Failure(error.failure()),
                }
            }
            SessionCommand::Resize(size) => {
                if self.engine.resize(size).is_err() {
                    return ActorControl::Failure(SessionFailure::Platform);
                }
                self.presentation
                    .on_resize(size, self.engine.viewport_bounds());
                match transport.resize(size).await {
                    Ok(()) => {
                        self.frame_clock.mark_dirty();
                        ActorControl::Continue
                    }
                    Err(error) => ActorControl::Failure(error.failure()),
                }
            }
            SessionCommand::Scroll(rows) => {
                if self.engine.scroll(rows).is_err() {
                    return ActorControl::Failure(SessionFailure::Platform);
                }
                self.presentation
                    .on_scroll(rows, self.engine.viewport_bounds());
                if rows != 0 {
                    self.frame_clock.mark_dirty();
                }
                ActorControl::Continue
            }
            SessionCommand::Search(query) => match self.engine.search(&query) {
                Ok(matches) => {
                    let _ = self.events.send(SessionEvent::SearchCompleted(matches));
                    ActorControl::Continue
                }
                Err(_) => ActorControl::Failure(SessionFailure::Platform),
            },
            SessionCommand::Select(range) => {
                self.presentation.set_selection(range);
                self.frame_clock.mark_dirty();
                ActorControl::Continue
            }
            SessionCommand::CopySelection => {
                let text = match self.presentation.selection() {
                    Some(range) => match self.engine.selected_text(range) {
                        Ok(text) => text,
                        Err(_) => return ActorControl::Failure(SessionFailure::Platform),
                    },
                    None => String::new(),
                };
                let _ = self.events.send(SessionEvent::CopyReady(text));
                ActorControl::Continue
            }
            SessionCommand::ClearScrollback => self.clear_scrollback(),
            SessionCommand::Respond(id, response) => {
                match self.interactions.respond(id, response) {
                    Ok(()) => ActorControl::Continue,
                    Err(error) => ActorControl::Failure(error.failure()),
                }
            }
            SessionCommand::Reconnect => ActorControl::Reconnect,
            SessionCommand::Shutdown => ActorControl::Shutdown,
        }
    }

    fn clear_scrollback(&mut self) -> ActorControl {
        if self.engine.clear_scrollback().is_err() {
            return ActorControl::Failure(SessionFailure::Platform);
        }
        self.presentation
            .on_clear_scrollback(self.engine.viewport_bounds());
        self.frame_clock.mark_dirty();
        ActorControl::Continue
    }

    fn reset_display(&mut self) -> ActorControl {
        if self.engine.recover_display().is_err() {
            return ActorControl::Failure(SessionFailure::Platform);
        }
        self.recovery.clear();
        self.presentation
            .on_display_recovery(self.engine.viewport_bounds());
        self.frame_clock.mark_dirty();
        if self.publish_frame().is_err() {
            return ActorControl::Failure(SessionFailure::Platform);
        }
        let _ = self.events.send(SessionEvent::RecoveryChanged(None));
        ActorControl::Continue
    }
}
