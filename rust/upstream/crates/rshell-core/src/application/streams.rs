use std::fmt;

use tokio::sync::watch;

use crate::{AppEvent, AppViewModel};

#[derive(Clone)]
pub struct AppEventStream {
    receiver: async_channel::Receiver<AppEvent>,
}

impl AppEventStream {
    pub(super) fn new(receiver: async_channel::Receiver<AppEvent>) -> Self {
        Self { receiver }
    }

    pub async fn recv(&self) -> Option<AppEvent> {
        self.receiver.recv().await.ok()
    }
}

impl fmt::Debug for AppEventStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AppEventStream")
    }
}

#[derive(Clone)]
pub struct LatestViewStream {
    receiver: watch::Receiver<AppViewModel>,
}

impl LatestViewStream {
    pub(super) fn new(receiver: watch::Receiver<AppViewModel>) -> Self {
        Self { receiver }
    }

    pub fn latest(&self) -> AppViewModel {
        self.receiver.borrow().clone()
    }

    pub async fn changed(&mut self) -> Option<AppViewModel> {
        self.receiver.changed().await.ok()?;
        Some(self.latest())
    }
}

impl fmt::Debug for LatestViewStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LatestViewStream")
            .field("revision", &self.receiver.borrow().revision)
            .finish()
    }
}
