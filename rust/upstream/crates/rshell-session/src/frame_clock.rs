use std::time::Duration;

use tokio::time::Instant;

pub(crate) const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

#[derive(Default)]
pub(crate) struct FrameClock {
    dirty: bool,
    last_published: Option<Instant>,
}

impl FrameClock {
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.last_published
            .map(|instant| instant + FRAME_INTERVAL)
            .unwrap_or_else(Instant::now)
    }

    pub(crate) fn published(&mut self) {
        self.dirty = false;
        self.last_published = Some(Instant::now());
    }
}
