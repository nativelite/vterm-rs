# vterm

A minimal **terminal-emulator core**: child VT output bytes in, a live
`ansi::Screen` out. Feed it a shell or TUI's byte stream; read back an
in-memory grid of styled cells you can composite into a sub-rectangle and diff
onto a real terminal.

Part of the nativelite **agent terminal** suite. Built for
[amux](https://github.com/nativelite/amux)'s tiled panes, where a pane no
longer owns the whole screen, so its whole-screen escape sequences (cursor
moves, clears, scroll regions, wrapping) must be *interpreted* into a grid and
painted at an offset. Useful anywhere child output is rendered into a
region: log viewers, test-runners, dashboards.

- **Zero third-party dependencies.** Standard library plus the org crate
  [`ansi`](https://github.com/nativelite/ansi-rs) only. Nothing from crates.io.
- **One concern.** `ansi` parses and diffs; `vterm` is the state machine that
  applies parsed tokens to a `Screen`. No I/O: no PTY, no raw mode.

## Use

```rust
let mut t = vterm::Term::new(24, 80);
t.feed(b"\x1b[2J\x1b[3;5HX");   // clear, cursor to row 3 col 5, print 'X'

let s = t.screen();
assert_eq!(s.cell(2, 4).ch, 'X');
assert_eq!(s.cursor, (2, 5));   // cursor advanced past the 'X'
```

The whole API is four methods:

```rust
pub struct Term { /* private */ }

impl Term {
    pub fn new(rows: usize, cols: usize) -> Self;
    pub fn feed(&mut self, bytes: &[u8]);   // drive with child output
    pub fn screen(&self) -> &ansi::Screen;  // active buffer, cursor synced
    pub fn resize(&mut self, rows: usize, cols: usize);
}
```

`feed` chunks may split at **any** byte boundary: mid-escape, mid-UTF-8. The
internal `ansi::Parser` holds partial state across calls, so the final screen
is independent of how the bytes were chunked. `screen()` returns the active
buffer (the alternate buffer while in alt-screen, else primary) with its
`cursor` field set to the emulator's position, ready for `Screen::diff`.

## What it interprets

The honest MVP, enough to host a shell and the common agent TUIs correctly:

- **Cursor motion:** `CUP`/`HVP` (`H`, `f`), `CUU`/`CUD`/`CUF`/`CUB` (`A`–`D`),
  `CNL`/`CPL` (`E`, `F`), `CHA` (`G`), `VPA` (`d`); `CR`, `LF`/`VT`/`FF` (with
  scroll at the bottom margin), `BS`, `HT` (tab stops every 8 columns).
- **Erase:** `ED` (`0J`/`1J`/`2J`), `EL` (`0K`/`1K`/`2K`).
- **Scroll region:** `DECSTBM` (`t;b r`) confining scroll-on-LF; `IL`/`DL`
  (insert/delete line), `ICH`/`DCH` (insert/delete char).
- **Style:** `SGR` (`m`) via `ansi::Style::apply_sgr`: full color and attributes.
- **Autowrap** (`DECAWM`, default on) with the deferred pending-wrap latch at
  the right edge.
- **Cursor save/restore:** `DECSC`/`DECRC` (`ESC 7`/`ESC 8`) and CSI `s`/`u`.
- **Cursor show/hide:** `?25h`/`?25l` (tracked for the host).
- **Alt-screen:** `?1049h/l`, `?47h/l`, `?1047h/l`: swap to a cleared second
  buffer so a full-screen app does not scribble the primary one.

Titles (`OSC`), charset selection (`ESC ( X`), and unrecognized private modes
are accepted and ignored quietly.

## Fidelity boundaries (honest scope)

`vterm` is a *correct common core*, **not** a pixel-perfect xterm. Deliberately
out of scope; the host app's answer for these is raw passthrough (in amux,
"zoom" the pane to full-screen and the bytes go straight to your real terminal):

- **Wide / CJK characters.** `ansi::Cell` is one column per `char` by design, so
  double-width glyphs mis-column in a tile.
- **Sixel / images / mouse reporting / bracketed-paste** semantics inside a tile.
- **Exotic private modes** beyond the set listed above.

This boundary is the point: a correct common core plus an honest escape hatch,
not a leaky imitation of xterm.

## Develop

```
python dev.py check    # zero-dependency guard + cargo test (the pre-push gate)
python dev.py test     # cargo test
python dev.py fmt      # cargo fmt --check
python dev.py guard    # dependency guard (org crates only, no third-party)
```

## License

MIT: see [LICENSE](LICENSE). nativelite ships everything permissively.
