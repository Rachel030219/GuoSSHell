use rshell_core::{DisplayRecoveryNotice, RenderFrame, TerminalDisplayModes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptionObservation {
    pub generation: u64,
    pub modes: TerminalDisplayModes,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayRecoveryTracker {
    pending: Option<InterruptionObservation>,
    published: Option<DisplayRecoveryNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryTransition {
    Unchanged,
    Changed(Option<DisplayRecoveryNotice>),
}

impl DisplayRecoveryTracker {
    pub fn record(&mut self, generation: u64, modes: TerminalDisplayModes) {
        self.pending = Some(InterruptionObservation { generation, modes });
    }

    pub fn observe(&mut self, frame: &RenderFrame) -> RecoveryTransition {
        match self.pending {
            Some(observation) if frame.generation <= observation.generation => {
                RecoveryTransition::Unchanged
            }
            Some(_) if !frame.display_modes.has_residue() => {
                self.pending = None;
                self.clear_published_for(frame.generation)
            }
            Some(observation) => {
                self.pending = None;
                let notice = DisplayRecoveryNotice {
                    interrupted_generation: observation.generation,
                    observed_generation: frame.generation,
                    modes: observation.modes,
                };
                if self.published == Some(notice) {
                    RecoveryTransition::Unchanged
                } else {
                    self.published = Some(notice);
                    RecoveryTransition::Changed(Some(notice))
                }
            }
            None if !frame.display_modes.has_residue() => {
                self.clear_published_for(frame.generation)
            }
            None => RecoveryTransition::Unchanged,
        }
    }

    pub fn clear(&mut self) -> bool {
        let changed = self.pending.is_some() || self.published.is_some();
        self.pending = None;
        self.published = None;
        changed
    }

    fn clear_published_for(&mut self, generation: u64) -> RecoveryTransition {
        if self
            .published
            .is_some_and(|notice| generation > notice.observed_generation)
        {
            self.published = None;
            RecoveryTransition::Changed(None)
        } else {
            RecoveryTransition::Unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rshell_core::{RenderFrame, TerminalSize};

    use super::*;

    #[test]
    fn same_generation_does_not_publish() {
        let mut tracker = DisplayRecoveryTracker::default();
        let modes = dirty_modes();
        tracker.record(7, modes);

        assert_eq!(
            tracker.observe(&frame(7, modes)),
            RecoveryTransition::Unchanged
        );
        assert_eq!(
            tracker.pending,
            Some(InterruptionObservation {
                generation: 7,
                modes,
            })
        );
    }

    #[test]
    fn newer_dirty_frame_publishes_recorded_interruption_modes_once() {
        let mut tracker = DisplayRecoveryTracker::default();
        let modes = dirty_modes();
        tracker.record(7, modes);

        assert_eq!(
            tracker.observe(&frame(8, modes)),
            RecoveryTransition::Changed(Some(DisplayRecoveryNotice {
                interrupted_generation: 7,
                observed_generation: 8,
                modes,
            }))
        );
    }

    #[test]
    fn later_dirty_frames_do_not_duplicate_a_published_notice() {
        let mut tracker = DisplayRecoveryTracker::default();
        let modes = dirty_modes();
        tracker.record(7, modes);
        let _ = tracker.observe(&frame(8, modes));

        assert_eq!(
            tracker.observe(&frame(9, modes)),
            RecoveryTransition::Unchanged
        );
    }

    #[test]
    fn newer_clean_frame_without_notice_only_clears_pending() {
        let mut tracker = DisplayRecoveryTracker::default();
        tracker.record(7, dirty_modes());

        assert_eq!(
            tracker.observe(&frame(8, TerminalDisplayModes::default())),
            RecoveryTransition::Unchanged
        );
        assert_eq!(tracker.pending, None);
        assert_eq!(tracker.published, None);
    }

    #[test]
    fn newer_clean_frame_clears_a_published_notice() {
        let mut tracker = DisplayRecoveryTracker::default();
        let modes = dirty_modes();
        tracker.record(7, modes);
        let _ = tracker.observe(&frame(8, modes));

        assert_eq!(
            tracker.observe(&frame(8, TerminalDisplayModes::default())),
            RecoveryTransition::Unchanged
        );

        assert_eq!(
            tracker.observe(&frame(9, TerminalDisplayModes::default())),
            RecoveryTransition::Changed(None)
        );
        assert_eq!(tracker.pending, None);
        assert_eq!(tracker.published, None);
    }

    fn dirty_modes() -> TerminalDisplayModes {
        TerminalDisplayModes {
            alternate_screen: true,
            enhanced_keyboard: true,
            mouse_reporting: true,
            application_cursor: true,
            cursor_hidden: true,
            stale_title: true,
        }
    }

    fn frame(generation: u64, display_modes: TerminalDisplayModes) -> RenderFrame {
        RenderFrame {
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
            title: String::new(),
            display_modes,
            alternate_screen: display_modes.alternate_screen,
            mouse_reporting: display_modes.mouse_reporting,
        }
    }
}
