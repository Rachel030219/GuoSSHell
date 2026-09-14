use std::sync::Arc;

use alacritty_terminal::{
    grid::Dimensions,
    index::Column,
    term::cell::{Cell, Flags},
    vte::ansi::{Color as VteColor, CursorShape as VteCursorShape},
};
use rshell_core::{
    CellAttributes, CellPosition, Color, CursorShape, RenderCell, RenderCursor, RenderFrame,
    RenderRow, SelectionRange, Viewport,
};

use crate::{alacritty_adapter::AlacrittyAdapter, text::selection_columns};

pub(crate) fn snapshot(
    adapter: &AlacrittyAdapter,
    viewport: Viewport,
    selection: Option<SelectionRange>,
) -> RenderFrame {
    let terminal = adapter.terminal();
    let grid = terminal.grid();
    let start = adapter.viewport_bounds().clamp_top(viewport.top_stable_row);
    let mut rows = Vec::with_capacity(usize::from(viewport.rows));
    for offset in 0..i64::from(viewport.rows) {
        let stable_row = start.saturating_add(offset);
        let Some(line) = adapter.backend_line(stable_row) else {
            break;
        };
        rows.push(convert_row(
            &grid[line],
            stable_row,
            grid.columns(),
            selection,
        ));
    }

    let cursor = grid.cursor.point;
    let cursor_shape = terminal.cursor_style().shape;
    let display_modes = adapter.display_modes();
    RenderFrame {
        generation: 0,
        size: adapter.size(),
        viewport_top: start,
        rows: Arc::from(rows),
        cursor: Some(RenderCursor {
            position: CellPosition {
                stable_row: adapter.stable_row(cursor.line),
                column: cursor.column.0.min(usize::from(u16::MAX)) as u16,
            },
            shape: convert_cursor_shape(cursor_shape),
            visible: !display_modes.cursor_hidden,
        }),
        title: adapter.title(),
        display_modes,
        alternate_screen: display_modes.alternate_screen,
        mouse_reporting: display_modes.mouse_reporting,
    }
}

fn convert_row(
    row: &alacritty_terminal::grid::Row<Cell>,
    stable_row: i64,
    columns: usize,
    selection: Option<SelectionRange>,
) -> RenderRow {
    let selected = selection.and_then(|range| selection_columns(range, stable_row, columns));
    let mut cells = Vec::with_capacity(columns);
    let mut column = 0;
    while column < columns {
        let cell = &row[Column(column)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            column += 1;
            continue;
        }
        if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            cells.push(blank_cell(is_selected(selected.clone(), column, 1)));
            column += 1;
            continue;
        }
        let width = if cell.flags.contains(Flags::WIDE_CHAR) {
            2.min(columns - column)
        } else {
            1
        };
        cells.push(convert_cell(
            cell,
            width,
            is_selected(selected.clone(), column, width),
        ));
        column += width;
    }
    RenderRow {
        stable_row,
        wrapped: row[Column(columns - 1)].flags.contains(Flags::WRAPLINE),
        cells: Arc::from(cells),
    }
}

fn convert_cell(cell: &Cell, width: usize, selected: bool) -> RenderCell {
    let mut text = String::from(cell.c);
    if let Some(zerowidth) = cell.zerowidth() {
        text.extend(zerowidth);
    }
    RenderCell {
        text,
        width: width as u8,
        foreground: convert_color(cell.fg),
        background: convert_color(cell.bg),
        attributes: CellAttributes {
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            strike: cell.flags.contains(Flags::STRIKEOUT),
            reverse: cell.flags.contains(Flags::INVERSE),
        },
        selected,
    }
}

fn blank_cell(selected: bool) -> RenderCell {
    RenderCell {
        text: " ".into(),
        width: 1,
        foreground: Color::Default,
        background: Color::Default,
        attributes: CellAttributes::default(),
        selected,
    }
}

fn is_selected(selected: Option<std::ops::Range<usize>>, column: usize, width: usize) -> bool {
    selected.is_some_and(|range| range.start < column + width && column < range.end)
}

fn convert_color(color: VteColor) -> Color {
    match color {
        VteColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        VteColor::Indexed(index) => Color::Ansi(index),
        VteColor::Named(named) if (named as usize) < 16 => Color::Ansi(named as u8),
        VteColor::Named(_) => Color::Default,
    }
}

fn convert_cursor_shape(shape: VteCursorShape) -> CursorShape {
    match shape {
        VteCursorShape::Underline => CursorShape::Underline,
        VteCursorShape::Beam => CursorShape::Beam,
        VteCursorShape::Block | VteCursorShape::HollowBlock | VteCursorShape::Hidden => {
            CursorShape::Block
        }
    }
}
