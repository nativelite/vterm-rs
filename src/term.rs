//! The terminal emulator: bytes in, a live [`ansi::Screen`] out.
//!
//! [`Term`] holds a primary and an alternate [`ansi::Screen`], a cursor, the
//! current SGR [`ansi::Style`], a scroll region, tab stops, a saved-cursor
//! slot, and the flags a VT100 tracks (autowrap, cursor visibility, and the
//! pending-wrap latch at the right edge). [`Term::feed`] runs the child's
//! output through an internal [`ansi::Parser`] and applies each token.
//!
//! Coordinates are 0-based internally; CSI parameters are 1-based on the wire
//! and converted on the way in. Where VT defaults a missing parameter to 1
//! (cursor motion, CUP), we treat a 0 from the parser as that default; where
//! it defaults to 0 (erase/SGR selectors), 0 is used directly.

use ansi::{Cell, Parser, Screen, Style, Token};

/// A minimal VT100/ECMA-48 terminal emulator over an [`ansi::Screen`].
pub struct Term {
    /// The primary (normal) screen buffer.
    primary: Screen,
    /// The alternate screen buffer, used while an app enters alt-screen mode
    /// (`?1049h`/`?47h`/`?1047h`). Kept the same size as `primary`.
    alt: Screen,
    /// True while the alternate buffer is active.
    in_alt: bool,
    /// Cursor position, 0-based `(row, col)`.
    row: usize,
    col: usize,
    /// Current SGR style applied to newly written cells.
    style: Style,
    /// Scroll region, 0-based inclusive `[top, bottom]` (DECSTBM).
    scroll_top: usize,
    scroll_bottom: usize,
    /// Saved cursor state (DECSC/`ESC 7` and CSI `s`): row, col, style.
    saved: Option<(usize, usize, Style)>,
    /// Tab stops: `tabs[c]` true means a tab stop at column `c`.
    tabs: Vec<bool>,
    /// Pending-wrap latch: set after writing the last column so the *next*
    /// printable wraps to the next line first (DECAWM deferred-wrap).
    wrap_pending: bool,
    /// Autowrap mode (DECAWM). Default on.
    autowrap: bool,
    /// Cursor visibility (DECTCEM, `?25h`/`?25l`). Tracked for the host; the
    /// screen cursor position is always synced regardless.
    cursor_visible: bool,
    /// The parser driving `feed`; one per stream so mid-sequence chunk splits
    /// are handled by `ansi`.
    parser: Parser,
}

impl Term {
    /// A blank emulator of `rows x cols` (both clamped to at least 1), cursor
    /// at the origin, full-height scroll region, default tab stops every 8
    /// columns, autowrap on, cursor visible.
    pub fn new(rows: usize, cols: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Term {
            primary: Screen::new(rows, cols),
            alt: Screen::new(rows, cols),
            in_alt: false,
            row: 0,
            col: 0,
            style: Style::default(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            saved: None,
            tabs: default_tabs(cols),
            wrap_pending: false,
            autowrap: true,
            cursor_visible: true,
            parser: Parser::new(),
        }
    }

    /// Feed a chunk of child output. Chunks may split at any byte boundary
    /// (mid-escape, mid-UTF-8); the internal parser holds partial state across
    /// calls, so the final screen is independent of how bytes were chunked.
    pub fn feed(&mut self, bytes: &[u8]) {
        let tokens = self.parser.feed(bytes);
        for tok in tokens {
            self.apply(&tok);
        }
    }

    /// The active buffer (alternate while in alt-screen, else primary), with
    /// its public `cursor` field synced to the emulator's position so a
    /// compositor / `Screen::diff` parks the cursor correctly.
    pub fn screen(&self) -> &Screen {
        // Sync happens in `sync_cursor`, called after every `apply`. This is a
        // read-only accessor; the cursor is already current.
        self.active()
    }

    /// Resize to `rows x cols` (clamped to at least 1). For 0.1 this rebuilds
    /// fresh buffers of the new size (the caller repaints); the cursor and
    /// scroll region are clamped, tab stops rebuilt, and pending wrap cleared.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let old_primary = std::mem::replace(&mut self.primary, Screen::new(rows, cols));
        let old_alt = std::mem::replace(&mut self.alt, Screen::new(rows, cols));
        copy_top_left(&old_primary, &mut self.primary);
        copy_top_left(&old_alt, &mut self.alt);
        self.scroll_top = self.scroll_top.min(rows - 1);
        self.scroll_bottom = rows - 1;
        self.row = self.row.min(rows - 1);
        self.col = self.col.min(cols - 1);
        self.tabs = default_tabs(cols);
        self.wrap_pending = false;
        self.sync_cursor();
    }

    // --- internals --------------------------------------------------------

    fn active(&self) -> &Screen {
        if self.in_alt {
            &self.alt
        } else {
            &self.primary
        }
    }

    fn active_mut(&mut self) -> &mut Screen {
        if self.in_alt {
            &mut self.alt
        } else {
            &mut self.primary
        }
    }

    fn rows(&self) -> usize {
        self.active().rows()
    }

    fn cols(&self) -> usize {
        self.active().cols()
    }

    /// Mirror the emulator cursor into the active screen's public field.
    fn sync_cursor(&mut self) {
        let (r, c) = (self.row, self.col);
        self.active_mut().cursor = (r, c);
    }

    fn apply(&mut self, tok: &Token) {
        match tok {
            Token::Text(s) => {
                for ch in s.chars() {
                    self.print(ch);
                }
            }
            Token::Control(b) => self.control(*b),
            Token::Csi {
                private,
                params,
                final_byte,
                ..
            } => self.csi(*private, params, *final_byte),
            Token::Esc { final_byte, .. } => self.esc(*final_byte),
            // Titles, DCS/SOS/PM/APC strings: nothing to render.
            Token::Osc(_) | Token::Other { .. } => {}
        }
        self.sync_cursor();
    }

    /// Write one printable character at the cursor, honoring autowrap via the
    /// pending-wrap latch.
    fn print(&mut self, ch: char) {
        let cols = self.cols();
        if self.wrap_pending && self.autowrap {
            // The previous write filled the last column; wrap now.
            self.col = 0;
            self.line_feed();
            self.wrap_pending = false;
        }
        let (r, c) = (self.row, self.col);
        let cell = Cell {
            ch,
            style: self.style,
        };
        self.active_mut().set(r, c, cell);
        if self.col + 1 >= cols {
            // At the right edge: stay put and latch a pending wrap (deferred to
            // the next printable), matching real terminals and the `ansi`
            // diff's pending-wrap comment.
            if self.autowrap {
                self.wrap_pending = true;
            }
            // With autowrap off, the cursor sticks at the last column.
        } else {
            self.col += 1;
        }
    }

    fn control(&mut self, b: u8) {
        match b {
            b'\r' => {
                self.col = 0;
                self.wrap_pending = false;
            }
            b'\n' | 0x0b | 0x0c => {
                // LF, VT, FF: move down one line, scrolling at the margin.
                self.line_feed();
                self.wrap_pending = false;
            }
            0x08 => {
                // BS: cursor left one column, clamped at column 0.
                if self.col > 0 {
                    self.col -= 1;
                }
                self.wrap_pending = false;
            }
            b'\t' => {
                self.col = self.next_tab_stop();
                self.wrap_pending = false;
            }
            _ => {} // BEL and other C0 controls: nothing to render.
        }
    }

    /// Move the cursor down one line; if it is at the bottom margin, scroll the
    /// scroll region up by one instead.
    fn line_feed(&mut self) {
        if self.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.row + 1 < self.rows() {
            self.row += 1;
        }
    }

    /// Next tab stop strictly right of the cursor, or the last column.
    fn next_tab_stop(&self) -> usize {
        let cols = self.cols();
        let mut c = self.col + 1;
        while c < cols {
            if self.tabs.get(c).copied().unwrap_or(false) {
                return c;
            }
            c += 1;
        }
        cols - 1
    }

    // --- CSI dispatch -----------------------------------------------------

    fn csi(&mut self, private: Option<char>, params: &[u16], final_byte: char) {
        // Private-marker sequences (`?...`) are the mode toggles.
        if private == Some('?') {
            match final_byte {
                'h' => self.set_mode(params, true),
                'l' => self.set_mode(params, false),
                _ => {} // other private CSI: ignore quietly
            }
            return;
        }
        if private.is_some() {
            return; // `>`/`<`/`=` device sequences: ignore quietly
        }

        // Helpers: VT motion params default to 1 (0 from the parser means the
        // param was absent → use 1). Erase/line selectors default to 0.
        let n1 = |i: usize| -> usize { at1(params, i) };

        match final_byte {
            // Cursor absolute position: CUP / HVP (row;col, 1-based).
            'H' | 'f' => {
                let r = n1(0).saturating_sub(1);
                let c = n1(1).saturating_sub(1);
                self.move_to(r, c);
            }
            'A' => self.move_up(n1(0)),
            'B' => self.move_down(n1(0)),
            'C' => self.move_right(n1(0)),
            'D' => self.move_left(n1(0)),
            'E' => {
                // CNL: cursor next line, column 0.
                self.move_down(n1(0));
                self.col = 0;
                self.wrap_pending = false;
            }
            'F' => {
                // CPL: cursor previous line, column 0.
                self.move_up(n1(0));
                self.col = 0;
                self.wrap_pending = false;
            }
            'G' => {
                // CHA: cursor to absolute column (1-based).
                let c = n1(0).saturating_sub(1);
                self.col = c.min(self.cols() - 1);
                self.wrap_pending = false;
            }
            'd' => {
                // VPA: cursor to absolute row (1-based).
                let r = n1(0).saturating_sub(1);
                self.row = r.min(self.rows() - 1);
                self.wrap_pending = false;
            }
            'J' => self.erase_display(at0(params, 0)),
            'K' => self.erase_line(at0(params, 0)),
            'm' => self.style.apply_sgr(params),
            'r' => self.set_scroll_region(params),
            'L' => self.insert_lines(n1(0)),
            'M' => self.delete_lines(n1(0)),
            '@' => self.insert_chars(n1(0)),
            'P' => self.delete_chars(n1(0)),
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            _ => {} // unimplemented CSI: ignore quietly
        }
    }

    fn set_mode(&mut self, params: &[u16], set: bool) {
        for &p in params {
            match p {
                25 => self.cursor_visible = set,
                7 => self.autowrap = set, // DECAWM
                47 | 1047 | 1049 => self.set_alt(set),
                _ => {} // other private modes: ignore quietly
            }
        }
    }

    fn set_alt(&mut self, enter: bool) {
        if enter && !self.in_alt {
            // Entering: switch to a cleared alternate buffer.
            self.alt = Screen::new(self.rows(), self.cols());
            self.in_alt = true;
        } else if !enter && self.in_alt {
            // Leaving: restore the primary buffer as-is.
            self.in_alt = false;
        }
        self.wrap_pending = false;
    }

    // --- cursor motion (clamped) -----------------------------------------

    fn move_to(&mut self, r: usize, c: usize) {
        self.row = r.min(self.rows() - 1);
        self.col = c.min(self.cols() - 1);
        self.wrap_pending = false;
    }

    fn move_up(&mut self, n: usize) {
        self.row = self.row.saturating_sub(n);
        self.wrap_pending = false;
    }

    fn move_down(&mut self, n: usize) {
        self.row = (self.row + n).min(self.rows() - 1);
        self.wrap_pending = false;
    }

    fn move_right(&mut self, n: usize) {
        self.col = (self.col + n).min(self.cols() - 1);
        self.wrap_pending = false;
    }

    fn move_left(&mut self, n: usize) {
        self.col = self.col.saturating_sub(n);
        self.wrap_pending = false;
    }

    // --- erase ------------------------------------------------------------

    /// ED: 0 = cursor to end of screen, 1 = start of screen to cursor, 2 = all.
    fn erase_display(&mut self, mode: u16) {
        let (rows, cols) = (self.rows(), self.cols());
        let (cr, cc) = (self.row, self.col);
        let blank = Cell::default();
        match mode {
            0 => {
                for c in cc..cols {
                    self.active_mut().set(cr, c, blank);
                }
                for r in (cr + 1)..rows {
                    for c in 0..cols {
                        self.active_mut().set(r, c, blank);
                    }
                }
            }
            1 => {
                for r in 0..cr {
                    for c in 0..cols {
                        self.active_mut().set(r, c, blank);
                    }
                }
                for c in 0..=cc {
                    self.active_mut().set(cr, c, blank);
                }
            }
            2 => {
                for r in 0..rows {
                    for c in 0..cols {
                        self.active_mut().set(r, c, blank);
                    }
                }
            }
            _ => {}
        }
    }

    /// EL: 0 = cursor to end of line, 1 = start of line to cursor, 2 = whole line.
    fn erase_line(&mut self, mode: u16) {
        let cols = self.cols();
        let (cr, cc) = (self.row, self.col);
        let blank = Cell::default();
        let range = match mode {
            0 => cc..cols,
            1 => 0..(cc + 1).min(cols),
            2 => 0..cols,
            _ => return,
        };
        for c in range {
            self.active_mut().set(cr, c, blank);
        }
    }

    // --- scroll region + line/char insert/delete --------------------------

    /// DECSTBM: set scroll region `top;bottom` (1-based, clamped); resets the
    /// cursor to the origin. An empty/`0` bottom means the last row.
    fn set_scroll_region(&mut self, params: &[u16]) {
        let rows = self.rows();
        let top = at1(params, 0).saturating_sub(1);
        let bottom = match at0(params, 1) {
            0 => rows - 1,
            b => (b as usize - 1).min(rows - 1),
        };
        // Ignore an inverted/empty region, as VT100 does.
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        }
        self.row = 0;
        self.col = 0;
        self.wrap_pending = false;
    }

    /// Scroll the scroll region up by `n` lines: top lines lost, blanks in at
    /// the bottom margin.
    fn scroll_up(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let cols = self.cols();
        let n = n.min(bottom - top + 1);
        for r in top..=bottom {
            for c in 0..cols {
                let cell = if r + n <= bottom {
                    self.active().cell(r + n, c)
                } else {
                    Cell::default()
                };
                self.active_mut().set(r, c, cell);
            }
        }
    }

    /// Scroll the scroll region down by `n` lines: bottom lines lost, blanks in
    /// at the top margin.
    fn scroll_down(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let cols = self.cols();
        let n = n.min(bottom - top + 1);
        for r in (top..=bottom).rev() {
            for c in 0..cols {
                let cell = if r >= top + n {
                    self.active().cell(r - n, c)
                } else {
                    Cell::default()
                };
                self.active_mut().set(r, c, cell);
            }
        }
    }

    /// IL: insert `n` blank lines at the cursor row, within the scroll region;
    /// lines below shift down and fall off the bottom margin. No-op outside the
    /// region.
    fn insert_lines(&mut self, n: usize) {
        if self.row < self.scroll_top || self.row > self.scroll_bottom {
            return;
        }
        let saved_top = self.scroll_top;
        self.scroll_top = self.row;
        self.scroll_down(n);
        self.scroll_top = saved_top;
        self.col = 0;
        self.wrap_pending = false;
    }

    /// DL: delete `n` lines at the cursor row, within the scroll region; lines
    /// below shift up and blanks fill the bottom margin. No-op outside region.
    fn delete_lines(&mut self, n: usize) {
        if self.row < self.scroll_top || self.row > self.scroll_bottom {
            return;
        }
        let saved_top = self.scroll_top;
        self.scroll_top = self.row;
        self.scroll_up(n);
        self.scroll_top = saved_top;
        self.col = 0;
        self.wrap_pending = false;
    }

    /// ICH: insert `n` blank cells at the cursor, shifting the rest of the line
    /// right off the edge.
    fn insert_chars(&mut self, n: usize) {
        let cols = self.cols();
        let (r, start) = (self.row, self.col);
        let n = n.min(cols - start);
        for c in (start..cols).rev() {
            let cell = if c >= start + n {
                self.active().cell(r, c - n)
            } else {
                Cell::default()
            };
            self.active_mut().set(r, c, cell);
        }
        self.wrap_pending = false;
    }

    /// DCH: delete `n` cells at the cursor, shifting the rest of the line left
    /// and filling the tail with blanks.
    fn delete_chars(&mut self, n: usize) {
        let cols = self.cols();
        let (r, start) = (self.row, self.col);
        let n = n.min(cols - start);
        for c in start..cols {
            let cell = if c + n < cols {
                self.active().cell(r, c + n)
            } else {
                Cell::default()
            };
            self.active_mut().set(r, c, cell);
        }
        self.wrap_pending = false;
    }

    // --- save / restore + ESC --------------------------------------------

    fn save_cursor(&mut self) {
        self.saved = Some((self.row, self.col, self.style));
    }

    fn restore_cursor(&mut self) {
        if let Some((r, c, style)) = self.saved {
            self.row = r.min(self.rows() - 1);
            self.col = c.min(self.cols() - 1);
            self.style = style;
        }
        self.wrap_pending = false;
    }

    fn esc(&mut self, final_byte: u8) {
        match final_byte {
            b'7' => self.save_cursor(),    // DECSC
            b'8' => self.restore_cursor(), // DECRC
            _ => {}                        // charset selection etc.: ignore quietly
        }
    }
}

/// Tab stops every 8 columns (VT default), with a stop at column 0.
fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c % 8 == 0).collect()
}

/// Read a param defaulting a missing/zero value to 1 (VT motion default).
fn at1(params: &[u16], i: usize) -> usize {
    match params.get(i).copied().unwrap_or(0) {
        0 => 1,
        v => v as usize,
    }
}

/// Read a param defaulting a missing value to 0 (erase/selector default).
fn at0(params: &[u16], i: usize) -> u16 {
    params.get(i).copied().unwrap_or(0)
}

/// Copy the overlapping top-left region of `src` into `dst` (for resize).
fn copy_top_left(src: &Screen, dst: &mut Screen) {
    let rows = src.rows().min(dst.rows());
    let cols = src.cols().min(dst.cols());
    for r in 0..rows {
        for c in 0..cols {
            dst.set(r, c, src.cell(r, c));
        }
    }
}
