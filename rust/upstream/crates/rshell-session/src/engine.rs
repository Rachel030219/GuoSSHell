use std::sync::Arc;

use rshell_core::{
    RenderFrame, ResolvedTerminalProfile, SearchMatch, SearchQuery, SelectionRange, TerminalInput,
    TerminalMouseEvent, TerminalSize, Viewport,
    render::{DisplayRecovery, TerminalDisplayModes},
};

use crate::{EngineError, ViewportBounds, alacritty_adapter::AlacrittyAdapter, render, text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineDelta {
    pub outbound: Vec<u8>,
    pub dirty: bool,
}

pub trait TerminalEngine: Send {
    fn display_modes(&self) -> TerminalDisplayModes;
    fn recover_display(&mut self) -> Result<DisplayRecovery, EngineError>;
    fn advance(&mut self, bytes: &[u8]) -> Result<EngineDelta, EngineError>;
    fn resize(&mut self, size: TerminalSize) -> Result<(), EngineError>;
    fn render(
        &mut self,
        viewport: Viewport,
        selection: Option<SelectionRange>,
    ) -> Result<Arc<RenderFrame>, EngineError>;
    fn encode_input(&mut self, input: TerminalInput) -> Result<Vec<u8>, EngineError>;
    fn encode_mouse(&mut self, input: TerminalMouseEvent) -> Result<Vec<u8>, EngineError>;
    fn clear_scrollback(&mut self) -> Result<(), EngineError>;
    fn scroll(&mut self, delta_rows: i32) -> Result<(), EngineError>;
    fn viewport_bounds(&self) -> ViewportBounds;
    fn search(&self, query: &SearchQuery) -> Result<Vec<SearchMatch>, EngineError>;
    fn selected_text(&self, range: SelectionRange) -> Result<String, EngineError>;
}

pub struct DefaultTerminalEngine {
    adapter: AlacrittyAdapter,
}

impl std::fmt::Debug for DefaultTerminalEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DefaultTerminalEngine")
            .field("size", &self.adapter.size())
            .finish_non_exhaustive()
    }
}

impl DefaultTerminalEngine {
    pub fn new(
        settings: &ResolvedTerminalProfile,
        size: TerminalSize,
    ) -> Result<Self, EngineError> {
        validate_size(size)?;
        Ok(Self {
            adapter: AlacrittyAdapter::new(settings, size),
        })
    }

    pub fn input(&mut self, bytes: &[u8]) -> Result<(), EngineError> {
        self.advance(bytes)?;
        Ok(())
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<(), EngineError> {
        TerminalEngine::resize(self, size)
    }

    pub fn snapshot(
        &self,
        viewport: Viewport,
        selection: Option<SelectionRange>,
    ) -> Arc<RenderFrame> {
        Arc::new(render::snapshot(&self.adapter, viewport, selection))
    }

    pub fn search(&self, query: &SearchQuery) -> Vec<SearchMatch> {
        text::search(&self.adapter, query)
    }

    pub fn selection_text(&self, range: SelectionRange) -> String {
        text::selection_text(&self.adapter, range)
    }

    pub fn clear_scrollback(&mut self) {
        self.adapter.clear_scrollback();
    }

    pub fn reset(&mut self) {
        self.adapter.reset();
    }
}

impl TerminalEngine for DefaultTerminalEngine {
    fn display_modes(&self) -> TerminalDisplayModes {
        self.adapter.display_modes()
    }

    fn recover_display(&mut self) -> Result<DisplayRecovery, EngineError> {
        Ok(self.adapter.recover_display())
    }

    fn advance(&mut self, bytes: &[u8]) -> Result<EngineDelta, EngineError> {
        let outbound = self.adapter.input(bytes);
        Ok(EngineDelta {
            outbound,
            dirty: !bytes.is_empty(),
        })
    }

    fn resize(&mut self, size: TerminalSize) -> Result<(), EngineError> {
        validate_size(size)?;
        self.adapter.resize(size);
        Ok(())
    }

    fn render(
        &mut self,
        viewport: Viewport,
        selection: Option<SelectionRange>,
    ) -> Result<Arc<RenderFrame>, EngineError> {
        Ok(self.snapshot(viewport, selection))
    }

    fn encode_input(&mut self, input: TerminalInput) -> Result<Vec<u8>, EngineError> {
        match input {
            TerminalInput::CommittedText(text) => Ok(text.into_bytes()),
            TerminalInput::Key { code, modifiers } => self.adapter.encode_key(code, modifiers),
        }
    }

    fn encode_mouse(&mut self, input: TerminalMouseEvent) -> Result<Vec<u8>, EngineError> {
        if !self.adapter.mouse_reporting_active() {
            return Err(EngineError::UnsupportedMouse("mouse reporting is disabled"));
        }
        self.adapter.encode_mouse(input)
    }

    fn clear_scrollback(&mut self) -> Result<(), EngineError> {
        self.adapter.clear_scrollback();
        Ok(())
    }

    fn scroll(&mut self, _delta_rows: i32) -> Result<(), EngineError> {
        // Viewport position is actor-owned; this stateless backend needs no extra mutation.
        Ok(())
    }

    fn viewport_bounds(&self) -> ViewportBounds {
        self.adapter.viewport_bounds()
    }

    fn search(&self, query: &SearchQuery) -> Result<Vec<SearchMatch>, EngineError> {
        Ok(DefaultTerminalEngine::search(self, query))
    }

    fn selected_text(&self, range: SelectionRange) -> Result<String, EngineError> {
        Ok(self.selection_text(range))
    }
}

fn validate_size(size: TerminalSize) -> Result<(), EngineError> {
    if size.cols == 0 || size.rows == 0 {
        Err(EngineError::InvalidSize {
            cols: size.cols,
            rows: size.rows,
        })
    } else {
        Ok(())
    }
}
