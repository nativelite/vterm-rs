# vterm

A minimal **terminal-emulator core**: child VT output bytes in, a live
`ansi::Screen` out. Feed it a shell or TUI's byte stream; read back an
in-memory grid of styled cells you can composite into a sub-rectangle and diff
onto a real terminal.

Part of the nativelite **agent terminal** suite. Built for
[atrium](https://github.com/nativelite/atrium)'s tiled panes, where a pane no
longer owns the whole screen, so its whole-screen escape sequences (cursor
moves, clears, scroll regions, wrapping) must be *interpreted* into a grid and
painted at an offset. Useful anywhere child output is rendered into a
region: log viewers, test-runners, dashboards.

- **Zero third-party dependencies.** Standard library plus the org crates
  [`ansi`](https://github.com/nativelite/ansi-rs) (screen model) and
  [`uwidth`](https://github.com/nativelite/uwidth-rs) (character display
  width) only. Nothing third-party from crates.io.
- **One concern.** `ansi` parses and diffs; `vterm` is the state machine that
  applies parsed tokens to a `Screen`. No I/O: no PTY, no raw mode.

## Use

```rust
let mut t = vterm::Term::new(24, 80);
t.feed(b"\x1b[2J\x1b[3;5HX");   // clear, cursor to row 3 col 5, print 'X'

let s = t.screen();
assert_eq!(s.cell(2, 4).ch, 'X');
assert_eq!(s.cursor(), ansi::Cursor::new(2, 5));   // advanced past the 'X'
```

The whole API is `Term` and its error type `RestoreError`:

```rust
pub struct Term { /* private */ }

impl Term {
    pub fn new(rows: usize, cols: usize) -> Self;
    pub fn with_history(rows: usize, cols: usize, lines: usize) -> Self; // keep scrollback
    pub fn feed(&mut self, bytes: &[u8]);   // drive with child output
    pub fn take_dirty(&mut self) -> bool;   // changed since the last call?
    pub fn screen(&self) -> &ansi::Screen;  // active buffer, cursor included
    pub fn scrollback(&self) -> &ansi::Screen; // primary buffer + its history
    pub fn in_alternate_screen(&self) -> bool;
    pub fn in_sync(&self) -> bool;          // inside a ?2026 synchronized update
    pub fn cursor_visible(&self) -> bool;   // ?25h (default) / ?25l
    pub fn bracketed_paste(&self) -> bool;  // ?2004h: wrap pastes in ESC[200~ ESC[201~
    pub fn resize(&mut self, rows: usize, cols: usize);

    // Save and rebuild the whole emulator state.
    pub fn can_checkpoint(&self) -> bool;   // false mid-sequence / mid-UTF-8
    pub fn checkpoint(&self) -> Option<Vec<u8>>;
    pub fn restore(bytes: &[u8]) -> Result<Term, RestoreError>;
}
```

`feed` chunks may split at **any** byte boundary: mid-escape, mid-UTF-8. The
internal `ansi::Parser` holds partial state across calls, so the final screen
is independent of how the bytes were chunked. `screen()` returns the active
buffer (the alternate buffer while in alt-screen, else primary) with its
`cursor` field set to the emulator's position, ready for `Screen::diff`. During
a synchronized update it returns the last complete frame instead.

## What it interprets

The honest MVP, enough to host a shell and the common agent TUIs correctly:

- **Cursor motion:** `CUP`/`HVP` (`H`, `f`), `CUU`/`CUD`/`CUF`/`CUB` (`A`–`D`),
  `CNL`/`CPL` (`E`, `F`), `CHA` (`G`), `VPA` (`d`); `CR`, `LF`/`VT`/`FF` (with
  scroll at the bottom margin), `BS`, `HT` (tab stops every 8 columns).
- **Erase:** `ED` (`0J`/`1J`/`2J`, and `3J` to clear the scrollback), `EL`
  (`0K`/`1K`/`2K`).
- **Scroll region:** `DECSTBM` (`t;b r`) confining scroll-on-LF; `IL`/`DL`
  (insert/delete line), `ICH`/`DCH` (insert/delete char).
- **Style:** `SGR` (`m`) via `ansi::Style::apply_sgr`: full color and attributes.
- **Autowrap** (`DECAWM`, default on) with the deferred pending-wrap latch at
  the right edge.
- **Cursor save/restore:** `DECSC`/`DECRC` (`ESC 7`/`ESC 8`) and CSI `s`/`u`.
- **Cursor show/hide:** `?25h`/`?25l`, read with `cursor_visible()`
  (checkpointed).
- **Bracketed paste:** `?2004h`/`?2004l`, read with `bracketed_paste()`
  (checkpointed); the host wraps what it pastes.
- **Alt-screen:** `?1049h/l`, `?47h/l`, `?1047h/l`: swap to a cleared second
  buffer so a full-screen app does not scribble the primary one.
- **Synchronized output:** `?2026h`/`?2026l`; see `in_sync` above.
- **New-line mode:** `LNM` (`CSI 20 h`/`l`): LF, VT and FF also return to
  column 0.
- **Wrapped lines:** a row that autowraps is marked on the screen
  (`Screen::row_wrapped`, and `history_wrapped` once it scrolls off), so a
  host can copy a wrapped line as one; erasing to the end of the row
  clears the mark. Checkpointed.
- **Wide characters:** a double-width glyph (CJK, emoji; width from `uwidth`)
  takes two cells and advances the cursor by two. At the right edge it wraps
  whole, or is dropped when autowrap is off; it is never split.

Titles (`OSC`), charset selection (`ESC ( X`), and unrecognized private modes
are accepted and ignored quietly.

## Fidelity boundaries (honest scope)

`vterm` is a *correct common core*, **not** a pixel-perfect xterm. Deliberately
out of scope; the host app's answer for these is raw passthrough (in atrium,
"zoom" the pane to full-screen and the bytes go straight to your real terminal):

- **Combining marks and other zero-width characters.** A cell holds one
  `char`, so these are dropped rather than composed onto the previous glyph.
- **Sixel / images / mouse reporting / bracketed-paste** semantics inside a tile.
- **Exotic private modes** beyond the set listed above.

This boundary is the point: a correct common core plus an honest escape hatch,
not a leaky imitation of xterm.

## Develop

```
python dev.py check    # dependency guard + cargo fmt --check + cargo test (the pre-push gate)
python dev.py test     # cargo test
python dev.py fmt      # cargo fmt --check
python dev.py guard    # dependency guard (org crates only, no third-party)
```

## License

MIT: see [LICENSE](LICENSE). nativelite ships everything permissively.
