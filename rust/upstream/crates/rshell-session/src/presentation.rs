use rshell_core::{SelectionRange, TerminalSize, Viewport};

use crate::EngineError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportBounds {
    pub first_stable_row: i64,
    pub bottom_top_stable_row: i64,
}

impl ViewportBounds {
    pub fn clamp_top(self, top_stable_row: i64) -> i64 {
        top_stable_row
            .max(self.first_stable_row)
            .min(self.bottom_top_stable_row.max(self.first_stable_row))
    }

    fn bottom_top(self) -> i64 {
        self.bottom_top_stable_row.max(self.first_stable_row)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationPolicy {
    pub scroll_on_output: bool,
    pub scroll_on_keypress: bool,
}

impl Default for PresentationPolicy {
    fn default() -> Self {
        Self {
            scroll_on_output: true,
            scroll_on_keypress: false,
        }
    }
}

pub(crate) struct PresentationState {
    viewport: Viewport,
    selection: Option<SelectionRange>,
    follow_bottom: bool,
    generation: u64,
    policy: PresentationPolicy,
}

impl PresentationState {
    pub(crate) fn new(size: TerminalSize, policy: PresentationPolicy) -> Self {
        Self {
            viewport: Viewport {
                top_stable_row: i64::MAX,
                rows: size.rows,
            },
            selection: None,
            follow_bottom: true,
            generation: 0,
            policy,
        }
    }

    pub(crate) fn viewport(&mut self, bounds: ViewportBounds) -> Viewport {
        self.clamp_to_bounds(bounds);
        self.viewport
    }

    pub(crate) fn selection(&self) -> Option<SelectionRange> {
        self.selection
    }

    pub(crate) fn set_selection(&mut self, selection: SelectionRange) {
        self.selection = Some(selection);
    }

    pub(crate) fn on_output(&mut self, bounds: ViewportBounds) {
        if self.follow_bottom || self.policy.scroll_on_output {
            self.follow_bottom = true;
            self.viewport.top_stable_row = bounds.bottom_top();
        } else {
            self.clamp_to_bounds(bounds);
        }
    }

    pub(crate) fn on_scroll(&mut self, delta_rows: i32, bounds: ViewportBounds) {
        let top = if self.follow_bottom {
            bounds.bottom_top()
        } else {
            bounds.clamp_top(self.viewport.top_stable_row)
        };
        self.viewport.top_stable_row = bounds.clamp_top(top.saturating_add(i64::from(delta_rows)));
        self.follow_bottom = self.viewport.top_stable_row == bounds.bottom_top();
    }

    pub(crate) fn on_resize(&mut self, size: TerminalSize, bounds: ViewportBounds) {
        self.viewport.rows = size.rows;
        self.clamp_to_bounds(bounds);
    }

    pub(crate) fn scroll_on_keypress(&self) -> bool {
        self.policy.scroll_on_keypress
    }

    pub(crate) fn on_input(&mut self, bounds: ViewportBounds) {
        self.follow_bottom = true;
        self.viewport.top_stable_row = bounds.bottom_top();
    }

    pub(crate) fn on_clear_scrollback(&mut self, bounds: ViewportBounds) {
        self.selection = None;
        self.clamp_to_bounds(bounds);
    }

    pub(crate) fn on_display_recovery(&mut self, bounds: ViewportBounds) {
        self.selection = None;
        self.follow_bottom = true;
        self.viewport.top_stable_row = bounds.bottom_top();
    }

    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn next_generation(&mut self) -> Result<u64, EngineError> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(EngineError::PresentationGenerationExhausted)?;
        Ok(self.generation)
    }

    fn clamp_to_bounds(&mut self, bounds: ViewportBounds) {
        self.viewport.top_stable_row = if self.follow_bottom {
            bounds.bottom_top()
        } else {
            bounds.clamp_top(self.viewport.top_stable_row)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: ViewportBounds = ViewportBounds {
        first_stable_row: 10,
        bottom_top_stable_row: 20,
    };

    fn state(policy: PresentationPolicy) -> PresentationState {
        PresentationState::new(
            TerminalSize {
                cols: 80,
                rows: 4,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 96,
            },
            policy,
        )
    }

    #[test]
    fn scroll_clamps_and_reenters_follow_bottom() {
        let mut state = state(PresentationPolicy::default());
        assert_eq!(state.viewport(BOUNDS).top_stable_row, 20);
        state.on_scroll(-3, BOUNDS);
        assert_eq!(state.viewport(BOUNDS).top_stable_row, 17);
        state.on_scroll(i32::MAX, BOUNDS);
        assert_eq!(state.viewport(BOUNDS).top_stable_row, 20);
    }

    #[test]
    fn output_policy_preserves_or_snaps_history() {
        let updated_bounds = ViewportBounds {
            bottom_top_stable_row: 21,
            ..BOUNDS
        };
        let mut preserved = state(PresentationPolicy {
            scroll_on_output: false,
            scroll_on_keypress: false,
        });
        preserved.on_scroll(-3, BOUNDS);
        preserved.on_output(updated_bounds);
        assert_eq!(preserved.viewport(updated_bounds).top_stable_row, 17);

        let mut snapped = state(PresentationPolicy::default());
        snapped.on_scroll(-3, BOUNDS);
        snapped.on_output(updated_bounds);
        assert_eq!(snapped.viewport(updated_bounds).top_stable_row, 21);
    }
}
