use alacritty_terminal::{
    Term,
    term::{Config, TermMode},
    vte::ansi::{CursorShape, Processor},
};
use rshell_core::{
    ResolvedTerminalProfile,
    render::{DisplayRecovery, TerminalDisplayModes},
};

use crate::alacritty_event::EventSink;

const RECOVERY_SEQUENCE: &[u8] = concat!(
    "\u{1b}[?1l",
    "\u{1b}[?25h",
    "\u{1b}[?1000l",
    "\u{1b}[?1002l",
    "\u{1b}[?1003l",
    "\u{1b}[?1005l",
    "\u{1b}[?1006l",
    "\u{1b}[1 q",
)
.as_bytes();

pub(crate) fn modes(terminal: &Term<EventSink>, events: &EventSink) -> TerminalDisplayModes {
    let mode = *terminal.mode();
    TerminalDisplayModes {
        alternate_screen: mode.contains(TermMode::ALT_SCREEN),
        enhanced_keyboard: mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL),
        mouse_reporting: mode.intersects(TermMode::MOUSE_MODE),
        application_cursor: mode.contains(TermMode::APP_CURSOR),
        cursor_hidden: !mode.contains(TermMode::SHOW_CURSOR)
            || terminal.cursor_style().shape == CursorShape::Hidden,
        stale_title: events.title() != "rsHell",
    }
}

pub(crate) fn recover(
    terminal: &mut Term<EventSink>,
    events: &EventSink,
    settings: &ResolvedTerminalProfile,
) -> DisplayRecovery {
    let before = modes(terminal, events);

    if terminal.mode().contains(TermMode::ALT_SCREEN) {
        terminal.swap_alt();
    }
    // Entering alternate screen clears its grid. The second swap returns to the
    // untouched primary grid and history.
    terminal.swap_alt();
    terminal.swap_alt();

    // Toggling the public option clears both active and inactive Kitty stacks.
    terminal.set_options(config(settings, false));
    terminal.set_options(config(settings, settings.enable_kitty_keyboard));

    // A dedicated processor preserves any partial UTF-8/parser state in the live processor.
    let mut recovery_processor: Processor = Processor::new();
    recovery_processor.advance(terminal, RECOVERY_SEQUENCE);
    events.take_outbound();
    events.reset_title();

    let after = modes(terminal, events);
    DisplayRecovery {
        before,
        after,
        changed: before != after,
    }
}

fn config(settings: &ResolvedTerminalProfile, kitty_keyboard: bool) -> Config {
    Config {
        scrolling_history: settings.scrollback_lines,
        kitty_keyboard,
        ..Config::default()
    }
}
