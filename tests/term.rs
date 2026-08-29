//! Integration tests for `vterm`.
//!
//! Spec-anchored: each vector is a hand-authored `(input_bytes, expected)`
//! pair traced against VT100/ECMA-48 behavior. We assert cell contents, cursor
//! position, style-on-write, scroll confinement, alt-screen swap, and
//! chunk-split invariance (feeding the same bytes split at every boundary must
//! yield an identical final screen). Zero dev-dependencies — the built-in
//! `#[test]` harness only.

use vterm::Term;

/// Collect a row's characters into a `String` for readable assertions.
fn row_text(t: &Term, row: usize) -> String {
    let s = t.screen();
    (0..s.cols()).map(|c| s.cell(row, c).ch).collect()
}

/// Build a `Term`, feed the bytes in one shot.
fn run(rows: usize, cols: usize, bytes: &[u8]) -> Term {
    let mut t = Term::new(rows, cols);
    t.feed(bytes);
    t
}

// --- basic printing + cursor ------------------------------------------------

#[test]
fn prints_text_and_advances_cursor() {
    let t = run(4, 10, b"hi");
    assert_eq!(t.screen().cell(0, 0).ch, 'h');
    assert_eq!(t.screen().cell(0, 1).ch, 'i');
    assert_eq!(t.screen().cursor, (0, 2));
}

#[test]
fn cup_and_erase_then_print() {
    // The spec's anchor vector.
    let t = run(24, 80, b"\x1b[2J\x1b[3;5HX");
    assert_eq!(t.screen().cell(2, 4).ch, 'X');
    assert_eq!(t.screen().cursor, (2, 5));
}

#[test]
fn hvp_f_is_same_as_cup() {
    let t = run(10, 10, b"\x1b[2;3fZ");
    assert_eq!(t.screen().cell(1, 2).ch, 'Z');
    assert_eq!(t.screen().cursor, (1, 3));
}

#[test]
fn cup_defaults_and_clamps() {
    // Bare CUP homes the cursor; out-of-range clamps to the last cell.
    assert_eq!(run(5, 5, b"\x1b[H").screen().cursor, (0, 0));
    assert_eq!(run(5, 5, b"\x1b[99;99H").screen().cursor, (4, 4));
}

// --- CR / LF / BS / TAB motion ---------------------------------------------

#[test]
fn carriage_return_to_col0() {
    let t = run(4, 10, b"abc\rX");
    assert_eq!(t.screen().cell(0, 0).ch, 'X');
    assert_eq!(t.screen().cell(0, 1).ch, 'b');
    assert_eq!(t.screen().cursor, (0, 1));
}

#[test]
fn line_feed_moves_down_same_column() {
    let t = run(4, 10, b"ab\nc");
    assert_eq!(t.screen().cell(0, 0).ch, 'a');
    assert_eq!(t.screen().cell(1, 2).ch, 'c');
    assert_eq!(t.screen().cursor, (1, 3));
}

#[test]
fn vt_and_ff_behave_like_lf() {
    assert_eq!(run(4, 5, b"a\x0bb").screen().cursor, (1, 2));
    assert_eq!(run(4, 5, b"a\x0cb").screen().cursor, (1, 2));
}

#[test]
fn backspace_moves_left_and_clamps() {
    let t = run(4, 10, b"abc\x08\x08");
    assert_eq!(t.screen().cursor, (0, 1));
    assert_eq!(run(4, 10, b"\x08").screen().cursor, (0, 0)); // clamps at 0
}

#[test]
fn tab_advances_to_next_stop() {
    let t = run(4, 40, b"\tX");
    assert_eq!(t.screen().cell(0, 8).ch, 'X');
    assert_eq!(t.screen().cursor, (0, 9));
    // A tab from mid-cell jumps to the next multiple of 8.
    let t2 = run(4, 40, b"abc\tY");
    assert_eq!(t2.screen().cell(0, 8).ch, 'Y');
}

// --- cursor motion CSI ------------------------------------------------------

#[test]
fn cursor_up_down_left_right() {
    // Home, down 2, right 3, up 1, left 1 → (1, 2).
    let t = run(10, 10, b"\x1b[H\x1b[2B\x1b[3C\x1b[A\x1b[D");
    assert_eq!(t.screen().cursor, (1, 2));
}

#[test]
fn cnl_and_cpl_go_to_col0() {
    let t = run(10, 10, b"\x1b[5;5H\x1b[2EX");
    assert_eq!(t.screen().cursor, (6, 1)); // down 2 lines, col 0, printed X
    assert_eq!(t.screen().cell(6, 0).ch, 'X');
    let t2 = run(10, 10, b"\x1b[5;5H\x1b[2F");
    assert_eq!(t2.screen().cursor, (2, 0));
}

#[test]
fn cha_and_vpa_absolute() {
    let t = run(10, 10, b"\x1b[5;5H\x1b[3G"); // CHA col 3
    assert_eq!(t.screen().cursor, (4, 2));
    let t2 = run(10, 10, b"\x1b[5;5H\x1b[2d"); // VPA row 2
    assert_eq!(t2.screen().cursor, (1, 4));
}

// --- autowrap ---------------------------------------------------------------

#[test]
fn autowrap_at_right_edge() {
    // 3 cols: "abc" fills the row, "d" wraps to the next line col 0.
    let t = run(4, 3, b"abcd");
    assert_eq!(row_text(&t, 0), "abc");
    assert_eq!(t.screen().cell(1, 0).ch, 'd');
    assert_eq!(t.screen().cursor, (1, 1));
}

#[test]
fn pending_wrap_is_deferred() {
    // After "abc" the cursor is latched at col 2 (pending), not yet wrapped.
    let t = run(4, 3, b"abc");
    assert_eq!(t.screen().cursor, (0, 2));
    assert_eq!(row_text(&t, 0), "abc");
}

#[test]
fn autowrap_off_overwrites_last_column() {
    // DECAWM off (?7l): writes past the edge overwrite the last cell.
    let t = run(4, 3, b"\x1b[?7labcd");
    assert_eq!(row_text(&t, 0), "abd");
    assert_eq!(t.screen().cursor, (0, 2));
}

// --- scroll on LF at bottom margin -----------------------------------------

#[test]
fn scroll_when_lf_at_bottom() {
    // 2 rows: fill row0="A", row1="B", then LF scrolls: row0="B", row1 blank.
    // Use CR+LF so each line starts at col 0.
    let t = run(2, 3, b"A\r\nB\r\n");
    assert_eq!(t.screen().cell(0, 0).ch, 'B');
    assert_eq!(t.screen().cell(1, 0).ch, ' ');
    assert_eq!(t.screen().cursor, (1, 0));
}

#[test]
fn scroll_loses_top_line() {
    let t = run(3, 3, b"1\r\n2\r\n3\r\n4");
    assert_eq!(t.screen().cell(0, 0).ch, '2');
    assert_eq!(t.screen().cell(1, 0).ch, '3');
    assert_eq!(t.screen().cell(2, 0).ch, '4');
}

// --- ED / EL ----------------------------------------------------------------

#[test]
fn erase_display_variants() {
    // ED 0: cursor to end. Fill 2x3 with X, home+down, ED0 clears from (1,0).
    let t = run(2, 3, b"XXX\rXXX"); // note: no newline scroll; simple fill
    let mut t = t;
    t.feed(b"\x1b[1;2H\x1b[0J"); // cursor (0,1), erase to end
    assert_eq!(t.screen().cell(0, 0).ch, 'X');
    assert_eq!(t.screen().cell(0, 1).ch, ' ');
    assert_eq!(t.screen().cell(0, 2).ch, ' ');

    // ED 1: start to cursor.
    let mut t = run(1, 5, b"ABCDE");
    t.feed(b"\x1b[1;3H\x1b[1J"); // cursor (0,2)
    assert_eq!(row_text(&t, 0), "   DE");

    // ED 2: whole screen.
    let mut t = run(2, 3, b"abcdef");
    t.feed(b"\x1b[2J");
    assert_eq!(row_text(&t, 0), "   ");
    assert_eq!(row_text(&t, 1), "   ");
}

#[test]
fn erase_line_variants() {
    let mut t = run(1, 5, b"ABCDE");
    t.feed(b"\x1b[1;3H\x1b[0K"); // erase cursor to end from col 2
    assert_eq!(row_text(&t, 0), "AB   ");

    let mut t = run(1, 5, b"ABCDE");
    t.feed(b"\x1b[1;3H\x1b[1K"); // erase start to cursor (inclusive)
    assert_eq!(row_text(&t, 0), "   DE");

    let mut t = run(1, 5, b"ABCDE");
    t.feed(b"\x1b[1;3H\x1b[2K"); // erase whole line
    assert_eq!(row_text(&t, 0), "     ");
}

// --- SGR --------------------------------------------------------------------

#[test]
fn sgr_sets_style_on_written_cells() {
    let t = run(2, 10, b"\x1b[1;31mR\x1b[0mN");
    let r = t.screen().cell(0, 0);
    assert!(r.style.bold);
    assert_eq!(r.style.fg, ansi::Color::Indexed(1));
    let n = t.screen().cell(0, 1);
    assert!(!n.style.bold);
    assert_eq!(n.style.fg, ansi::Color::Default);
}

#[test]
fn bare_sgr_m_resets() {
    let t = run(2, 10, b"\x1b[1m\x1b[mX"); // empty params ⇒ reset
    assert!(!t.screen().cell(0, 0).style.bold);
}

// --- DECSTBM scroll region --------------------------------------------------

#[test]
fn scroll_region_confines_scrolling() {
    // Region rows 1..2 (1-based 2;3). LFs scroll only within it; row0 fixed.
    let mut t = run(4, 3, b"top");
    t.feed(b"\x1b[2;3r"); // region rows 2..3, cursor homes to (0,0)
                          // Move into the region bottom and force scrolls (CR+LF for col 0).
    t.feed(b"\x1b[3;1HA\r\nB\r\nC");
    assert_eq!(row_text(&t, 0), "top"); // untouched, above region
                                        // Region scrolled: last two written lines survive at rows 1 and 2.
    assert_eq!(t.screen().cell(1, 0).ch, 'B');
    assert_eq!(t.screen().cell(2, 0).ch, 'C');
}

#[test]
fn scroll_region_resets_cursor_to_origin() {
    let mut t = run(6, 6, b"\x1b[3;4H");
    t.feed(b"\x1b[2;5r");
    assert_eq!(t.screen().cursor, (0, 0));
}

// --- IL / DL ----------------------------------------------------------------

#[test]
fn insert_lines_shifts_down() {
    // rows: 0=A 1=B 2=C. IL 1 at row 1 → 0=A 1=blank 2=B (C falls off).
    let mut t = run(3, 3, b"A\r\nB\r\nC");
    t.feed(b"\x1b[2;1H\x1b[L");
    assert_eq!(t.screen().cell(0, 0).ch, 'A');
    assert_eq!(t.screen().cell(1, 0).ch, ' ');
    assert_eq!(t.screen().cell(2, 0).ch, 'B');
}

#[test]
fn delete_lines_shifts_up() {
    // rows: 0=A 1=B 2=C. DL 1 at row 1 → 0=A 1=C 2=blank.
    let mut t = run(3, 3, b"A\r\nB\r\nC");
    t.feed(b"\x1b[2;1H\x1b[M");
    assert_eq!(t.screen().cell(0, 0).ch, 'A');
    assert_eq!(t.screen().cell(1, 0).ch, 'C');
    assert_eq!(t.screen().cell(2, 0).ch, ' ');
}

// --- ICH / DCH --------------------------------------------------------------

#[test]
fn insert_chars_shifts_right() {
    let mut t = run(1, 5, b"ABCDE");
    t.feed(b"\x1b[1;2H\x1b[2@"); // insert 2 blanks at col 1
    assert_eq!(row_text(&t, 0), "A  BC");
}

#[test]
fn delete_chars_shifts_left() {
    let mut t = run(1, 5, b"ABCDE");
    t.feed(b"\x1b[1;2H\x1b[2P"); // delete 2 chars at col 1
    assert_eq!(row_text(&t, 0), "ADE  ");
}

// --- save / restore cursor --------------------------------------------------

#[test]
fn csi_save_restore_cursor() {
    let t = run(10, 10, b"\x1b[3;4H\x1b[s\x1b[8;8H\x1b[u");
    assert_eq!(t.screen().cursor, (2, 3));
}

#[test]
fn esc_decsc_decrc() {
    let t = run(10, 10, b"\x1b[3;4H\x1b7\x1b[8;8H\x1b8");
    assert_eq!(t.screen().cursor, (2, 3));
}

#[test]
fn restore_also_restores_style() {
    let t = run(2, 10, b"\x1b[1m\x1b7\x1b[0m\x1b8X");
    assert!(t.screen().cell(0, 0).style.bold); // style restored to bold
}

// --- alt screen -------------------------------------------------------------

#[test]
fn alt_screen_hides_and_restores_primary() {
    let mut t = run(4, 5, b"PRIME");
    assert_eq!(t.screen().cell(0, 0).ch, 'P');
    t.feed(b"\x1b[?1049h"); // enter alt: cleared buffer
    assert_eq!(t.screen().cell(0, 0).ch, ' ');
    t.feed(b"\x1b[HALT");
    assert_eq!(t.screen().cell(0, 0).ch, 'A');
    t.feed(b"\x1b[?1049l"); // leave alt: primary restored
    assert_eq!(t.screen().cell(0, 0).ch, 'P');
}

#[test]
fn alt_screen_47_and_1047_also_work() {
    let mut t = run(4, 5, b"X");
    t.feed(b"\x1b[?47h");
    assert_eq!(t.screen().cell(0, 0).ch, ' ');
    t.feed(b"\x1b[?47l");
    assert_eq!(t.screen().cell(0, 0).ch, 'X');
}

// --- cursor visibility (tracked, non-fatal) --------------------------------

#[test]
fn cursor_hide_show_is_accepted() {
    // Just assert these don't corrupt the screen; visibility is host-side.
    let t = run(4, 5, b"\x1b[?25lAB\x1b[?25h");
    assert_eq!(t.screen().cell(0, 0).ch, 'A');
    assert_eq!(t.screen().cursor, (0, 2));
}

// --- resize -----------------------------------------------------------------

#[test]
fn resize_preserves_top_left_and_clamps_cursor() {
    let mut t = run(4, 4, b"\x1b[4;4Hxy"); // content near old bottom-right
    t.feed(b"\x1b[1;1HAB"); // top-left content
    t.resize(2, 2);
    assert_eq!(t.screen().rows(), 2);
    assert_eq!(t.screen().cols(), 2);
    assert_eq!(t.screen().cell(0, 0).ch, 'A');
    assert_eq!(t.screen().cell(0, 1).ch, 'B');
    // Cursor was at (0,2) after "AB"; clamps into 2x2.
    let (r, c) = t.screen().cursor;
    assert!(r < 2 && c < 2);
}

// --- chunk-split invariance -------------------------------------------------

#[test]
fn chunk_split_yields_identical_screen() {
    let script = b"\x1b[2J\x1b[3;5m\x1b[1;1mHello\r\n\x1b[1;31mWorld\x1b[0m\x1b[2;3r\nmore\ttext\x1b[?1049h\x1b[HALT\x1b[?1049l";
    let oneshot = run(8, 20, script);

    // Feed the same bytes one byte at a time.
    let mut split = Term::new(8, 20);
    for &b in script.iter() {
        split.feed(&[b]);
    }
    // Compare full grids and cursor.
    let (a, b) = (oneshot.screen(), split.screen());
    assert_eq!(a.rows(), b.rows());
    assert_eq!(a.cols(), b.cols());
    assert_eq!(a.cursor, b.cursor);
    for r in 0..a.rows() {
        for c in 0..a.cols() {
            assert_eq!(a.cell(r, c), b.cell(r, c), "mismatch at ({r},{c})");
        }
    }

    // And split at a different, arbitrary boundary (chunks of 3).
    let mut split3 = Term::new(8, 20);
    for chunk in script.chunks(3) {
        split3.feed(chunk);
    }
    let c = split3.screen();
    assert_eq!(a.cursor, c.cursor);
    for r in 0..a.rows() {
        for col in 0..a.cols() {
            assert_eq!(a.cell(r, col), c.cell(r, col));
        }
    }
}
