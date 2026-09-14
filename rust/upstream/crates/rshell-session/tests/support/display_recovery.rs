use std::sync::Arc;

use rshell_core::{RenderCursor, RenderRow, Viewport};
use rshell_session::{DefaultTerminalEngine, TerminalEngine, ViewportBounds};

pub const MODE_SEQUENCE: &[u8] = concat!(
    "\u{1b}]0;rshell-recovery-fixture\u{7}",
    "\u{1b}[?1049h",
    "\u{1b}[>1u",
    "\u{1b}[?1000h",
    "\u{1b}[?1006h",
    "\u{1b}[?1h",
    "\u{1b}[?25l",
    "fixture-界-e\u{301}"
)
.as_bytes();

#[derive(Debug)]
pub struct FrameObservation {
    rows: Arc<[RenderRow]>,
    cursor: Option<RenderCursor>,
    title: String,
    alternate_screen: bool,
    mouse_reporting: bool,
    bounds: ViewportBounds,
    replacement_count: usize,
}

impl FrameObservation {
    pub fn replacement_count(&self) -> usize {
        self.replacement_count
    }
}

pub fn two_chunk_splits(bytes: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    (0..=bytes.len()).map(|index| bytes.split_at(index))
}

pub fn feed_chunks<'a>(
    engine: &mut DefaultTerminalEngine,
    chunks: impl IntoIterator<Item = &'a [u8]>,
    viewport: Viewport,
) -> FrameObservation {
    for chunk in chunks {
        engine.input(chunk).expect("mode fixture input");
    }

    let bounds = engine.viewport_bounds();
    let frame = engine.snapshot(viewport, None);
    let replacement_count = frame
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .map(|cell| cell.text.matches('\u{fffd}').count())
        .sum();
    FrameObservation {
        rows: Arc::clone(&frame.rows),
        cursor: frame.cursor,
        title: frame.title.clone(),
        alternate_screen: frame.alternate_screen,
        mouse_reporting: frame.mouse_reporting,
        bounds,
        replacement_count,
    }
}

pub fn assert_frames_equivalent(expected: &FrameObservation, actual: &FrameObservation) {
    assert_eq!(actual.rows, expected.rows, "rendered rows differ");
    assert_eq!(actual.cursor, expected.cursor, "cursor state differs");
    assert_eq!(actual.title, expected.title, "terminal title differs");
    assert_eq!(
        actual.alternate_screen, expected.alternate_screen,
        "alternate-screen state differs"
    );
    assert_eq!(
        actual.mouse_reporting, expected.mouse_reporting,
        "mouse-reporting state differs"
    );
    assert_eq!(actual.bounds, expected.bounds, "viewport bounds differ");
    assert_eq!(
        actual.replacement_count, expected.replacement_count,
        "replacement-character count differs"
    );
}

pub fn assert_every_two_chunk_split(
    mut engine_factory: impl FnMut() -> DefaultTerminalEngine,
    viewport: Viewport,
    expected: &FrameObservation,
) {
    for (index, (first, second)) in two_chunk_splits(MODE_SEQUENCE).enumerate() {
        let mut engine = engine_factory();
        let actual = feed_chunks(&mut engine, [first, second], viewport);
        assert_frames_equivalent(expected, &actual);
        assert_eq!(
            actual.replacement_count(),
            0,
            "split {index} introduced a replacement character"
        );
    }
}
