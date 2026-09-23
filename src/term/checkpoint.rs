//! Checkpoints: a [`Term`]'s whole state as bytes, and back.
//!
//! A terminal that keeps its history on disk (foldwave) rebuilds a pane by
//! restoring the nearest checkpoint and replaying the log after it, instead
//! of replaying everything. A checkpoint therefore has to capture *all*
//! state that affects how later bytes land: both screens, the cursor, the
//! current style, the scroll region, tab stops, the saved cursor, the modes,
//! and a synchronized update in progress.
//!
//! It deliberately does not capture the tokenizer. Checkpoints are only taken
//! when the tokenizer is idle ([`Term::can_checkpoint`]): no escape sequence
//! open and no UTF-8 character half-decoded. At such a point a fresh parser is
//! indistinguishable from the running one, so there is nothing to save. On
//! agent output about 96 % of byte offsets qualify (foldwave E4), so a caller
//! simply takes the checkpoint at the next chunk boundary that does.
//!
//! Format (little-endian, versioned): magic `VTCK`, version, then the fields
//! below. Screens are run-length encoded, since most cells of most screens are
//! blank. [`Term::restore`] validates everything and never panics on bad
//! input.

use super::Term;
use ansi::{Cell, CellWidth, Color, Cursor, Parser, Screen, Style, Utf8Decoder};
use std::fmt;

const MAGIC: &[u8; 4] = b"VTCK";
const VERSION: u16 = 1;
/// Refuse to allocate grids larger than this when restoring untrusted bytes.
const MAX_CELLS: usize = 16 * 1024 * 1024;

/// Why a checkpoint could not be restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreError(&'static str);

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid terminal checkpoint: {}", self.0)
    }
}

impl std::error::Error for RestoreError {}

impl Term {
    /// True when a checkpoint can be taken now: the tokenizer is between
    /// sequences and no UTF-8 character is half-decoded.
    pub fn can_checkpoint(&self) -> bool {
        self.parser.is_ground() && !self.decoder.is_pending()
    }

    /// The emulator's whole state as bytes, or `None` if a sequence or
    /// character is in progress ([`can_checkpoint`](Term::can_checkpoint)).
    /// Deterministic: equal states give equal bytes.
    pub fn checkpoint(&self) -> Option<Vec<u8>> {
        if !self.can_checkpoint() {
            return None;
        }
        let mut w = Vec::with_capacity(4096);
        w.extend_from_slice(MAGIC);
        put_u16(&mut w, VERSION);
        let flags = (self.in_alt as u8)
            | (self.wrap_pending as u8) << 1
            | (self.autowrap as u8) << 2
            | (self.cursor_visible as u8) << 3
            | (self.in_sync as u8) << 4
            | (self.saved.is_some() as u8) << 5
            | (self.sync_frame.is_some() as u8) << 6
            | (self.newline_mode as u8) << 7;
        w.push(flags);
        put_style(&mut w, &self.style);
        put_u32(&mut w, self.scroll_top as u32);
        put_u32(&mut w, self.scroll_bottom as u32);
        if let Some((cursor, style)) = &self.saved {
            put_u32(&mut w, cursor.row as u32);
            put_u32(&mut w, cursor.col as u32);
            put_style(&mut w, style);
        }
        put_u32(&mut w, self.tabs.len() as u32);
        for chunk in self.tabs.chunks(8) {
            let mut byte = 0u8;
            for (i, &t) in chunk.iter().enumerate() {
                byte |= (t as u8) << i;
            }
            w.push(byte);
        }
        put_screen(&mut w, &self.primary);
        put_screen(&mut w, &self.alt);
        if let Some(frame) = &self.sync_frame {
            put_screen(&mut w, frame);
        }
        Some(w)
    }

    /// Rebuild an emulator from [`checkpoint`](Term::checkpoint) bytes. Feeding
    /// it the same bytes afterwards gives the same result as feeding the
    /// original. Bad input is an error, never a panic.
    pub fn restore(bytes: &[u8]) -> Result<Term, RestoreError> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != MAGIC {
            return Err(RestoreError("not a checkpoint"));
        }
        if r.u16()? != VERSION {
            return Err(RestoreError("unsupported version"));
        }
        let flags = r.u8()?;
        let bit = |n: u8| flags & (1 << n) != 0;
        let style = r.style()?;
        let scroll_top = r.u32()? as usize;
        let scroll_bottom = r.u32()? as usize;
        let saved = if bit(5) {
            let row = r.u32()? as usize;
            let col = r.u32()? as usize;
            Some((Cursor::new(row, col), r.style()?))
        } else {
            None
        };
        let ntabs = r.u32()? as usize;
        let tab_bytes = r.take(ntabs.div_ceil_compat(8))?;
        let tabs: Vec<bool> = (0..ntabs)
            .map(|i| tab_bytes[i / 8] & (1 << (i % 8)) != 0)
            .collect();
        let primary = r.screen()?;
        let alt = r.screen()?;
        let sync_frame = if bit(6) { Some(r.screen()?) } else { None };
        if r.at != bytes.len() {
            return Err(RestoreError("trailing bytes"));
        }

        let (rows, cols) = (primary.rows(), primary.cols());
        let same = |s: &Screen| s.rows() == rows && s.cols() == cols;
        if !same(&alt) || !sync_frame.as_ref().map_or(true, same) {
            return Err(RestoreError("screens disagree on size"));
        }
        if tabs.len() != cols {
            return Err(RestoreError("tab stops do not match the width"));
        }
        if scroll_top > scroll_bottom || scroll_bottom >= rows {
            return Err(RestoreError("scroll region outside the screen"));
        }
        if saved.is_some_and(|(c, _)| c.row >= rows || c.col >= cols) {
            return Err(RestoreError("saved cursor outside the screen"));
        }
        if bit(4) != sync_frame.is_some() {
            return Err(RestoreError("synchronized update without its frame"));
        }

        let mut term = Term::new(rows, cols);
        term.primary = primary;
        term.alt = alt;
        term.in_alt = bit(0);
        term.wrap_pending = bit(1);
        term.autowrap = bit(2);
        term.cursor_visible = bit(3);
        // Bit 7 was always 0 before LNM existed, so older checkpoints restore
        // with the mode off, as they were taken.
        term.newline_mode = bit(7);
        term.in_sync = bit(4);
        term.style = style;
        term.scroll_top = scroll_top;
        term.scroll_bottom = scroll_bottom;
        term.saved = saved;
        term.tabs = tabs;
        term.sync_frame = sync_frame;
        term.parser = Parser::new();
        term.decoder = Utf8Decoder::new();
        term.dirty = true;
        Ok(term)
    }
}

/// `div_ceil` for `usize`, which is newer than the MSRV.
trait DivCeil {
    fn div_ceil_compat(self, d: usize) -> usize;
}

impl DivCeil for usize {
    fn div_ceil_compat(self, d: usize) -> usize {
        (self + d - 1) / d
    }
}

fn put_u16(w: &mut Vec<u8>, v: u16) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(w: &mut Vec<u8>, v: u32) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn put_color(w: &mut Vec<u8>, c: Color) {
    match c {
        Color::Default => w.extend_from_slice(&[0, 0, 0, 0]),
        Color::Indexed(i) => w.extend_from_slice(&[1, i, 0, 0]),
        Color::Rgb(r, g, b) => w.extend_from_slice(&[2, r, g, b]),
    }
}

fn put_style(w: &mut Vec<u8>, s: &Style) {
    put_color(w, s.fg);
    put_color(w, s.bg);
    w.push(
        (s.bold as u8)
            | (s.dim as u8) << 1
            | (s.italic as u8) << 2
            | (s.underline as u8) << 3
            | (s.reverse as u8) << 4
            | (s.strike as u8) << 5,
    );
}

fn put_cell(w: &mut Vec<u8>, c: &Cell) {
    put_u32(w, c.ch as u32);
    w.push(match c.width {
        CellWidth::Continuation => 0,
        CellWidth::Single => 1,
        CellWidth::Wide => 2,
    });
    put_style(w, &c.style);
}

/// Dimensions, cursor, then runs of identical cells in row-major order.
fn put_screen(w: &mut Vec<u8>, s: &Screen) {
    put_u32(w, s.rows() as u32);
    put_u32(w, s.cols() as u32);
    let cursor = s.cursor();
    put_u32(w, cursor.row as u32);
    put_u32(w, cursor.col as u32);
    let mut run: Option<(Cell, u32)> = None;
    for row in 0..s.rows() {
        for col in 0..s.cols() {
            let cell = s.cell(row, col);
            match &mut run {
                Some((c, n)) if *c == cell => *n += 1,
                _ => {
                    if let Some((c, n)) = run.take() {
                        put_u32(w, n);
                        put_cell(w, &c);
                    }
                    run = Some((cell, 1));
                }
            }
        }
    }
    if let Some((c, n)) = run {
        put_u32(w, n);
        put_cell(w, &c);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], RestoreError> {
        let end = self.at.checked_add(n).ok_or(RestoreError("truncated"))?;
        let s = self
            .bytes
            .get(self.at..end)
            .ok_or(RestoreError("truncated"))?;
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, RestoreError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, RestoreError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, RestoreError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn color(&mut self) -> Result<Color, RestoreError> {
        let b = self.take(4)?;
        match b[0] {
            0 => Ok(Color::Default),
            1 => Ok(Color::Indexed(b[1])),
            2 => Ok(Color::Rgb(b[1], b[2], b[3])),
            _ => Err(RestoreError("unknown colour")),
        }
    }
    fn style(&mut self) -> Result<Style, RestoreError> {
        let fg = self.color()?;
        let bg = self.color()?;
        let a = self.u8()?;
        if a >> 6 != 0 {
            return Err(RestoreError("unknown style attribute"));
        }
        Ok(Style {
            fg,
            bg,
            bold: a & 1 != 0,
            dim: a & 2 != 0,
            italic: a & 4 != 0,
            underline: a & 8 != 0,
            reverse: a & 16 != 0,
            strike: a & 32 != 0,
        })
    }
    fn cell(&mut self) -> Result<Cell, RestoreError> {
        let ch = char::from_u32(self.u32()?).ok_or(RestoreError("invalid character"))?;
        let width = match self.u8()? {
            0 => CellWidth::Continuation,
            1 => CellWidth::Single,
            2 => CellWidth::Wide,
            _ => return Err(RestoreError("unknown cell width")),
        };
        Ok(Cell {
            ch,
            style: self.style()?,
            width,
        })
    }
    fn screen(&mut self) -> Result<Screen, RestoreError> {
        let rows = self.u32()? as usize;
        let cols = self.u32()? as usize;
        let total = rows
            .checked_mul(cols)
            .filter(|&n| n > 0 && n <= MAX_CELLS)
            .ok_or(RestoreError("screen size out of range"))?;
        let (crow, ccol) = (self.u32()? as usize, self.u32()? as usize);
        if crow >= rows || ccol >= cols {
            return Err(RestoreError("cursor outside the screen"));
        }
        let mut screen = Screen::new(rows, cols);
        let mut filled = 0usize;
        while filled < total {
            let n = self.u32()? as usize;
            if n == 0 || n > total - filled {
                return Err(RestoreError("bad cell run"));
            }
            let cell = self.cell()?;
            for k in filled..filled + n {
                screen.set(k / cols, k % cols, cell);
            }
            filled += n;
        }
        screen.set_cursor(Cursor::new(crow, ccol));
        Ok(screen)
    }
}
