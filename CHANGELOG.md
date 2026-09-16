# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.1] - 2026-09-15

Feeding output is about 6x faster.

### Changed
- **Scrolling goes through `ansi::Screen::scroll_rows_up`/`scroll_rows_down`**
  instead of moving cells one at a time. Scrolling is what a terminal does on
  nearly every line of output, and it was the emulator's dominant cost.
  Feeding 16 MB of coloured, agent-style log output into one `Term`:

  | grid | 0.5.0 | 0.5.1 |
  | --- | --- | --- |
  | 24x80 | 16.1 | 70.2 MB/s |
  | 40x160 | 11.0 | 66.0 MB/s |
  | 60x240 | 5.3 | 63.8 MB/s |

  The cost no longer grows with the grid. Measured on Linux with atrium's
  `cargo bench --bench terminal` (`ATRIUM_BENCH_ONLY=feed`).
- **Requires `nativelite-ansi` 0.3.1**, which is where those two methods
  arrived. No API change here.

## [0.5.0] - 2026-09-14

Breaking: built on `ansi` 0.3, whose `Screen` cursor and cell width
are typed. `Term::screen()` returns that `Screen`, so the change reaches
callers.

### Changed
- **Requires `nativelite-ansi` 0.3.** Read the cursor as
  `term.screen().cursor()` (an `ansi::Cursor { row, col }`) instead of the
  `.cursor` tuple field; compare widths with `ansi::CellWidth` instead of
  `0`/`1`/`2`.
- **One cursor.** `Term` no longer keeps its own row/column mirrored into the
  active screen after every token: the active buffer's cursor *is* the
  emulator cursor, and it moves with the buffer on an alt-screen switch or a
  resize.

### Fixed
- **A synchronized-update snapshot taken in the same sequence as an alt-screen
  switch now carries the live cursor.** `[?1049;2026h` switched buffers and
  snapshotted mid-token, before the end-of-token mirror ran, so `screen()`
  reported the fresh buffer's `(0,0)` for that frame while the emulator was
  elsewhere. It is the one divergence a review of the change found; final
  screens are checksum-identical to 0.4 on mixed benchmark streams, and a
  cursor test suite for alt switches, save/restore across buffers, resize in
  alt and plain sync updates passes unchanged against both versions.
- The crate docs no longer list wide/CJK glyphs as out of scope (they are laid
  out as two-column cells since 0.4.0).

## [0.3.0] - 2026-08-31

### Added
- **ECH (`CSI n X`): erase character.** Blanks `n` cells from the cursor
  rightward without moving the cursor or shifting the rest of the line (unlike
  DCH). This is a common way to clear a run of cells (status lines, trailing
  content); dropping it left **stale text behind**, a "leftover artifact" when
  compositing. Now handled.
- **SU (`CSI n S`) / SD (`CSI n T`): scroll up / down.** Wire the existing
  scroll-region up/down to their CSI finals, so an app that scrolls via SU/SD
  (rather than newline/reverse-index) renders correctly instead of leaving stale
  rows.

These close standard VT100/ECMA-48 gaps in the "correct common core": the class
of missing sequences that leaves uncleared cells on screen.

## [0.2.0] - 2026-08-30

### Added
- Synchronized output (DEC private mode 2026, `?2026h`/`?2026l`). Apps such as
  Claude Code wrap each screen update in a synchronized block so a terminal
  never shows a half-drawn frame. `Term` now double-buffers across a
  synchronized update: on `?2026h` it snapshots the last complete frame and
  `screen()` serves that snapshot until `?2026l`, then reveals the completed
  live buffer atomically. Writes continue to land on the live buffer throughout.
- `Term::in_sync()`: true while a synchronized update is open, so a host can
  also gate its own compositing on it.

### Fixed
- Tiled compositors (e.g. atrium) that sample `screen()` on a timer no longer
  composite a pane mid-redraw, which produced stray leftover / overlapping text
  when hosting apps that use mode 2026. Nested opens keep the first snapshot; a
  resize mid-sync drops the stale snapshot and reveals the live buffer.

## [0.1.0] - 2026-08-28

### Added
- `Term`: a minimal VT100/ECMA-48 terminal-emulator core: `new`, `feed`
  (drive with child output through an internal `ansi::Parser`), `screen`
  (the active buffer with its cursor synced for compositing/diff), and
  `resize`.
- Control set: cursor motion (`CUP`/`HVP`, `CUU`/`CUD`/`CUF`/`CUB`,
  `CNL`/`CPL`, `CHA`, `VPA`; `CR`, `LF`/`VT`/`FF` with scroll, `BS`, `HT`);
  erase (`ED`, `EL`); scroll region (`DECSTBM`) with `IL`/`DL` and `ICH`/`DCH`;
  `SGR` style via `ansi::Style::apply_sgr`; autowrap (`DECAWM`) with a deferred
  pending-wrap latch; cursor save/restore (`DECSC`/`DECRC`, CSI `s`/`u`);
  cursor show/hide (`?25h/l`); alt-screen (`?1049`/`?47`/`?1047`). Titles,
  charset selection, and other private modes are accepted and ignored quietly.
- Test suite: hand-authored VT100/ECMA-48 vectors for every control: cursor
  motion, CR/LF/BS/TAB, autowrap and the pending-wrap edge case, scroll on LF
  at the bottom margin, `ED`/`EL` ranges, SGR-on-write, `DECSTBM` confinement,
  `IL`/`DL`, `ICH`/`DCH`, save/restore, alt-screen swap/restore, resize, and a
  chunk-split-invariance test proving the final screen is independent of how
  the byte stream is chunked.
- Stdlib-only `dev.py` runner (`check`, `test`, `fmt`, `guard`) and a Cargo.toml
  dependency guard (nativelite org crates only; the sole allowed dependency is
  `ansi`, with zero third-party/crates.io deps).

Depends only on the org crate `ansi` (the app-variant rule); third-party
dependencies remain forbidden. Third crate in the nativelite **agent terminal**
suite (see `roadmap/atrium-0.2-tiling.md` in `nativelite/ops`); the emulator core
that atrium 0.2 tiled panes are built on.

[Unreleased]: https://github.com/nativelite/vterm-rs/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/nativelite/vterm-rs/compare/v0.3.0...v0.5.0
[0.1.0]: https://github.com/nativelite/vterm-rs/releases/tag/v0.1.0
