use std::ops::Range;

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line},
    term::cell::Flags,
};
use regex::RegexBuilder;
use rshell_core::{CellPosition, SearchMatch, SearchQuery, SelectionRange};

use crate::alacritty_adapter::AlacrittyAdapter;

struct TextCell {
    column: usize,
    width: usize,
    byte_range: Range<usize>,
}

struct TextRow {
    stable_row: i64,
    wrapped: bool,
    text: String,
    cells: Vec<TextCell>,
}

pub(crate) fn search(adapter: &AlacrittyAdapter, query: &SearchQuery) -> Vec<SearchMatch> {
    if query.needle.is_empty() {
        return Vec::new();
    }
    let pattern = if query.regex {
        query.needle.clone()
    } else {
        regex::escape(&query.needle)
    };
    let Ok(regex) = RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .build()
    else {
        return Vec::new();
    };

    let mut matches = Vec::new();
    for row in all_rows(adapter) {
        for found in regex.find_iter(&row.text) {
            matches.push(SearchMatch {
                start: CellPosition {
                    stable_row: row.stable_row,
                    column: to_u16(byte_to_column(&row, found.start(), false)),
                },
                end: CellPosition {
                    stable_row: row.stable_row,
                    column: to_u16(byte_to_column(&row, found.end(), true)),
                },
            });
        }
    }
    matches
}

pub(crate) fn selection_text(adapter: &AlacrittyAdapter, range: SelectionRange) -> String {
    let (start, end) = ordered(range.start, range.end);
    if start == end {
        return String::new();
    }
    let mut result = String::new();
    let mut previous_wrapped = true;
    for row in all_rows(adapter)
        .into_iter()
        .filter(|row| row.stable_row >= start.stable_row && row.stable_row <= end.stable_row)
    {
        if !result.is_empty() && !previous_wrapped {
            result.push('\n');
        }
        if let Some(columns) = selection_columns(range, row.stable_row, usize::MAX) {
            result.push_str(&columns_text(&row, columns));
        }
        previous_wrapped = row.wrapped;
    }
    result
}

pub(crate) fn selection_columns(
    range: SelectionRange,
    stable_row: i64,
    columns: usize,
) -> Option<Range<usize>> {
    let (start, end) = ordered(range.start, range.end);
    if stable_row < start.stable_row || stable_row > end.stable_row || start == end {
        return None;
    }
    if range.rectangular {
        let left = usize::from(range.start.column.min(range.end.column));
        let right = usize::from(range.start.column.max(range.end.column)).min(columns);
        return (left < right).then_some(left..right);
    }
    let left = if stable_row == start.stable_row {
        usize::from(start.column)
    } else {
        0
    };
    let right = if stable_row == end.stable_row {
        usize::from(end.column)
    } else {
        columns
    }
    .min(columns);
    (left < right).then_some(left..right)
}

fn all_rows(adapter: &AlacrittyAdapter) -> Vec<TextRow> {
    let grid = adapter.terminal().grid();
    (grid.topmost_line().0..=grid.bottommost_line().0)
        .map(|line| {
            let line = Line(line);
            project_line(&grid[line], adapter.stable_row(line), grid.columns())
        })
        .collect()
}

fn project_line(
    row: &alacritty_terminal::grid::Row<alacritty_terminal::term::cell::Cell>,
    stable_row: i64,
    columns: usize,
) -> TextRow {
    let mut text = String::new();
    let mut cells = Vec::new();
    let mut column = 0;
    while column < columns {
        let cell = &row[Column(column)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            column += 1;
            continue;
        }
        let start = text.len();
        text.push(cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            text.extend(zerowidth);
        }
        let width = if cell.flags.contains(Flags::WIDE_CHAR) {
            2.min(columns - column)
        } else {
            1
        };
        cells.push(TextCell {
            column,
            width,
            byte_range: start..text.len(),
        });
        column += width;
    }
    let trimmed_len = text.trim_end_matches(' ').len();
    text.truncate(trimmed_len);
    cells.retain(|cell| cell.byte_range.start < trimmed_len);
    TextRow {
        stable_row,
        wrapped: row[Column(columns - 1)].flags.contains(Flags::WRAPLINE),
        text,
        cells,
    }
}

fn byte_to_column(row: &TextRow, byte: usize, end: bool) -> usize {
    row.cells
        .iter()
        .find(|cell| {
            if end {
                byte <= cell.byte_range.end && byte > cell.byte_range.start
            } else {
                byte >= cell.byte_range.start && byte < cell.byte_range.end
            }
        })
        .map(|cell| cell.column + usize::from(end) * cell.width)
        .unwrap_or_else(|| row.cells.last().map_or(0, |cell| cell.column + cell.width))
}

fn columns_text(row: &TextRow, columns: Range<usize>) -> String {
    let mut selected = String::new();
    for cell in &row.cells {
        if columns.start < cell.column + cell.width && cell.column < columns.end {
            selected.push_str(&row.text[cell.byte_range.clone()]);
        }
    }
    selected.trim_end_matches(' ').into()
}

fn ordered(first: CellPosition, second: CellPosition) -> (CellPosition, CellPosition) {
    if (first.stable_row, first.column) <= (second.stable_row, second.column) {
        (first, second)
    } else {
        (second, first)
    }
}

fn to_u16(column: usize) -> u16 {
    column.min(usize::from(u16::MAX)) as u16
}
