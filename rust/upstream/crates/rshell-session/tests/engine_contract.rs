use std::{collections::BTreeSet, sync::Arc};

mod support;

use rshell_core::{
    CellPosition, Color, CursorShape, KeyCode, KeyModifiers, MouseButton, MouseEventKind,
    RenderFrame, ResolvedTerminalProfile, SearchQuery, SelectionRange, TerminalInput,
    TerminalMouseEvent, TerminalOverrides, TerminalSettingsV1, TerminalSize, Viewport,
};
use rshell_session::{DefaultTerminalEngine, TerminalEngine};
use sha2::{Digest, Sha256};
use support::display_recovery::{MODE_SEQUENCE, assert_every_two_chunk_split, feed_chunks};

const FIXTURE: &[u8] = include_bytes!("fixtures/compatibility.ansi");
const CANARY_FIXTURE: &str = include_str!("fixtures/vt/canary.json");

#[test]
fn compatibility_fixture_is_not_rewritten_by_platform_line_endings() {
    assert!(FIXTURE.ends_with(b"\x1b[?25l\n"));
    assert!(
        !FIXTURE.windows(2).any(|bytes| bytes == b"\r\n"),
        "terminal control fixture must retain repository LF bytes"
    );
}

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize {
        cols,
        rows,
        pixel_width: u32::from(cols) * 8,
        pixel_height: u32::from(rows) * 16,
        dpi: 96,
    }
}

fn profile(scrollback_lines: usize) -> ResolvedTerminalProfile {
    TerminalSettingsV1 {
        scrollback_lines,
        ..TerminalSettingsV1::default()
    }
    .resolve(&TerminalOverrides::default())
}

fn viewport(top_stable_row: i64, rows: u16) -> Viewport {
    Viewport {
        top_stable_row,
        rows,
    }
}

fn display_mode_fixture_engine() -> DefaultTerminalEngine {
    engine_with_size(size(32, 6), |settings| {
        settings.scrollback_lines = 1_000;
        settings.enable_kitty_keyboard = true;
    })
}

#[test]
fn whole_and_every_two_chunk_split_are_identical() {
    let fixture_viewport = viewport(0, 6);
    let mut engine = display_mode_fixture_engine();
    let whole = feed_chunks(&mut engine, [MODE_SEQUENCE], fixture_viewport);

    assert_every_two_chunk_split(display_mode_fixture_engine, fixture_viewport, &whole);
}

#[test]
fn mode_fixture_contains_no_replacement_character() {
    let mut engine = display_mode_fixture_engine();
    let observed = feed_chunks(&mut engine, [MODE_SEQUENCE], viewport(0, 6));

    assert_eq!(observed.replacement_count(), 0);
}

fn frame_text(frame: &RenderFrame) -> String {
    frame
        .rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.text.as_str()).collect())
        .collect::<Vec<String>>()
        .join("\n")
}

#[test]
fn display_recovery_preserves_primary_and_clears_modes() {
    let mut engine = display_mode_fixture_engine();
    engine
        .input(
            b"primary-00\r\nprimary-01\r\nprimary-02\r\nprimary-03\r\nprimary-04\r\nprimary-05\r\nprimary-06\r\nprimary-07\r\nprimary-08\r\nprimary-09\r\nprimary-tail",
        )
        .unwrap();
    let query = SearchQuery {
        needle: "primary-00".into(),
        case_sensitive: true,
        regex: false,
    };
    let primary_before = engine.search(&query)[0];
    engine.input(MODE_SEQUENCE).unwrap();
    engine
        .input(b"\x1b[>2u\x1b[>4u\x1b[?1002h\x1b[?1003h\x1b[?1005h\xe7\x95")
        .unwrap();

    let residue = engine.display_modes();
    assert!(residue.alternate_screen);
    assert!(residue.enhanced_keyboard);
    assert!(residue.mouse_reporting);
    assert!(residue.application_cursor);
    assert!(residue.cursor_hidden);
    assert!(residue.stale_title);
    assert!(residue.has_residue());

    let recovery = engine.recover_display().unwrap();
    assert_eq!(recovery.before, residue);
    assert_eq!(recovery.after, Default::default());
    assert!(recovery.changed);
    assert_eq!(engine.display_modes(), Default::default());
    engine.input(b"\x8c").unwrap();
    assert_eq!(engine.advance(b"\x1b[?u").unwrap().outbound, b"\x1b[?0u");

    let primary_after = engine.search(&query)[0];
    assert_eq!(
        primary_after.start.stable_row,
        primary_before.start.stable_row
    );
    let bounds = engine.viewport_bounds();
    let history = engine.snapshot(viewport(bounds.first_stable_row, 6), None);
    let recovered = engine.snapshot(viewport(bounds.bottom_top_stable_row, 6), None);
    let recovered_text = format!("{}\n{}", frame_text(&history), frame_text(&recovered));
    assert_eq!(recovered.title, "rsHell");
    assert!(frame_text(&history).contains("primary-00"));
    assert!(frame_text(&recovered).contains("primary-tail"));
    assert!(recovered_text.contains('界'));
    assert!(!recovered_text.contains("fixture-"));
    assert!(!recovered_text.contains('\u{fffd}'));
    assert_eq!(
        recovered.alternate_screen,
        recovered.display_modes.alternate_screen
    );
    assert_eq!(
        recovered.mouse_reporting,
        recovered.display_modes.mouse_reporting
    );
}

#[test]
fn fixture_converts_styles_unicode_wrap_title_cursor_and_mouse_mode() {
    let mut engine = DefaultTerminalEngine::new(&profile(20_000), size(16, 8)).unwrap();
    engine.input(FIXTURE).unwrap();

    let frame = engine.snapshot(viewport(0, 8), None);
    let styled = frame
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .find(|cell| cell.text.contains('S'))
        .expect("styled cell");
    assert_eq!(styled.foreground, Color::Ansi(1));
    assert_eq!(styled.background, Color::Ansi(4));
    assert!(styled.attributes.bold);
    assert!(styled.attributes.italic);
    assert!(styled.attributes.underline);
    assert!(styled.attributes.strike);
    assert!(styled.attributes.reverse);

    let text = frame_text(&frame);
    assert!(text.contains('界'));
    assert!(text.contains("é"), "combining mark must stay with its cell");
    assert!(frame.rows.iter().any(|row| row.wrapped));
    assert_eq!(frame.title, "rsHell contract title");
    assert!(frame.mouse_reporting);
    assert!(!frame.alternate_screen);

    let cursor = frame
        .cursor
        .expect("cursor state is represented even when hidden");
    assert_eq!(cursor.position.column, 3);
    assert_eq!(cursor.position.stable_row, 3);
    assert_eq!(cursor.shape, CursorShape::Beam);
    assert!(!cursor.visible);
}

#[test]
fn indexed_rgb_colors_and_configured_answerback_use_public_adapter_state() {
    let mut engine = engine_with(|settings| settings.answerback = "rshell-ready".into());
    let delta = engine.advance(b"\x1b[38;5;123;48;2;1;2;3mX\x05").unwrap();
    assert_eq!(delta.outbound, b"rshell-ready");

    let frame = engine.snapshot(viewport(0, 3), None);
    let cell = frame
        .rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .find(|cell| cell.text == "X")
        .unwrap();
    assert_eq!(cell.foreground, Color::Ansi(123));
    assert_eq!(cell.background, Color::Rgb(1, 2, 3));
}

#[test]
fn alternate_screen_is_isolated_and_restores_primary_screen() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 4)).unwrap();
    engine.input(b"primary").unwrap();
    engine.input(b"\x1b[?1049hALT-SCREEN\x1b[?1002h").unwrap();

    let alternate = engine.snapshot(viewport(0, 4), None);
    assert!(alternate.alternate_screen);
    assert!(alternate.mouse_reporting);
    assert!(frame_text(&alternate).contains("ALT-SCREEN"));
    assert!(!frame_text(&alternate).contains("primary"));

    engine.input(b"\x1b[?1049l").unwrap();
    let primary = engine.snapshot(viewport(0, 4), None);
    assert!(!primary.alternate_screen);
    assert!(frame_text(&primary).contains("primary"));
    assert!(!frame_text(&primary).contains("ALT-SCREEN"));

    engine.input(b"\x1b[?1002l").unwrap();
    assert!(!engine.snapshot(viewport(0, 4), None).mouse_reporting);
}

#[test]
fn same_chunk_primary_output_before_alternate_screen_keeps_stable_row_ids() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    engine.input(b"anchor\r\n").unwrap();
    let before = engine.search(&SearchQuery {
        needle: "anchor".into(),
        case_sensitive: true,
        regex: false,
    })[0];

    engine
        .input(b"one\r\ntwo\r\nthree\r\n\x1b[?1049hALT")
        .unwrap();
    engine.input(b"\x1b[?1049l").unwrap();
    let after = engine.search(&SearchQuery {
        needle: "anchor".into(),
        case_sensitive: true,
        regex: false,
    })[0];

    assert_eq!(after.start.stable_row, before.start.stable_row);
}

#[test]
fn scrollback_selection_and_all_search_modes_use_stable_rows() {
    let mut engine = DefaultTerminalEngine::new(&profile(20_000), size(32, 5)).unwrap();
    let mut stream = Vec::new();
    for line in 0..10_050 {
        stream.extend_from_slice(format!("record-{line:05} Alpha beta\r\n").as_bytes());
    }
    engine.input(&stream).unwrap();

    let plain = engine.search(&SearchQuery {
        needle: "record-00001".into(),
        case_sensitive: true,
        regex: false,
    });
    assert_eq!(
        plain.len(),
        1,
        "oldest retained scrollback remains searchable"
    );
    assert!(
        engine
            .search(&SearchQuery {
                needle: "alpha".into(),
                case_sensitive: false,
                regex: false,
            })
            .len()
            >= 10_000
    );
    assert!(
        engine
            .search(&SearchQuery {
                needle: r"record-1004[0-9] Alpha".into(),
                case_sensitive: true,
                regex: true,
            })
            .len()
            >= 10
    );
    assert!(
        engine
            .search(&SearchQuery {
                needle: "alpha".into(),
                case_sensitive: true,
                regex: false,
            })
            .is_empty()
    );

    let first = plain[0];
    let historical = engine.snapshot(viewport(first.start.stable_row, 1), None);
    assert_eq!(historical.rows[0].stable_row, first.start.stable_row);
    assert!(frame_text(&historical).contains("record-00001"));

    let second = engine.search(&SearchQuery {
        needle: "record-10049".into(),
        case_sensitive: true,
        regex: false,
    })[0];
    let selected = engine.selection_text(SelectionRange {
        start: first.start,
        end: second.end,
        rectangular: false,
    });
    assert!(selected.starts_with("record-00001"));
    assert!(selected.contains("record-05000 Alpha beta"));
    assert!(selected.ends_with("record-10049"));
}

#[test]
fn long_output_exposes_clamped_viewport_bounds() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(24, 3)).unwrap();
    let output = (0..20)
        .map(|index| format!("record-{index:02}\r\n"))
        .collect::<String>();
    engine.input(output.as_bytes()).unwrap();

    let bounds = engine.viewport_bounds();
    assert!(bounds.first_stable_row <= bounds.bottom_top_stable_row);

    let bottom = engine.snapshot(viewport(bounds.bottom_top_stable_row, 3), None);
    assert_eq!(bottom.viewport_top, bounds.bottom_top_stable_row);
    assert!(frame_text(&bottom).contains("record-19"));

    let clamped = engine.snapshot(viewport(i64::MAX, 3), None);
    assert_eq!(clamped.viewport_top, bounds.bottom_top_stable_row);
    let oldest = engine.snapshot(viewport(i64::MIN, 3), None);
    assert_eq!(oldest.viewport_top, bounds.first_stable_row);
}

#[test]
fn stable_row_ids_survive_output_after_scrollback_reaches_its_limit() {
    let mut engine = DefaultTerminalEngine::new(&profile(100), size(24, 3)).unwrap();
    let first = (0..150)
        .map(|index| format!("record-{index:03}\r\n"))
        .collect::<String>();
    engine.input(first.as_bytes()).unwrap();
    let before = engine.search(&SearchQuery {
        needle: "record-100".into(),
        case_sensitive: true,
        regex: false,
    })[0];

    let additional = (150..160)
        .map(|index| format!("record-{index:03}\r\n"))
        .collect::<String>();
    engine.input(additional.as_bytes()).unwrap();
    let after = engine.search(&SearchQuery {
        needle: "record-100".into(),
        case_sensitive: true,
        regex: false,
    })[0];

    assert_eq!(after.start.stable_row, before.start.stable_row);
}

#[test]
fn identical_rows_at_capacity_receive_new_monotonic_stable_rows() {
    let mut engine = repeated_rows_at_capacity();
    let before = engine.viewport_bounds();
    let history_span = before.bottom_top_stable_row - before.first_stable_row;
    let mut bottom_rows = BTreeSet::new();
    for cycle in 1..=12 {
        engine.input(b"same\r\n").unwrap();
        let bounds = engine.viewport_bounds();
        assert_eq!(
            bounds.first_stable_row,
            before.first_stable_row + cycle,
            "capacity eviction must not reuse the oldest row identity"
        );
        assert_eq!(
            bounds.bottom_top_stable_row,
            before.bottom_top_stable_row + cycle,
            "follow-bottom must advance one stable row per full-history eviction"
        );
        assert_eq!(
            bounds.bottom_top_stable_row - bounds.first_stable_row,
            history_span,
            "capacity eviction must not make the viewport jump"
        );
        assert!(bottom_rows.insert(bounds.bottom_top_stable_row));
    }
}

#[test]
fn unsaturated_output_advances_stable_rows_without_a_capacity_anchor() {
    let mut engine = DefaultTerminalEngine::new(&profile(100), size(12, 3)).unwrap();
    engine.input(b"same\r\nsame\r\nsame\r\n").unwrap();
    let first = engine.viewport_bounds();
    assert_eq!(first.first_stable_row, 0);
    assert_eq!(first.bottom_top_stable_row, 1);

    engine.input(b"same\r\nsame\r\n").unwrap();
    let second = engine.viewport_bounds();
    assert_eq!(second.first_stable_row, 0);
    assert_eq!(second.bottom_top_stable_row, 3);
}

#[test]
fn multi_line_chunk_at_capacity_tracks_each_identical_row_eviction() {
    let mut engine = repeated_rows_at_capacity();
    let before = engine.viewport_bounds();
    engine.input(b"same\r\nsame\r\nsame\r\n").unwrap();
    let after = engine.viewport_bounds();

    assert_eq!(after.first_stable_row, before.first_stable_row + 3);
    assert_eq!(
        after.bottom_top_stable_row,
        before.bottom_top_stable_row + 3
    );
}

#[test]
fn all_evicted_capacity_anchor_reserves_a_disjoint_stable_range() {
    let mut engine = repeated_rows_at_capacity();
    let before = engine.viewport_bounds();
    let output = b"same\r\n".repeat(101);
    engine.input(&output).unwrap();
    let after = engine.viewport_bounds();

    assert_eq!(
        after.bottom_top_stable_row,
        before.bottom_top_stable_row + 101
    );
    assert!(after.first_stable_row > before.bottom_top_stable_row);
}

#[test]
fn tiny_saturated_grid_reserves_a_disjoint_range_when_no_window_is_safe() {
    let mut settings = profile(100);
    settings.scrollback_lines = 1;
    let mut engine = DefaultTerminalEngine::new(&settings, size(12, 3)).unwrap();
    engine.input(b"same\r\nsame\r\nsame\r\n").unwrap();
    let before = engine.viewport_bounds();
    engine.input(b"\x1b[10S").unwrap();
    let after = engine.viewport_bounds();

    assert_eq!(
        after.bottom_top_stable_row,
        before.bottom_top_stable_row + 4
    );
    assert!(after.first_stable_row > before.bottom_top_stable_row);
}

#[test]
fn single_input_full_ring_rotation_keeps_identical_rows_monotonic() {
    assert_full_ring_rotations(1);
}

#[test]
fn single_input_multiple_ring_rotations_keep_identical_rows_monotonic() {
    assert_full_ring_rotations(2);
}

#[test]
fn fresh_large_input_crosses_capacity_without_reusing_stable_rows() {
    let mut engine = DefaultTerminalEngine::new(&profile(100), size(12, 3)).unwrap();
    let before = engine.viewport_bounds();
    engine.input(&b"same\r\n".repeat(205)).unwrap();
    assert_crossing_bounds(&engine, before, 203, 103);
}

#[test]
fn partial_large_input_crosses_capacity_without_reusing_stable_rows() {
    let mut engine = DefaultTerminalEngine::new(&profile(100), size(12, 3)).unwrap();
    for _ in 0..52 {
        engine.input(b"same\r\n").unwrap();
    }
    let before = engine.viewport_bounds();
    engine.input(&b"same\r\n".repeat(256)).unwrap();
    assert_crossing_bounds(&engine, before, 256, 206);
}

#[test]
fn ind_scrolls_in_one_input_keep_stable_rows_monotonic() {
    let mut engine = repeated_rows_at_capacity();
    let before = engine.viewport_bounds();
    engine.input(&b"\x1bD".repeat(206)).unwrap();
    assert_saturated_scroll(&engine, before, 206);
}

#[test]
fn csi_scroll_up_in_one_input_keeps_stable_rows_monotonic() {
    let mut engine = repeated_rows_at_capacity();
    let before = engine.viewport_bounds();
    engine.input(&b"\x1b[10S".repeat(400)).unwrap();
    assert_saturated_scroll(&engine, before, 1_200);
}

#[test]
fn repeated_default_csi_scroll_up_tracks_one_and_two_logical_rotations() {
    for scrolls in [103, 206] {
        let mut engine = repeated_rows_at_capacity();
        let before = engine.viewport_bounds();
        engine.input(&b"\x1b[S".repeat(scrolls)).unwrap();
        assert_saturated_scroll(&engine, before, scrolls as i64);
        assert!(
            engine.viewport_bounds().first_stable_row > before.bottom_top_stable_row,
            "a complete logical rotation must not reuse a stable row"
        );
    }
}

#[test]
fn csi_scroll_up_only_advances_primary_origin_with_full_margins() {
    let mut engine = repeated_rows_at_capacity();
    engine.input(b"\x1b[2;3r").unwrap();
    let subregion_before = engine.viewport_bounds();
    engine.input(&b"\x1b[S".repeat(206)).unwrap();
    assert_eq!(engine.viewport_bounds(), subregion_before);

    engine.input(b"\x1b[r").unwrap();
    let full_before = engine.viewport_bounds();
    engine.input(b"\x1b[S").unwrap();
    assert_saturated_scroll(&engine, full_before, 1);
}

#[test]
fn cursor_forward_then_text_wraps_are_counted_at_capacity() {
    let mut engine = repeated_rows_at_capacity();
    let before = engine.viewport_bounds();
    engine.input(&b"\x1b[999CXX".repeat(103)).unwrap();
    assert_saturated_scroll(&engine, before, 103);
    assert!(engine.viewport_bounds().first_stable_row > before.bottom_top_stable_row);
}

#[test]
fn wide_utf8_rows_complete_a_logical_rotation_without_reusing_ids() {
    let mut whole = repeated_rows_at_capacity();
    let whole_before = whole.viewport_bounds();
    whole.input(&"界".as_bytes().repeat(619)).unwrap();
    assert_saturated_scroll(&whole, whole_before, 103);
    assert!(whole.viewport_bounds().first_stable_row > whole_before.bottom_top_stable_row);

    let mut split = repeated_rows_at_capacity();
    let split_before = split.viewport_bounds();
    split.input(b"\xe7\x95").unwrap();
    split.input(b"\x8c").unwrap();
    split.input(&"界".as_bytes().repeat(618)).unwrap();
    assert_saturated_scroll(&split, split_before, 103);
    assert!(split.viewport_bounds().first_stable_row > split_before.bottom_top_stable_row);
}

#[test]
fn utf8_continuation_c1_bytes_do_not_start_csi() {
    let mut whole = repeated_rows_at_capacity();
    let whole_before = whole.viewport_bounds();
    whole.input(&"Û".as_bytes().repeat(1_237)).unwrap();
    assert_saturated_scroll(&whole, whole_before, 103);
    assert!(whole.viewport_bounds().first_stable_row > whole_before.bottom_top_stable_row);

    let mut split = repeated_rows_at_capacity();
    let split_before = split.viewport_bounds();
    split.input(b"\xc3").unwrap();
    split.input(b"\x9b").unwrap();
    split.input(&"Û".as_bytes().repeat(1_236)).unwrap();
    assert_saturated_scroll(&split, split_before, 103);
    assert!(split.viewport_bounds().first_stable_row > split_before.bottom_top_stable_row);
}

#[test]
fn same_size_resize_preserves_wrap_tracking_before_linefeed() {
    let mut engine = repeated_rows_at_capacity();
    engine.input(b"\x1b[3;12HX").unwrap();
    let before = engine.viewport_bounds();
    engine.resize(size(12, 3)).unwrap();
    assert_eq!(engine.viewport_bounds(), before);
    engine.input(b"\n").unwrap();
    assert_saturated_scroll(&engine, before, 1);
}

#[test]
fn changed_size_resize_syncs_cursor_before_following_scroll() {
    let mut engine = repeated_rows_at_capacity();
    engine.input(b"\x1b[3;12HX").unwrap();
    engine.resize(size(24, 3)).unwrap();
    let resized = engine.viewport_bounds();
    let frame = engine.snapshot(viewport(resized.bottom_top_stable_row, 3), None);
    assert_eq!(frame.size, size(24, 3));
    engine.input(b"\n").unwrap();
    assert_saturated_scroll(&engine, resized, 1);
}

#[test]
fn decsc_decrc_restore_bottom_wrap_for_every_scroll() {
    let mut engine = repeated_rows_at_capacity();
    engine.input(b"\x1b[3;12HX\x1b7").unwrap();
    let before = engine.viewport_bounds();
    engine.input(&b"\x1b[H\x1b8XX".repeat(103)).unwrap();
    assert_saturated_scroll(&engine, before, 103);
    assert!(engine.viewport_bounds().first_stable_row > before.bottom_top_stable_row);
}

#[test]
fn csi_save_restore_matches_decsc_decrc_scroll_behavior() {
    let mut engine = repeated_rows_at_capacity();
    engine.input(b"\x1b[3;12HX\x1b[s").unwrap();
    let before = engine.viewport_bounds();
    engine.input(&b"\x1b[H\x1b[uXX".repeat(103)).unwrap();
    assert_saturated_scroll(&engine, before, 103);
    assert!(engine.viewport_bounds().first_stable_row > before.bottom_top_stable_row);
}

#[test]
fn split_cursor_save_restore_sequences_preserve_primary_and_alternate_state() {
    let mut engine = repeated_rows_at_capacity();
    engine.input(b"\x1b[3;12HX\x1b").unwrap();
    engine.input(b"7").unwrap();
    let primary_before = engine.viewport_bounds();

    engine.input(b"\x1b[?1049h\x1b[3;12HX\x1b7").unwrap();
    engine.input(b"\x1b[H\x1b").unwrap();
    engine.input(b"8XX\x1b[?1049l").unwrap();
    assert_eq!(engine.viewport_bounds(), primary_before);

    engine.input(b"\x1b[").unwrap();
    engine.input(b"H\x1b").unwrap();
    engine.input(b"8XX").unwrap();
    let primary_after = engine.viewport_bounds();
    assert_saturated_scroll(&engine, primary_before, 1);

    engine
        .input(b"\x1b[?1049h\x1b[H\x1b8XX\x1b[?1049l")
        .unwrap();
    assert_eq!(engine.viewport_bounds(), primary_after);
}

#[test]
fn tracker_persists_split_csi_margin_and_cursor_sequences() {
    let mut engine = repeated_rows_at_capacity();
    engine.input(b"\x1b[2;").unwrap();
    engine.input(b"3r\x1b[").unwrap();
    engine.input(b"S").unwrap();
    let before = engine.viewport_bounds();
    engine.input(&b"\x1b[S".repeat(103)).unwrap();
    assert_eq!(engine.viewport_bounds(), before);

    engine.input(b"\x1b[").unwrap();
    engine.input(b"r\x1b[3;").unwrap();
    engine.input(b"1H\x1b[999").unwrap();
    engine.input(b"CXX").unwrap();
    let after_first_wrap = engine.viewport_bounds();
    assert_saturated_scroll(&engine, before, 1);
    for _ in 1..103 {
        engine.input(b"\x1b[999").unwrap();
        engine.input(b"CXX").unwrap();
    }
    assert_saturated_scroll(&engine, after_first_wrap, 102);
}

#[test]
fn subregion_scroll_does_not_advance_primary_origin() {
    let mut engine = repeated_rows_at_capacity();
    let before = engine.viewport_bounds();
    engine.input(b"\x1b[2;3r\x1b[100S\x1b[r").unwrap();
    assert_eq!(engine.viewport_bounds(), before);
}

#[test]
fn cursor_shape_canary_maps_fixed_revision_variants() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();

    engine.input(b"\x1b[3 q").unwrap();
    let underline = engine.snapshot(viewport(0, 3), None).cursor.unwrap();
    assert_eq!(underline.shape, CursorShape::Underline);
    assert!(underline.visible);

    engine.input(b"\x1b[1 q").unwrap();
    let block = engine.snapshot(viewport(0, 3), None).cursor.unwrap();
    assert_eq!(block.shape, CursorShape::Block);

    engine.input(b"\x1b[5 q").unwrap();
    let beam = engine.snapshot(viewport(0, 3), None).cursor.unwrap();
    assert_eq!(beam.shape, CursorShape::Beam);
}

#[test]
fn resize_preserves_content_and_updates_frame_geometry() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(12, 3)).unwrap();
    engine.input(b"preserved-content").unwrap();
    engine.resize(size(24, 6)).unwrap();

    let frame = engine.snapshot(viewport(0, 6), None);
    assert_eq!(frame.size, size(24, 6));
    assert!(frame_text(&frame).contains("preserved-content"));
}

#[test]
fn taller_resize_preserves_stable_rows_promoted_from_history() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(12, 3)).unwrap();
    engine.input(b"alpha\r\nbeta\r\ngamma\r\ndelta").unwrap();
    let query = SearchQuery {
        needle: "alpha".into(),
        case_sensitive: true,
        regex: false,
    };
    let before = engine.search(&query)[0];

    engine.resize(size(12, 5)).unwrap();

    let after = engine.search(&query)[0];
    assert_eq!(after.start.stable_row, before.start.stable_row);
    let historical = engine.snapshot(viewport(before.start.stable_row, 1), None);
    assert!(frame_text(&historical).contains("alpha"));

    engine.resize(size(12, 3)).unwrap();
    let after_shrink = engine.search(&query)[0];
    assert_eq!(after_shrink.start.stable_row, before.start.stable_row);
}

#[test]
fn alternate_resize_reconciles_inactive_primary_stable_rows_on_restore() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(12, 3)).unwrap();
    engine.input(b"alpha\r\nbeta\r\ngamma\r\ndelta").unwrap();
    let query = SearchQuery {
        needle: "alpha".into(),
        case_sensitive: true,
        regex: false,
    };
    let before = engine.search(&query)[0];

    engine.input(b"\x1b[?1049halternate").unwrap();
    engine.resize(size(12, 5)).unwrap();
    engine.input(b"\x1b[?1049l").unwrap();

    let after = engine.search(&query)[0];
    assert_eq!(after.start.stable_row, before.start.stable_row);
    let restored = engine.snapshot(viewport(before.start.stable_row, 1), None);
    assert!(frame_text(&restored).contains("alpha"));
}

#[test]
fn saturated_scrollback_shrink_preserves_retained_primary_stable_rows() {
    let mut engine = saturated_scrollback_engine();
    let query = SearchQuery {
        needle: "line002".into(),
        case_sensitive: true,
        regex: false,
    };
    let before = engine.search(&query)[0];

    engine.resize(size(12, 2)).unwrap();

    let after = engine.search(&query)[0];
    assert_eq!(after.start.stable_row, before.start.stable_row);
}

#[test]
fn saturated_scrollback_shrink_in_alternate_preserves_primary_stable_rows() {
    let mut engine = saturated_scrollback_engine();
    let query = SearchQuery {
        needle: "line002".into(),
        case_sensitive: true,
        regex: false,
    };
    let before = engine.search(&query)[0];

    engine.input(b"\x1b[?1049halternate").unwrap();
    engine.resize(size(12, 2)).unwrap();
    engine.input(b"\x1b[?1049l").unwrap();

    let after = engine.search(&query)[0];
    assert_eq!(after.start.stable_row, before.start.stable_row);
}

#[test]
fn repeated_inactive_primary_resizes_preserve_saturated_stable_rows() {
    let mut engine = saturated_scrollback_engine();
    let query = SearchQuery {
        needle: "line002".into(),
        case_sensitive: true,
        regex: false,
    };
    let before = engine.search(&query)[0];

    engine.input(b"\x1b[?1049halternate").unwrap();
    engine.resize(size(12, 5)).unwrap();
    engine.resize(size(12, 2)).unwrap();
    engine.input(b"\x1b[?1049l").unwrap();

    let after = engine.search(&query)[0];
    assert_eq!(after.start.stable_row, before.start.stable_row);
}

#[test]
fn clear_scrollback_removes_history_from_later_resize_reconciliation() {
    let mut engine = saturated_scrollback_engine();
    let query = SearchQuery {
        needle: "line102".into(),
        case_sensitive: true,
        regex: false,
    };
    let before = engine.search(&query)[0];

    engine.input(b"\x1b7").unwrap();
    engine.clear_scrollback();
    engine.resize(size(12, 5)).unwrap();
    engine.input(b"\x1b8\r\nnew").unwrap();

    let after = engine.search(&query)[0];
    assert_eq!(after.start.stable_row, before.start.stable_row);
}

fn saturated_scrollback_engine() -> DefaultTerminalEngine {
    let mut engine = DefaultTerminalEngine::new(&profile(100), size(12, 3)).unwrap();
    let input = (0..104)
        .map(|index| format!("line{index:03}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    engine.input(input.as_bytes()).unwrap();
    let bounds = engine.viewport_bounds();
    assert_eq!(bounds.bottom_top_stable_row - bounds.first_stable_row, 100);
    engine
}

#[test]
fn clear_scrollback_keeps_visible_content_and_reset_restores_defaults() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    engine
        .input(b"old-line\r\nkeep-1\r\nkeep-2\r\nvisible")
        .unwrap();
    assert!(
        !engine
            .search(&SearchQuery {
                needle: "old-line".into(),
                case_sensitive: true,
                regex: false,
            })
            .is_empty()
    );

    engine.clear_scrollback();
    assert!(
        engine
            .search(&SearchQuery {
                needle: "old-line".into(),
                case_sensitive: true,
                regex: false,
            })
            .is_empty()
    );
    assert!(frame_text(&engine.snapshot(viewport(0, 3), None)).contains("visible"));

    engine
        .input(b"\x1b]2;changed\x07\x1b[?1003h\x1b[?1049hdirty")
        .unwrap();
    engine.reset();
    let reset = engine.snapshot(viewport(0, 3), None);
    assert_eq!(reset.title, "rsHell");
    assert!(!reset.mouse_reporting);
    assert!(!reset.alternate_screen);
    assert!(!frame_text(&reset).contains("dirty"));
}

#[test]
fn snapshot_selection_overlay_does_not_mutate_engine_state() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    engine.input(b"select me").unwrap();
    let range = SelectionRange {
        start: CellPosition {
            stable_row: 0,
            column: 0,
        },
        end: CellPosition {
            stable_row: 0,
            column: 5,
        },
        rectangular: false,
    };
    let selected = engine.snapshot(viewport(0, 3), Some(range));
    let plain = engine.snapshot(viewport(0, 3), None);
    assert_ne!(selected, plain);
    assert_eq!(plain, engine.snapshot(viewport(0, 3), None));
}

#[test]
fn wide_midpoint_selection_normalizes_to_the_stable_wide_cell_and_frame_overlay() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(8, 1)).unwrap();
    engine.input("A界B".as_bytes()).unwrap();
    let range = SelectionRange {
        start: CellPosition {
            stable_row: 0,
            column: 2,
        },
        end: CellPosition {
            stable_row: 0,
            column: 3,
        },
        rectangular: false,
    };

    assert_eq!(engine.selection_text(range), "界");
    let frame = engine.snapshot(viewport(0, 1), Some(range));
    let wide = frame.rows[0]
        .cells
        .iter()
        .find(|cell| cell.text == "界")
        .expect("wide cell");
    assert_eq!(wide.width, 2);
    assert!(wide.selected);
}

#[test]
fn rejects_zero_sized_terminal() {
    let error = DefaultTerminalEngine::new(&profile(1_000), size(0, 3)).unwrap_err();
    assert!(error.to_string().contains("non-zero"));
}

#[test]
fn unsupported_input_is_an_error_instead_of_a_silent_drop() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    let error = engine
        .encode_input(TerminalInput::Key {
            code: KeyCode::Character('x'),
            modifiers: KeyModifiers {
                super_key: true,
                ..KeyModifiers::default()
            },
        })
        .unwrap_err();

    assert!(error.to_string().contains("unsupported"));
}

#[test]
fn every_p0_key_and_supported_modifier_bit_uses_the_pinned_encoder() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    let mut keys = vec![
        KeyCode::Character('a'),
        KeyCode::Enter,
        KeyCode::Escape,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Insert,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::ArrowUp,
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
    ];
    keys.extend((1..=24).map(KeyCode::F));
    for code in keys {
        assert!(
            !engine
                .encode_input(key(code.clone(), KeyModifiers::default()))
                .unwrap_or_else(|error| panic!("{code:?} failed: {error}"))
                .is_empty(),
            "{code:?} encoded to no bytes"
        );
    }

    for modifiers in [
        KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        },
        control(),
        KeyModifiers {
            alt: true,
            ..KeyModifiers::default()
        },
        KeyModifiers {
            shift: true,
            control: true,
            alt: true,
            ..KeyModifiers::default()
        },
    ] {
        assert!(
            !engine
                .encode_input(key(KeyCode::Character('a'), modifiers))
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        engine
            .encode_input(TerminalInput::CommittedText("秘密".to_owned()))
            .unwrap(),
        "秘密".as_bytes()
    );
}

#[test]
fn keyboard_modes_follow_terminal_state_and_profile() {
    let mut default_engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    assert_eq!(
        default_engine
            .encode_input(key(KeyCode::ArrowUp, KeyModifiers::default()))
            .unwrap(),
        b"\x1b[A"
    );
    default_engine.advance(b"\x1b[?1h").unwrap();
    assert_eq!(
        default_engine
            .encode_input(key(KeyCode::ArrowUp, KeyModifiers::default()))
            .unwrap(),
        b"\x1bOA"
    );
    assert_eq!(
        DefaultTerminalEngine::new(&profile(1_000), size(20, 3))
            .unwrap()
            .encode_input(key(KeyCode::Character('['), control()))
            .unwrap(),
        b"\x1b"
    );
    assert_eq!(
        engine_with(|settings| settings.enable_csi_u = true)
            .encode_input(key(KeyCode::Character('['), control()))
            .unwrap(),
        b"\x1b[91;5u"
    );

    let mut kitty = engine_with(|settings| settings.enable_kitty_keyboard = true);
    kitty.advance(b"\x1b[>1u").unwrap();
    assert_eq!(kitty.advance(b"\x1b[?u").unwrap().outbound, b"\x1b[?1u");
    assert_eq!(
        kitty
            .encode_input(key(KeyCode::Character('a'), KeyModifiers::default()))
            .unwrap(),
        b"a"
    );
    assert_eq!(
        kitty.encode_input(key(KeyCode::Tab, control())).unwrap(),
        b"\x1b[9;5u"
    );
    assert_eq!(
        DefaultTerminalEngine::new(&profile(1_000), size(20, 3))
            .unwrap()
            .encode_input(key(
                KeyCode::Character('x'),
                KeyModifiers {
                    alt: true,
                    ..KeyModifiers::default()
                },
            ))
            .unwrap(),
        b"\x1bx"
    );
}

#[test]
fn configured_mouse_policy_can_disable_dynamic_reporting() {
    let mut allowed = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    assert!(!allowed.snapshot(viewport(0, 3), None).mouse_reporting);
    assert!(allowed.encode_mouse(left_press()).is_err());

    let mut disabled = engine_with(|settings| settings.mouse_reporting = false);
    disabled.advance(b"\x1b[?1002h\x1b[?1006h").unwrap();
    assert!(disabled.snapshot(viewport(0, 3), None).mouse_reporting);
    assert!(disabled.encode_mouse(left_press()).is_err());
}

#[test]
fn mouse_sgr_requires_tracking_and_sgr_negotiation() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    engine.input(b"\x1b[?1002h").unwrap();
    let tracking_only = engine.encode_mouse(left_press()).unwrap();
    assert!(!tracking_only.starts_with(b"\x1b[<"));

    engine.input(b"\x1b[?1006h").unwrap();
    assert_eq!(engine.encode_mouse(left_press()).unwrap(), b"\x1b[<0;4;2M");
}

#[test]
fn legacy_mouse_releases_use_code_three_while_sgr_keeps_button_codes() {
    let mut engine = DefaultTerminalEngine::new(&profile(1_000), size(20, 3)).unwrap();
    engine.input(b"\x1b[?1002h").unwrap();
    for (button, modifiers, expected) in [
        (MouseButton::Left, KeyModifiers::default(), b"\x1b[M#$\""),
        (
            MouseButton::Middle,
            KeyModifiers {
                shift: true,
                ..KeyModifiers::default()
            },
            b"\x1b[M'$\"",
        ),
        (
            MouseButton::Right,
            KeyModifiers {
                alt: true,
                control: true,
                ..KeyModifiers::default()
            },
            b"\x1b[M;$\"",
        ),
    ] {
        assert_eq!(
            engine
                .encode_mouse(mouse_event(MouseEventKind::Release, button, modifiers))
                .unwrap(),
            expected
        );
    }

    engine.input(b"\x1b[?1006h").unwrap();
    for (button, code) in [
        (MouseButton::Left, 0),
        (MouseButton::Middle, 1),
        (MouseButton::Right, 2),
    ] {
        assert_eq!(
            engine
                .encode_mouse(mouse_event(
                    MouseEventKind::Release,
                    button,
                    KeyModifiers::default(),
                ))
                .unwrap(),
            format!("\x1b[<{code};4;2m").as_bytes()
        );
    }
    assert_eq!(
        engine
            .encode_mouse(mouse_event(
                MouseEventKind::Release,
                MouseButton::Right,
                KeyModifiers {
                    alt: true,
                    control: true,
                    ..KeyModifiers::default()
                },
            ))
            .unwrap(),
        b"\x1b[<26;4;2m"
    );
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> TerminalInput {
    TerminalInput::Key { code, modifiers }
}

fn control() -> KeyModifiers {
    KeyModifiers {
        control: true,
        ..KeyModifiers::default()
    }
}

fn engine_with(change: impl FnOnce(&mut TerminalSettingsV1)) -> DefaultTerminalEngine {
    engine_with_size(size(20, 3), change)
}

fn engine_with_size(
    size: TerminalSize,
    change: impl FnOnce(&mut TerminalSettingsV1),
) -> DefaultTerminalEngine {
    let mut settings = TerminalSettingsV1::default();
    change(&mut settings);
    DefaultTerminalEngine::new(&settings.resolve(&TerminalOverrides::default()), size).unwrap()
}

fn left_press() -> TerminalMouseEvent {
    mouse_event(
        MouseEventKind::Press,
        MouseButton::Left,
        KeyModifiers::default(),
    )
}

fn repeated_rows_at_capacity() -> DefaultTerminalEngine {
    let mut engine = DefaultTerminalEngine::new(&profile(100), size(12, 3)).unwrap();
    for _ in 0..102 {
        engine.input(b"same\r\n").unwrap();
    }
    engine
}

fn assert_full_ring_rotations(rotations: usize) {
    const RING_LINES: usize = 103;

    let mut engine = repeated_rows_at_capacity();
    let before = engine.viewport_bounds();
    let output = b"same\r\n".repeat(RING_LINES * rotations);
    engine.input(&output).unwrap();
    let after = engine.viewport_bounds();

    assert_eq!(
        after.first_stable_row,
        before.first_stable_row + (RING_LINES * rotations) as i64
    );
    assert_eq!(
        after.bottom_top_stable_row,
        before.bottom_top_stable_row + (RING_LINES * rotations) as i64
    );
    assert!(after.first_stable_row > before.bottom_top_stable_row);
    let frame = engine.snapshot(viewport(after.bottom_top_stable_row, 3), None);
    assert_eq!(
        trimmed_rows(&frame),
        vec!["same".to_owned(), "same".to_owned(), String::new()]
    );
}

fn assert_crossing_bounds(
    engine: &DefaultTerminalEngine,
    before: rshell_session::ViewportBounds,
    origin_advance: i64,
    first_advance: i64,
) {
    let after = engine.viewport_bounds();
    assert_eq!(
        after.bottom_top_stable_row,
        before.bottom_top_stable_row + origin_advance
    );
    assert_eq!(
        after.first_stable_row,
        before.first_stable_row + first_advance
    );
    assert!(after.first_stable_row > before.bottom_top_stable_row);
    let frame = engine.snapshot(viewport(after.bottom_top_stable_row, 3), None);
    assert_eq!(
        trimmed_rows(&frame),
        vec!["same".to_owned(), "same".to_owned(), String::new()]
    );
}

fn assert_saturated_scroll(
    engine: &DefaultTerminalEngine,
    before: rshell_session::ViewportBounds,
    shift: i64,
) {
    let after = engine.viewport_bounds();
    assert_eq!(
        after.bottom_top_stable_row,
        before.bottom_top_stable_row + shift
    );
    assert_eq!(after.first_stable_row, before.first_stable_row + shift);
}

fn mouse_event(
    kind: MouseEventKind,
    button: MouseButton,
    modifiers: KeyModifiers,
) -> TerminalMouseEvent {
    TerminalMouseEvent {
        kind,
        button: Some(button),
        cell: CellPosition {
            stable_row: 101,
            column: 3,
        },
        viewport_row: 1,
        pixel_x: 24,
        pixel_y: 32,
        modifiers,
    }
}

fn trimmed_rows(frame: &RenderFrame) -> Vec<String> {
    frame
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn terminal_engine_canary_verifies_exact_crlf_rows_before_candidate_hashing() {
    const ROWS: usize = 1000;
    const VIEWPORT_ROWS: u16 = 40;
    let expected = (0..ROWS)
        .map(|index| format!("scrollback-{index:04}"))
        .collect::<Vec<_>>();
    let mut input = Vec::new();
    for label in &expected {
        input.extend_from_slice(label.as_bytes());
        input.extend_from_slice(b"\r\n");
    }
    assert!(input.ends_with(b"\r\n"));
    assert_eq!(
        input.windows(2).filter(|bytes| *bytes == b"\r\n").count(),
        ROWS
    );
    assert!(
        input
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte != b'\n' || input.get(index.wrapping_sub(1)) == Some(&b'\r'))
    );

    let mut engine = DefaultTerminalEngine::new(&profile(2_000), size(120, VIEWPORT_ROWS)).unwrap();
    engine.input(&input).unwrap();
    let bounds = engine.viewport_bounds();
    let mut top = bounds.first_stable_row;
    let mut seen = BTreeSet::new();
    let mut rendered = Vec::with_capacity(ROWS + 1);
    let cursor_row = loop {
        let frame = engine.snapshot(viewport(top, VIEWPORT_ROWS), None);
        assert_eq!(frame.rows.len(), usize::from(VIEWPORT_ROWS));
        for row in frame.rows.iter() {
            if seen.insert(row.stable_row) {
                let raw = row
                    .cells
                    .iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>();
                let text = if raw.bytes().all(|byte| byte == b' ') {
                    String::new()
                } else {
                    raw.trim_end_matches(' ').to_owned()
                };
                rendered.push((row.stable_row, text));
            }
        }
        if top == bounds.bottom_top_stable_row {
            break frame.cursor.unwrap().position.stable_row;
        }
        top = top
            .saturating_add(i64::from(VIEWPORT_ROWS))
            .min(bounds.bottom_top_stable_row);
    };

    assert_eq!(rendered.len(), ROWS + 1);
    let trailing = rendered.pop().unwrap();
    assert_eq!(trailing.0, cursor_row);
    assert!(trailing.1.is_empty());
    let actual = rendered
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "rendered row equality is the hash oracle");

    let expected_canonical = expected.join("\n").into_bytes();
    let actual_canonical = actual.join("\n").into_bytes();
    assert!(!actual_canonical.ends_with(b"\n"));
    assert_eq!(actual_canonical, expected_canonical);
    let candidate = format!("{:x}", Sha256::digest(&actual_canonical));
    assert_eq!(
        candidate,
        format!("{:x}", Sha256::digest(&expected_canonical))
    );
    assert_eq!(candidate.len(), 64);
    assert!(
        candidate
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    let fixture_digest = CANARY_FIXTURE
        .lines()
        .find_map(|line| line.trim().strip_prefix("\"sha256\":"))
        .unwrap()
        .trim()
        .trim_end_matches(',');
    if fixture_digest != "null" {
        assert_eq!(fixture_digest.trim_matches('"'), candidate);
    }
}

#[test]
fn snapshots_are_arc_backed_immutable_values() {
    let engine = DefaultTerminalEngine::new(&profile(1_000), size(10, 2)).unwrap();
    let frame: Arc<RenderFrame> = engine.snapshot(viewport(0, 2), None);
    assert_eq!(Arc::strong_count(&frame), 1);
}
