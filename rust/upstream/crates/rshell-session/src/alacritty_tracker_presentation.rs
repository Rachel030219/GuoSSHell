use crate::alacritty_tracker::Csi;
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy)]
pub(super) struct Presentation {
    columns: usize,
    lines: usize,
    row: usize,
    column: usize,
    wrap: bool,
    top: usize,
    bottom: usize,
    saved: CursorState,
}

#[derive(Clone, Copy, Default)]
struct CursorState {
    row: usize,
    column: usize,
    wrap: bool,
}

impl Presentation {
    pub(super) fn new(columns: usize, lines: usize) -> Self {
        Self {
            columns: columns.max(1),
            lines: lines.max(1),
            row: 0,
            column: 0,
            wrap: false,
            top: 0,
            bottom: lines.max(1),
            saved: CursorState::default(),
        }
    }

    pub(super) fn resize(
        &mut self,
        columns: usize,
        lines: usize,
        available_history: usize,
    ) -> usize {
        let lines = lines.max(1);
        let pulled_from_history = available_history.min(lines.saturating_sub(self.lines));
        let primary_scrolls = (self.row + 1).saturating_sub(lines);
        self.columns = columns.max(1);
        self.lines = lines;
        self.top = 0;
        self.bottom = self.lines;
        self.sync_cursor(
            self.row.saturating_add(pulled_from_history),
            self.column,
            self.wrap,
        );
        self.saved.row = self.saved.row.saturating_add(pulled_from_history);
        self.saved = self.clamped(self.saved);
        primary_scrolls
    }

    pub(super) fn sync_cursor(&mut self, row: usize, column: usize, wrap: bool) {
        self.row = row.min(self.lines - 1);
        self.column = column.min(self.columns - 1);
        self.wrap = wrap;
    }

    pub(super) fn save_cursor(&mut self) {
        self.saved = CursorState {
            row: self.row,
            column: self.column,
            wrap: self.wrap,
        };
    }

    pub(super) fn restore_cursor(&mut self) {
        let saved = self.clamped(self.saved);
        self.sync_cursor(saved.row, saved.column, saved.wrap);
    }

    pub(super) fn print_char(&mut self, character: char, primary: bool) -> usize {
        character
            .width()
            .map_or(0, |width| self.print_width(width, primary))
    }

    pub(super) fn print_width(&mut self, width: usize, primary: bool) -> usize {
        if width == 0 {
            return 0;
        }
        if width == 1 {
            return self.print_many(1, primary);
        }
        let mut shift = usize::from(self.wrapline(primary));
        if self.columns < 2 || self.column + 1 >= self.columns {
            shift += usize::from(self.line_wrap(primary));
        }
        if self.column + 2 < self.columns {
            self.column += 2;
        } else {
            self.column = self.columns - 1;
            self.wrap = true;
        }
        shift
    }

    pub(super) fn print_many(&mut self, mut count: usize, primary: bool) -> usize {
        let mut shift = 0;
        while count != 0 {
            shift += usize::from(self.wrapline(primary));
            let available = self.columns - self.column;
            if count < available {
                self.column += count;
                break;
            }
            count -= available;
            self.column = self.columns - 1;
            self.wrap = true;
        }
        shift
    }

    pub(super) fn linefeed(&mut self, primary: bool) -> usize {
        if self.row + 1 == self.bottom {
            return usize::from(self.scrolls_primary(primary));
        }
        if self.row + 1 < self.lines {
            self.row += 1;
        }
        0
    }

    pub(super) fn newline(&mut self, primary: bool) -> usize {
        let shift = self.linefeed(primary);
        self.carriage_return();
        shift
    }

    pub(super) fn carriage_return(&mut self) {
        self.column = 0;
        self.wrap = false;
    }

    pub(super) fn backspace(&mut self) {
        if self.column != 0 {
            self.column -= 1;
            self.wrap = false;
        }
    }

    pub(super) fn tab(&mut self, primary: bool) -> usize {
        if self.wrap {
            return usize::from(self.wrapline(primary));
        }
        self.column = self.columns - 1;
        0
    }

    pub(super) fn csi(&mut self, final_byte: u8, csi: Csi, primary: bool) -> usize {
        let amount = csi.parameter(0, 1);
        let shift = match final_byte {
            b'A' => self.goto(self.row.saturating_sub(amount), self.column),
            b'B' => self.goto(
                self.row.saturating_add(amount).min(self.lines - 1),
                self.column,
            ),
            b'C' => self.goto(
                self.row,
                self.column.saturating_add(amount).min(self.columns - 1),
            ),
            b'D' => self.goto(self.row, self.column.saturating_sub(amount)),
            b'E' => self.goto(self.row.saturating_add(amount).min(self.lines - 1), 0),
            b'F' => self.goto(self.row.saturating_sub(amount), 0),
            b'G' => self.goto(self.row, (amount - 1).min(self.columns - 1)),
            b'H' | b'f' => self.goto(
                (amount - 1).min(self.lines - 1),
                (csi.parameter(1, 1) - 1).min(self.columns - 1),
            ),
            b'M' if self.row == 0 && self.full_margin() => amount.min(self.lines),
            b'S' if self.full_margin() => amount.min(self.bottom - self.top),
            b'r' => self.margins(csi),
            b's' => self.save_cursor_return(),
            b'u' => self.restore_cursor_return(),
            _ => 0,
        };
        shift * usize::from(primary)
    }

    fn wrapline(&mut self, primary: bool) -> bool {
        if !self.wrap {
            return false;
        }
        self.line_wrap(primary)
    }

    fn line_wrap(&mut self, primary: bool) -> bool {
        let scroll = if self.row + 1 >= self.bottom {
            self.scrolls_primary(primary)
        } else {
            self.row += 1;
            false
        };
        self.column = 0;
        self.wrap = false;
        scroll
    }

    fn margins(&mut self, csi: Csi) -> usize {
        let top = csi.parameter(0, 1).min(self.lines);
        let bottom = csi.parameter(1, self.lines).min(self.lines);
        if top < bottom {
            self.top = top - 1;
            self.bottom = bottom;
            self.goto(0, 0);
        }
        0
    }

    fn goto(&mut self, row: usize, column: usize) -> usize {
        self.row = row;
        self.column = column;
        self.wrap = false;
        0
    }

    fn save_cursor_return(&mut self) -> usize {
        self.save_cursor();
        0
    }

    fn restore_cursor_return(&mut self) -> usize {
        self.restore_cursor();
        0
    }

    fn clamped(&self, cursor: CursorState) -> CursorState {
        CursorState {
            row: cursor.row.min(self.lines - 1),
            column: cursor.column.min(self.columns - 1),
            wrap: cursor.wrap,
        }
    }

    fn full_margin(&self) -> bool {
        self.top == 0 && self.bottom == self.lines
    }

    fn scrolls_primary(&self, primary: bool) -> bool {
        primary && self.full_margin()
    }
}
