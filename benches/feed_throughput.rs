//! `Term::feed` throughput over generated terminal output, in MiB/s.
//!
//! Run: `cargo bench --bench feed_throughput` (a release build). Under
//! `cargo test --all-targets` this target only builds and returns, so the
//! local gate stays fast.
//!
//! How it measures, so the numbers mean what they say:
//! - Each variant is one deterministic byte stream (a fixed-seed generator, so
//!   every run and every machine feeds the same bytes), built once up front.
//! - A run is: a fresh `Term`, then the whole stream fed in pane-reader-sized
//!   chunks (8 KiB by default), timed with `std::time::Instant` around the
//!   feeding only. Generation is not timed.
//! - Each variant is run `VTERM_BENCH_RUNS` times (default 5); the table
//!   reports the median, with the slowest and fastest run beside it so the
//!   spread is visible. Nothing is cherry-picked.
//! - The screen is read after every run (`std::hint::black_box`), so the
//!   compiler cannot discard the work.
//!
//! The variants, each `VTERM_BENCH_MIB` MiB (default 16) of CRLF-terminated
//! lines on a `VTERM_BENCH_ROWS` x `VTERM_BENCH_COLS` grid (default 47 x 177,
//! the grid behind the figures in the changelog). Every line ends at the
//! bottom row, so the screen scrolls on every line:
//! - `ascii`: plain printable ASCII, no escape sequences at all. The upper
//!   bound: the ASCII fast path and scrolling, nothing else.
//! - `agent`: what an agent CLI prints: cargo/test/log/diff lines, a colour
//!   change (SGR) on some of them. The changelog's headline figure.
//! - `sgr`: every word in its own colour, bold and reset, so the stream is
//!   roughly half escape sequences.
//! - `cursor`: a status region redrawn in place: cursor up, column 1, erase
//!   line, CUP, erase to end of screen, carriage-return progress bars.
//! - `utf8`: box-drawing, accented Latin, CJK and emoji mixed into the ASCII,
//!   so the UTF-8 decoder and the wide-glyph path are exercised.
//! - `mixed`: the five above interleaved line by line.
//! - `agent, history`: the `agent` stream into `Term::with_history` with a
//!   10,000-line scrollback, so every scrolled-off line is also recorded.
//!
//! Knobs (environment): `VTERM_BENCH_MIB`, `VTERM_BENCH_RUNS`,
//! `VTERM_BENCH_ROWS`, `VTERM_BENCH_COLS`, `VTERM_BENCH_CHUNK` (bytes per
//! `feed`), `VTERM_BENCH_ONLY` (one variant's name, e.g. `agent`).

use std::fmt::Write as _;
use std::time::Instant;

use vterm::Term;

const HISTORY_LINES: usize = 10_000;

fn main() {
    // `cargo test --all-targets` runs bench targets too; only `cargo bench`
    // passes `--bench`. Build-check under test, measure under bench.
    if !std::env::args().any(|a| a == "--bench") {
        println!("feed_throughput bench: skipped (run `cargo bench --bench feed_throughput`)");
        return;
    }
    let mib = env_num("VTERM_BENCH_MIB", 16);
    let runs = env_num("VTERM_BENCH_RUNS", 5);
    let rows = env_num("VTERM_BENCH_ROWS", 47);
    let cols = env_num("VTERM_BENCH_COLS", 177);
    let chunk = env_num("VTERM_BENCH_CHUNK", 8192);
    let only = std::env::var("VTERM_BENCH_ONLY").ok();

    println!("# nativelite-vterm feed throughput\n");
    println!(
        "- nativelite-vterm {} ({}), {} logical cores, {} {}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!(
        "- grid {rows}x{cols}, {mib} MiB per variant fed in {chunk}-byte chunks, \
         median of {runs} runs (min and max shown)\n"
    );
    println!("| variant | bytes | median MiB/s | min | max |");
    println!("| --- | --- | --- | --- | --- |");

    let target = mib * 1024 * 1024;
    let variants: [(&str, fn(&mut Gen, &mut String)); 6] = [
        ("ascii", line_ascii),
        ("agent", line_agent),
        ("sgr", line_sgr),
        ("cursor", line_cursor),
        ("utf8", line_utf8),
        ("mixed", line_mixed),
    ];
    for (name, line) in variants {
        if only.as_deref().map_or(true, |o| o == name) {
            let stream = generate(target, line);
            let stats = measure(&stream, runs, chunk, || Term::new(rows, cols));
            report(name, stream.len(), &stats);
        }
    }
    let name = "agent, history";
    if only.as_deref().map_or(true, |o| o == name) {
        let stream = generate(target, line_agent);
        let stats = measure(&stream, runs, chunk, || {
            Term::with_history(rows, cols, HISTORY_LINES)
        });
        report(name, stream.len(), &stats);
    }
    println!();
}

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// Feed `stream` into a fresh terminal `runs` times; MiB/s of every run,
/// sorted ascending.
fn measure(stream: &[u8], runs: usize, chunk: usize, fresh: impl Fn() -> Term) -> Vec<f64> {
    let mut rates = Vec::with_capacity(runs);
    for _ in 0..runs {
        let mut term = fresh();
        let start = Instant::now();
        for piece in stream.chunks(chunk) {
            term.feed(piece);
        }
        let secs = start.elapsed().as_secs_f64();
        // Read the result so the feeding cannot be optimised away.
        std::hint::black_box(term.screen().cursor());
        rates.push(stream.len() as f64 / (1024.0 * 1024.0) / secs);
    }
    rates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    rates
}

fn report(name: &str, bytes: usize, sorted: &[f64]) {
    let median = sorted[sorted.len() / 2];
    println!(
        "| {name} | {bytes} | {median:.1} | {:.1} | {:.1} |",
        sorted[0],
        sorted[sorted.len() - 1]
    );
}

/// `target` bytes (or a little more: whole lines) from `line`, deterministic.
fn generate(target: usize, line: fn(&mut Gen, &mut String)) -> Vec<u8> {
    let mut g = Gen::new(0x5EED_1234_ABCD_0001);
    let mut out = String::with_capacity(target + 512);
    while out.len() < target {
        line(&mut g, &mut out);
    }
    out.into_bytes()
}

/// A small deterministic generator (xorshift64*), so every run feeds the same
/// bytes and the numbers compare across runs and machines.
struct Gen {
    state: u64,
    /// Line counter, for the cursor variant's redraw cadence.
    n: usize,
}

impl Gen {
    fn new(seed: u64) -> Gen {
        Gen { state: seed, n: 0 }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const WORDS: &[&str] = &[
    "resmon",
    "panelog",
    "vterm",
    "governor",
    "indexer",
    "parser",
    "renderer",
    "atlas",
    "segment",
    "checkpoint",
    "throughput",
    "latency",
    "scrollback",
    "tokenizer",
];

/// SGR sequences an agent CLI typically emits.
const COLOURS: &[&str] = &[
    "\x1b[0m",
    "\x1b[1m",
    "\x1b[31m",
    "\x1b[32m",
    "\x1b[33m",
    "\x1b[36m",
    "\x1b[1;32m",
    "\x1b[38;5;208m",
];

/// Plain ASCII: words and numbers, no escapes.
fn line_ascii(g: &mut Gen, out: &mut String) {
    let words = 6 + g.below(10);
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        let w = g.pick(WORDS);
        let _ = write!(out, "{w}_{}", g.below(100_000));
    }
    out.push_str("\r\n");
}

/// Agent-style lines: build, test, log and diff output, coloured now and then.
fn line_agent(g: &mut Gen, out: &mut String) {
    let w = g.pick(WORDS);
    let a = g.below(1000);
    let b = g.below(100);
    match g.below(10) {
        0 => {
            let _ = write!(
                out,
                "\x1b[1;32m   Compiling\x1b[0m nativelite-{w} v0.{b}.{}",
                g.below(20)
            );
        }
        1 => {
            let _ = write!(
                out,
                "\x1b[1;32m    Finished\x1b[0m `release` profile [optimized] target(s) in {b}.{a}s"
            );
        }
        2 | 3 => {
            let _ = write!(out, "test {w}::tests::case_{a} ... \x1b[32mok\x1b[0m");
        }
        4 => {
            let _ = write!(
                out,
                "[INFO ] {w} worker {b} processed {} records in {a} ms",
                g.below(1_000_000)
            );
        }
        5 => {
            let _ = write!(
                out,
                "\x1b[33m[WARN ]\x1b[0m retrying {w} after transient failure (attempt {b})"
            );
        }
        6 => {
            let _ = write!(
                out,
                "\x1b[31m[ERROR]\x1b[0m {w}: expected {b} fields, found {}",
                g.below(50)
            );
        }
        7 => {
            let _ = write!(out, "\x1b[32m+    let value = compute({a})?;\x1b[0m");
        }
        8 => {
            let _ = write!(out, "\x1b[31m-    let value = compute_old({a})?;\x1b[0m");
        }
        _ => {
            let _ = write!(out, "     at crates/{w}/src/main.rs:{a}:{b}");
        }
    }
    out.push_str("\r\n");
}

/// Every word in its own colour: roughly half the bytes are SGR sequences.
fn line_sgr(g: &mut Gen, out: &mut String) {
    let words = 6 + g.below(10);
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        let c = g.pick(COLOURS);
        let w = g.pick(WORDS);
        let _ = write!(out, "{c}{w}\x1b[0m");
    }
    out.push_str("\r\n");
}

/// A status area redrawn in place: progress bars over `\r`, cursor up and
/// erase-line to repaint earlier rows, an occasional CUP plus erase-to-end.
fn line_cursor(g: &mut Gen, out: &mut String) {
    g.n += 1;
    let pct = g.below(101);
    let filled = pct * 30 / 100;
    // A progress bar rewritten on the same row, then committed with a newline.
    for step in 0..3 {
        let f = (filled + step).min(30);
        let _ = write!(out, "\r\x1b[K[");
        for i in 0..30 {
            out.push(if i < f { '=' } else { ' ' });
        }
        let _ = write!(out, "] {pct:>3}% {}", g.pick(WORDS));
    }
    out.push_str("\r\n");
    if g.n % 4 == 0 {
        // Repaint the two rows above, then come back down.
        let _ = write!(
            out,
            "\x1b[2A\x1b[G\x1b[2K  done: {}\x1b[2B\x1b[G",
            g.below(1000)
        );
    }
    if g.n % 40 == 0 {
        // Home, erase to the end of the screen, redraw a header.
        let _ = write!(
            out,
            "\x1b[1;1H\x1b[J== {} ==\x1b[{};1H",
            g.pick(WORDS),
            5 + g.below(10)
        );
    }
}

const UTF8_WORDS: &[&str] = &[
    "café",
    "naïve",
    "Zürich",
    "→",
    "✓",
    "✗",
    "│",
    "├──",
    "└──",
    "終了",
    "日本語",
    "데이터",
    "🚀",
    "📦",
    "—",
    "…",
];

/// ASCII with non-ASCII words mixed in, wide glyphs included.
fn line_utf8(g: &mut Gen, out: &mut String) {
    let words = 6 + g.below(10);
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        if g.below(3) == 0 {
            out.push_str(g.pick(UTF8_WORDS));
        } else {
            let _ = write!(out, "{}_{}", g.pick(WORDS), g.below(10_000));
        }
    }
    out.push_str("\r\n");
}

/// The other five, one line each in turn.
fn line_mixed(g: &mut Gen, out: &mut String) {
    match g.below(5) {
        0 => line_ascii(g, out),
        1 => line_agent(g, out),
        2 => line_sgr(g, out),
        3 => line_cursor(g, out),
        _ => line_utf8(g, out),
    }
}
