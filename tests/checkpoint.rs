//! Checkpoints restore exactly: restore, feed the rest, and the result is
//! indistinguishable from never having stopped.

use vterm::Term;

/// A stream that touches every piece of state a checkpoint must carry.
fn rich_stream() -> Vec<u8> {
    let mut s = Vec::new();
    for i in 0..30 {
        s.extend_from_slice(format!("line {i} plain text\r\n").as_bytes());
    }
    s.extend_from_slice(
        b"\x1b[1;31mbold red\x1b[0m \x1b[38;5;208m256\x1b[48;2;10;20;30mrgb\x1b[0m\r\n",
    );
    s.extend_from_slice(b"\x1b[3;4;7;9mital-under-rev-strike\x1b[0m\x1b[2mdim\x1b[22m\r\n");
    s.extend_from_slice(b"\x1b[5;10Hmoved\x1b7saved\x1b[20;1Helsewhere\x1b8back\r\n");
    s.extend_from_slice(b"\x1b[3g\x1b[1;5H\x1bH\x1b[1;13H\x1bH\r\ta\tb\tc\r\n");
    s.extend_from_slice(b"\x1b[4;12r");
    for i in 0..20 {
        s.extend_from_slice(format!("\x1b[12;1Hscroll region {i}\n").as_bytes());
    }
    s.extend_from_slice(b"\x1b[r\x1b[?1049halternate screen\x1b[2J\x1b[Hon alt\x1b[?1049l");
    s.extend_from_slice("wide: 中文字 and é\r\n".as_bytes());
    s.extend_from_slice(b"\x1b[24;75Hedge-wraps-here and on\r\n");
    s.extend_from_slice(b"\x1b[?7l\x1b[23;70Hno autowrap past the edge\x1b[?7h\r\n");
    s.extend_from_slice(b"\x1b[?25l\x1b[?2026hsync part one\x1b[1;1Hdrawn\x1b[?2026l\x1b[?25h");
    s.extend_from_slice(b"\x1b]0;a window title\x07\x1bPq#0;2;0;0;0\x1b\\");
    s.extend_from_slice(b"\x1b[2;2H\x1b[K\x1b[1J\x1b[3;3H\x1b[J done\r\n");
    s.extend_from_slice(b"\x1b[0m");
    s
}

fn fed(bytes: &[u8]) -> Term {
    let mut t = Term::new(24, 80);
    t.feed(bytes);
    t
}

#[test]
fn every_checkpointable_split_restores_exactly() {
    let stream = rich_stream();
    let whole = fed(&stream);
    let final_state = whole
        .checkpoint()
        .expect("the stream ends between sequences");
    let mut checked = 0;
    for split in 0..=stream.len() {
        let mut original = fed(&stream[..split]);
        let Some(ck) = original.checkpoint() else {
            continue; // mid-sequence or mid-character: not a checkpoint point
        };
        let mut restored = Term::restore(&ck).expect("a fresh checkpoint restores");
        assert_eq!(
            restored.checkpoint().as_deref(),
            Some(&ck[..]),
            "round trip at {split}"
        );
        original.feed(&stream[split..]);
        restored.feed(&stream[split..]);
        assert_eq!(
            restored.screen(),
            original.screen(),
            "screen after resuming at {split}"
        );
        assert_eq!(
            restored.checkpoint(),
            Some(final_state.clone()),
            "full state after resuming at {split}"
        );
        checked += 1;
    }
    // Most offsets are between sequences; make sure the test really ran.
    assert!(
        checked > stream.len() / 2,
        "only {checked} of {} splits checked",
        stream.len()
    );
}

#[test]
fn chunked_feeding_matches_one_shot_feeding_after_restore() {
    // The same stream fed in awkward chunks, checkpointing and restoring at
    // every chunk boundary that allows it, ends in the same state.
    let stream = rich_stream();
    let expected = fed(&stream).checkpoint().unwrap();
    for chunk in [1usize, 3, 7, 64, 1000] {
        let mut t = Term::new(24, 80);
        for piece in stream.chunks(chunk) {
            t.feed(piece);
            if let Some(ck) = t.checkpoint() {
                t = Term::restore(&ck).unwrap();
            }
        }
        assert_eq!(t.checkpoint().unwrap(), expected, "chunk size {chunk}");
    }
}

#[test]
fn no_checkpoint_mid_sequence_or_mid_character() {
    for partial in [
        &b"\x1b"[..],
        b"\x1b[1;3",
        b"\x1b]0;tit",
        b"\x1bPdata",
        "é".as_bytes().split_at(1).0,
    ] {
        let t = fed(partial);
        assert!(!t.can_checkpoint(), "{partial:?}");
        assert_eq!(t.checkpoint(), None, "{partial:?}");
    }
    assert!(fed(b"plain").can_checkpoint());
}

#[test]
fn damaged_checkpoints_are_rejected_never_panic() {
    let ck = fed(&rich_stream()).checkpoint().unwrap();
    for len in 0..ck.len() {
        assert!(Term::restore(&ck[..len]).is_err(), "truncated to {len}");
    }
    let mut longer = ck.clone();
    longer.push(0);
    assert!(Term::restore(&longer).is_err(), "trailing byte");
    // Every single-byte change either fails cleanly or restores something.
    for i in 0..ck.len() {
        let mut bad = ck.clone();
        bad[i] ^= 0xFF;
        let _ = Term::restore(&bad);
    }
    assert!(Term::restore(b"not a checkpoint at all").is_err());
}

#[test]
fn checkpoints_are_small_for_ordinary_screens() {
    let ck = fed(&rich_stream()).checkpoint().unwrap();
    // Two 24x80 screens are 3,840 cells; run-length encoding keeps a mostly
    // blank terminal to a few KiB at most.
    assert!(ck.len() < 32 * 1024, "checkpoint is {} bytes", ck.len());
}

/// Manual measurement, not part of the gate:
/// `cargo test --release --test checkpoint -- --ignored --nocapture`.
#[test]
#[ignore]
fn measure_checkpoint_cost() {
    use std::time::Instant;
    let mut agent = Vec::new();
    for i in 0..5000 {
        agent.extend_from_slice(
            format!(
                "\x1b[32m   Compiling\x1b[0m crate-{i} v0.{}.{} (build step {})\r\n",
                i % 9,
                i % 13,
                i * 7
            )
            .as_bytes(),
        );
    }
    for (rows, cols) in [(24usize, 80usize), (50, 200)] {
        let mut t = Term::new(rows, cols);
        t.feed(&agent);
        let n = 2000;
        let start = Instant::now();
        let mut ck = Vec::new();
        for _ in 0..n {
            ck = t.checkpoint().unwrap();
        }
        let save = start.elapsed() / n;
        let start = Instant::now();
        for _ in 0..n {
            std::hint::black_box(Term::restore(&ck).unwrap());
        }
        let load = start.elapsed() / n;
        println!(
            "{rows}x{cols}: checkpoint {} bytes, save {:.1} us, restore {:.1} us",
            ck.len(),
            save.as_secs_f64() * 1e6,
            load.as_secs_f64() * 1e6
        );
    }
}

#[test]
fn new_line_mode_survives_a_checkpoint() {
    let mut t = Term::new(4, 10);
    t.feed(b"\x1b[20hab");
    let ck = t.checkpoint().expect("at ground");
    let mut r = Term::restore(&ck).expect("restore");
    r.feed(b"\ncd");
    assert_eq!(r.screen().cell(1, 0).ch, 'c', "LNM still on after restore");
}
