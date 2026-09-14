use rshell_core::{ExitStatus, SessionFailure, SessionState};
use tokio::sync::broadcast;

use crate::SessionEvent;

pub(crate) struct Lifecycle {
    state: SessionState,
}

impl Lifecycle {
    pub(crate) fn new(events: &broadcast::Sender<SessionEvent>) -> Self {
        let _ = events.send(SessionEvent::StateChanged(SessionState::Created));
        Self {
            state: SessionState::Created,
        }
    }

    pub(crate) fn transition(
        &mut self,
        next: SessionState,
        events: &broadcast::Sender<SessionEvent>,
    ) -> bool {
        if !allowed(self.state, next) {
            return false;
        }
        self.state = next;
        let _ = events.send(SessionEvent::StateChanged(next));
        true
    }

    pub(crate) const fn state(&self) -> SessionState {
        self.state
    }

    pub(crate) fn fail(
        &mut self,
        failure: SessionFailure,
        events: &broadcast::Sender<SessionEvent>,
    ) {
        if self.state != SessionState::Failed && self.transition(SessionState::Failed, events) {
            let _ = events.send(SessionEvent::Failed(failure));
        }
    }

    pub(crate) fn illegal(&mut self, events: &broadcast::Sender<SessionEvent>) {
        self.fail(SessionFailure::Crashed, events);
    }

    pub(crate) fn exit(&mut self, status: ExitStatus, events: &broadcast::Sender<SessionEvent>) {
        if self.transition(SessionState::Exited, events) {
            let _ = events.send(SessionEvent::Exited(status));
        }
    }
}

fn allowed(current: SessionState, next: SessionState) -> bool {
    use SessionState::{
        AwaitingAuthentication, AwaitingHostKey, Closing, Connected, Connecting, Created, Exited,
        Failed, Reconnecting,
    };

    matches!(
        (current, next),
        (Created, Connecting | Closing | Failed)
            | (
                Connecting,
                AwaitingHostKey | AwaitingAuthentication | Connected | Closing | Failed
            )
            | (
                AwaitingHostKey,
                AwaitingAuthentication | Connected | Reconnecting | Closing | Exited | Failed
            )
            | (
                AwaitingAuthentication,
                AwaitingHostKey | Connected | Reconnecting | Closing | Exited | Failed
            )
            | (
                Connected,
                AwaitingHostKey | AwaitingAuthentication | Reconnecting | Closing | Exited | Failed
            )
            | (Reconnecting, Connecting | Closing | Failed)
            | (Closing, Exited | Failed)
    )
}
