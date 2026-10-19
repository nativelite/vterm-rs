# nativelite-vterm benchmarks

```sh
cargo bench --bench feed_throughput
```

A release build, about 4 s. Standard library only: a `main()` around
`std::time::Instant`, no benchmark framework (`harness = false`). Under
`cargo test --all-targets` the target only builds and returns, so the local gate
stays fast. Knobs (environment):

- `VTERM_BENCH_MIB` (per variant, default 16)
- `VTERM_BENCH_RUNS` (default 5; the median is reported)
- `VTERM_BENCH_ROWS` / `VTERM_BENCH_COLS` (default 47 x 177)
- `VTERM_BENCH_CHUNK` (bytes per `Term::feed`, default 8192)
- `VTERM_BENCH_ONLY` (one variant, e.g. `agent`)

## What `feed_throughput` measures

The cost of `Term::feed` alone: bytes in, screen state out. No pty, no
rendering, no I/O. Each variant is one deterministic stream (a fixed-seed
generator, so every machine feeds the same bytes), generated once, untimed.
A run is a fresh `Term` fed the whole stream in pane-reader-sized chunks; the
timer surrounds the feeding only. Every variant runs `VTERM_BENCH_RUNS` times
and the table shows the **median**, with the slowest and fastest run beside it
so the spread is visible. MiB/s is 2^20 bytes per second of feeding.

Every line ends `\r\n` on the bottom row, so the screen scrolls on every line;
scrolling is in every number.

| variant | the stream |
| --- | --- |
| `ascii` | printable ASCII words and numbers, no escape sequences: the ASCII fast path and scrolling, nothing else. The upper bound. |
| `agent` | what an agent CLI prints: cargo, test, log and diff lines, SGR colour on some of them. The headline figure. |
| `sgr` | every word in its own colour with a reset after it: about half the bytes are SGR sequences. |
| `cursor` | a status area redrawn in place: `\r` progress bars with erase-line, cursor up and erase to repaint rows above, an occasional home plus erase-to-end-of-screen. |
| `utf8` | box drawing, accented Latin, CJK and emoji among the ASCII: the UTF-8 decoder and the wide-glyph path. |
| `mixed` | the five above, interleaved line by line. |
| `agent, history` | the `agent` stream into `Term::with_history(rows, cols, 10_000)`, so every scrolled-off line is also kept. |

## Results: nativelite-vterm 0.5.4 (2026-10-18)

Machine: the author's desktop, 12th Gen Intel i5-12600K (16 logical cores),
64 GB RAM, Windows 11 Pro, release build, machine otherwise idle. Defaults:
47 x 177, 16 MiB per variant, 8 KiB chunks, median of 5 runs. Spread within a
variant was 1-6%; treat single-digit percentage differences as noise until
repeated.

| variant | median MiB/s | min | max |
| --- | --- | --- | --- |
| ascii | 804.5 | 772.9 | 817.6 |
| agent | 544.6 | 542.4 | 559.6 |
| sgr | 276.0 | 259.4 | 285.9 |
| cursor | 39.0 | 38.7 | 39.3 |
| utf8 | 262.7 | 260.3 | 266.8 |
| mixed | 117.4 | 117.1 | 119.3 |
| agent, history | 244.9 | 237.3 | 251.8 |

A 32 MiB run of the same variants gave the same picture (ascii 828.8, agent
549.4, sgr 279.2, cursor 40.0, utf8 269.7, mixed 119.5, history 251.5), so
the rate does not depend on the stream length at these sizes.

What the numbers say:
- Plain text is cheap: the `ascii` and `agent` variants are the ASCII fast
  path (`Term::print_run` into `ansi::Screen::write_ascii`) plus a scroll per
  line.
- Escape sequences cost more than text: `sgr` and `utf8` run at about half
  the `agent` rate; both leave the fast path for every word.
- `cursor` is an order of magnitude slower than everything else. Its stream
  is mostly erase and cursor-motion sequences on a 177-column grid; that is
  where the remaining per-cell work lives. Not measured further here, so this
  is an observation, not a diagnosis.
- Scrollback halves `agent`: each scrolled-off line is packed into the history
  as it leaves.

## Checkpoint size and cost

Not in the bench. `tests/checkpoint.rs::measure_checkpoint_cost` (`#[ignore]`)
measures checkpoint bytes, save time and restore time for a 24x80 and a 50x200
`Term` full of coloured build output:

```sh
cargo test --release --test checkpoint -- --ignored --nocapture measure_checkpoint_cost
```

On the machine above, 2026-10-18: `24x80: checkpoint 19709 bytes, save 26.7
us, restore 47.7 us` and `50x200: checkpoint 42142 bytes, save 107.6 us,
restore 66.6 us`. The sizes match the 0.5.4 changelog exactly (19.7 KB and
42.1 KB); the timings are within the run-to-run range of the original (23 / 63
us and 111 / 93 us).

## Where the changelog's figures came from

- **0.5.4** (`Term::feed` 188 -> 330 -> 570 -> 631 MiB/s): development
  measurements recorded in the commits that made each change (d39b249, 673dc5f,
  b7870bb), 64 MiB of generated agent output into a 177x47 `Term` on the
  machine above, with a harness that is not in this repository. They are not
  reproducible from the repo. This bench's `agent` variant is the in-repo
  equivalent (same grid, a different generated stream, so a different number):
  **544.6 MiB/s** on the 0.5.4 code. Compare against that from here on.
- **0.5.3** (120.6 -> 212.0 MiB/s): foldwave's `fwbench parse`, a 24x80 `Term`
  over 64 MiB, external to this repo (private, nativelite/foldwave).
- **0.5.1 and 0.5.2** (16.1 -> 70.2 -> 173.1 MB/s at 40x160 and the other
  grids): atrium's `cargo bench --bench terminal` with `ATRIUM_BENCH_ONLY=feed`,
  which feeds a 16 MB coloured log into a `vterm::Term`; in the atrium repo,
  `benches/README.md` there has the tables.
