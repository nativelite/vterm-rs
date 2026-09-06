//! vterm: a minimal terminal-emulator core, on the Rust standard library
//! plus the nativelite `ansi` crate alone. Zero third-party dependencies.
//!
//! One concern: turn a child process's VT output byte stream into a live
//! [`ansi::Screen`] you can composite and diff. [`Term::feed`] drives an
//! internal [`ansi::Parser`] and applies the resulting tokens (cursor
//! motion, erase, scroll region, SGR style, autowrap, alt-screen) to a cell
//! grid. [`Term::screen`] hands back the active buffer with its cursor set,
//! ready for `Screen::diff` compositing.
//!
//! ```
//! let mut t = vterm::Term::new(24, 80);
//! t.feed(b"\x1b[2J\x1b[3;5HX");   // clear, cursor to row 3 col 5, print 'X'
//! let s = t.screen();
//! assert_eq!(s.cell(2, 4).ch, 'X');
//! assert_eq!(s.cursor, (2, 5)); // cursor advanced past the 'X'
//! ```
//!
//! This is a *correct common core*, not a pixel-perfect xterm. Wide/CJK
//! characters, sixel/images, mouse reporting, and exotic private modes are
//! deliberately out of scope (see the README's fidelity boundaries); the host
//! app's answer for those is raw passthrough (amux's "zoom").
//!
//! There is no I/O here: no PTY, no raw mode. `vterm` turns bytes into a
//! screen; feeding it child output and painting its screen are the caller's
//! job (see `pty` and `amux`).

mod term;

pub use term::Term;
