use std::sync::{Arc, Mutex};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    vte::ansi::Rgb,
};
use rshell_core::TerminalSize;

#[derive(Debug)]
struct EventState {
    outbound: Vec<u8>,
    title: String,
    size: TerminalSize,
}

#[derive(Clone, Debug)]
pub(crate) struct EventSink(Arc<Mutex<EventState>>);

impl EventSink {
    pub(crate) fn new(size: TerminalSize) -> Self {
        Self(Arc::new(Mutex::new(EventState {
            outbound: Vec::new(),
            title: "rsHell".into(),
            size,
        })))
    }

    pub(crate) fn take_outbound(&self) -> Vec<u8> {
        std::mem::take(&mut self.state().outbound)
    }

    pub(crate) fn push_outbound(&self, bytes: &[u8]) {
        self.state().outbound.extend_from_slice(bytes);
    }

    pub(crate) fn title(&self) -> String {
        self.state().title.clone()
    }

    pub(crate) fn reset_title(&self) {
        self.state().title = "rsHell".into();
    }

    pub(crate) fn resize(&self, size: TerminalSize) {
        self.state().size = size;
    }

    fn state(&self) -> std::sync::MutexGuard<'_, EventState> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl EventListener for EventSink {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.push_outbound(text.as_bytes()),
            Event::Title(title) => self.state().title = title,
            Event::ResetTitle => self.state().title = "rsHell".into(),
            Event::TextAreaSizeRequest(formatter) => {
                let size = self.state().size;
                let cell_width = cell_extent(size.pixel_width, size.cols);
                let cell_height = cell_extent(size.pixel_height, size.rows);
                let response = formatter(WindowSize {
                    num_lines: size.rows,
                    num_cols: size.cols,
                    cell_width,
                    cell_height,
                });
                self.push_outbound(response.as_bytes());
            }
            Event::ColorRequest(_, formatter) => {
                let response = formatter(Rgb::default());
                self.push_outbound(response.as_bytes());
            }
            Event::ClipboardLoad(_, formatter) => {
                let response = formatter("");
                self.push_outbound(response.as_bytes());
            }
            Event::MouseCursorDirty
            | Event::ClipboardStore(_, _)
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Bell
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }
}

fn cell_extent(pixels: u32, cells: u16) -> u16 {
    let extent = pixels / u32::from(cells);
    extent.min(u32::from(u16::MAX)) as u16
}
