use alacritty_terminal::{Term, grid::Dimensions, term::TermMode, vte::ansi::Processor};
use rshell_core::ResolvedTerminalProfile;

use crate::{
    alacritty_event::EventSink,
    alacritty_primary_rows::PrimaryRows,
    alacritty_rows,
    alacritty_tracker::{ScrollTracker, Window},
};

struct FeedWindow<'a> {
    bytes: &'a [u8],
    maximum_shift: usize,
    track_capacity: bool,
}

pub(crate) fn advance(
    processor: &mut Processor,
    terminal: &mut Term<EventSink>,
    events: &EventSink,
    settings: &ResolvedTerminalProfile,
    primary_rows: &mut PrimaryRows,
    tracker: &mut ScrollTracker,
    bytes: &[u8],
) -> Vec<u8> {
    let mut remaining = bytes;
    while let Some(sequence) = next_alt_switch(remaining) {
        advance_segment(
            processor,
            terminal,
            events,
            settings,
            primary_rows,
            tracker,
            &remaining[..sequence.start],
        );
        advance_segment(
            processor,
            terminal,
            events,
            settings,
            primary_rows,
            tracker,
            &remaining[sequence.clone()],
        );
        remaining = &remaining[sequence.end..];
    }
    advance_segment(
        processor,
        terminal,
        events,
        settings,
        primary_rows,
        tracker,
        remaining,
    );
    events.take_outbound()
}

fn advance_segment(
    processor: &mut Processor,
    terminal: &mut Term<EventSink>,
    events: &EventSink,
    settings: &ResolvedTerminalProfile,
    primary_rows: &mut PrimaryRows,
    tracker: &mut ScrollTracker,
    bytes: &[u8],
) {
    if bytes.is_empty() {
        return;
    }
    let mut remaining = bytes;
    while let Some(index) = remaining.iter().position(|byte| *byte == 0x05) {
        advance_windows(
            processor,
            terminal,
            settings,
            primary_rows,
            tracker,
            &remaining[..=index],
        );
        events.push_outbound(settings.answerback.as_bytes());
        remaining = &remaining[index + 1..];
    }
    advance_windows(
        processor,
        terminal,
        settings,
        primary_rows,
        tracker,
        remaining,
    );
}

fn advance_windows(
    processor: &mut Processor,
    terminal: &mut Term<EventSink>,
    settings: &ResolvedTerminalProfile,
    primary_rows: &mut PrimaryRows,
    tracker: &mut ScrollTracker,
    mut remaining: &[u8],
) {
    while !remaining.is_empty() {
        let was_primary = !terminal.mode().contains(TermMode::ALT_SCREEN);
        let (length, maximum_shift, track_capacity) = if was_primary {
            let grid = terminal.grid();
            let room = settings
                .scrollback_lines
                .saturating_sub(grid.history_size());
            let maximum = if room == 0 {
                grid.total_lines().saturating_sub(3)
            } else {
                room
            };
            match tracker.next_window(remaining, true, maximum) {
                Window::Bounded {
                    length,
                    maximum_shift,
                } => (length, maximum_shift, true),
                Window::Unsafe { length } => (length, 0, false),
            }
        } else {
            tracker.consume(remaining, false);
            (remaining.len(), 0, false)
        };
        advance_window(
            processor,
            terminal,
            settings,
            primary_rows,
            tracker,
            FeedWindow {
                bytes: &remaining[..length],
                maximum_shift,
                track_capacity,
            },
        );
        remaining = &remaining[length..];
    }
}

fn advance_window(
    processor: &mut Processor,
    terminal: &mut Term<EventSink>,
    settings: &ResolvedTerminalProfile,
    primary_rows: &mut PrimaryRows,
    tracker: &mut ScrollTracker,
    window: FeedWindow<'_>,
) {
    let FeedWindow {
        bytes,
        maximum_shift,
        track_capacity,
    } = window;
    let was_primary = !terminal.mode().contains(TermMode::ALT_SCREEN);
    let old_history = terminal.grid().history_size();
    let anchor_shift = if track_capacity { maximum_shift } else { 0 };
    let capacity_anchor = was_primary
        .then(|| alacritty_rows::capture(terminal, anchor_shift))
        .flatten();

    processor.advance(terminal, bytes);
    let active_primary = !terminal.mode().contains(TermMode::ALT_SCREEN);
    let cursor = &terminal.grid().cursor;
    tracker.sync_cursor(
        active_primary,
        cursor.point.line.0 as usize,
        cursor.point.column.0,
        cursor.input_needs_wrap,
    );

    if active_primary {
        let history = terminal.grid().history_size();
        if was_primary {
            let completed = if old_history == settings.scrollback_lines
                || (!track_capacity && history == settings.scrollback_lines)
            {
                alacritty_rows::completed_shift(
                    terminal,
                    settings.scrollback_lines,
                    capacity_anchor,
                )
            } else {
                0
            };
            let shift = history
                .saturating_sub(old_history)
                .saturating_add(completed);
            primary_rows.origin = primary_rows.origin.saturating_add(shift as i64);
        } else if history >= primary_rows.history {
            primary_rows.origin = primary_rows
                .origin
                .saturating_add(i64::try_from(history - primary_rows.history).unwrap_or(i64::MAX));
        } else {
            primary_rows.origin = primary_rows
                .origin
                .saturating_sub(i64::try_from(primary_rows.history - history).unwrap_or(i64::MAX));
        }
        primary_rows.history = history;
    } else if was_primary {
        primary_rows.history = old_history;
    }
}

fn next_alt_switch(bytes: &[u8]) -> Option<std::ops::Range<usize>> {
    for start in 0..bytes.len().saturating_sub(3) {
        if !bytes[start..].starts_with(b"\x1b[?") {
            continue;
        }
        let tail = &bytes[start + 3..];
        let final_offset = tail.iter().position(|byte| (0x40..=0x7e).contains(byte))?;
        let final_byte = tail[final_offset];
        if !matches!(final_byte, b'h' | b'l') {
            continue;
        }
        let parameters = &tail[..final_offset];
        if parameters
            .split(|byte| *byte == b';')
            .any(|value| value == b"1049")
        {
            return Some(start..start + 3 + final_offset + 1);
        }
    }
    None
}
