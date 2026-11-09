//! What the program says rather than draws (`vterm::Notice`): titles and the
//! title stack, the bell, notifications and progress. Each vector is traced
//! against xterm's control sequences (OSC 0/2, XTWINOPS 22/23), ConEmu's
//! OSC 9 and Windows Terminal's OSC 9;4.

use vterm::{Notice, Term};

fn run(bytes: &[u8]) -> Term {
    let mut t = Term::new(4, 20);
    t.feed(bytes);
    t
}

fn row_text(t: &Term, row: usize) -> String {
    let s = t.screen();
    (0..s.cols()).map(|c| s.cell(row, c).ch).collect()
}

#[test]
fn osc_0_and_2_set_the_title_and_say_so() {
    let mut t = run(b"\x1b]0;first\x07x\x1b]2;second\x1b\\y");
    assert_eq!(t.title(), Some("second"));
    assert!(t.has_notices());
    assert_eq!(
        t.take_notices(),
        [
            Notice::Title("first".into()),
            Notice::Title("second".into())
        ]
    );
    assert!(!t.has_notices());
    assert!(t.take_notices().is_empty(), "taking leaves nothing behind");
    assert_eq!(row_text(&t, 0).trim_end(), "xy", "titles draw nothing");
}

#[test]
fn an_icon_name_and_other_osc_strings_are_not_notices() {
    let mut t = run(b"\x1b]1;icon\x07\x1b]133;A\x07\x1b]8;;http://x\x07\x1b]10;?\x07");
    assert_eq!(t.title(), None);
    assert!(t.take_notices().is_empty());
}

#[test]
fn a_title_split_across_chunks_arrives_whole() {
    let mut t = Term::new(4, 20);
    let bytes = b"\x1b]2;hello world\x07";
    for i in 1..bytes.len() {
        let mut t2 = Term::new(4, 20);
        t2.feed(&bytes[..i]);
        t2.feed(&bytes[i..]);
        assert_eq!(t2.title(), Some("hello world"), "split at {i}");
        assert_eq!(t2.take_notices(), [Notice::Title("hello world".into())]);
    }
    t.feed(bytes);
    assert_eq!(t.title(), Some("hello world"));
}

#[test]
fn the_title_stack_saves_and_restores() {
    // atrium's pattern: save the host's title, set its own, give it back.
    let mut t = run(b"\x1b]2;host\x07\x1b[22;2t\x1b]2;atrium \xc2\xb7 dev\x07\x1b[23;2t");
    assert_eq!(t.title(), Some("host"));
    assert_eq!(
        t.take_notices(),
        [
            Notice::Title("host".into()),
            Notice::Title("atrium · dev".into()),
            Notice::Title("host".into()),
        ]
    );
}

#[test]
fn the_stack_takes_the_bare_and_the_both_forms_and_pops_in_order() {
    let mut t = run(b"\x1b]2;a\x07\x1b[22t\x1b]2;b\x07\x1b[22;0t\x1b]2;c\x07");
    t.take_notices();
    t.feed(b"\x1b[23t");
    assert_eq!(t.title(), Some("b"));
    t.feed(b"\x1b[23;0t");
    assert_eq!(t.title(), Some("a"));
    assert_eq!(
        t.take_notices(),
        [Notice::Title("b".into()), Notice::Title("a".into())]
    );
}

#[test]
fn popping_an_empty_stack_changes_nothing() {
    let mut t = run(b"\x1b]2;a\x07");
    t.take_notices();
    t.feed(b"\x1b[23;2t\x1b[23;2t");
    assert_eq!(t.title(), Some("a"));
    assert!(t.take_notices().is_empty());
}

#[test]
fn saving_with_no_title_restores_an_empty_one() {
    let mut t = run(b"\x1b[22;2t\x1b]2;x\x07\x1b[23;2t");
    assert_eq!(t.title(), Some(""));
    assert_eq!(
        t.take_notices(),
        [Notice::Title("x".into()), Notice::Title("".into())]
    );
}

#[test]
fn the_stack_holds_ten_titles_and_forgets_the_oldest() {
    let mut t = Term::new(4, 20);
    for i in 0..12 {
        t.feed(format!("\x1b]2;t{i}\x07\x1b[22;2t").as_bytes());
    }
    t.take_notices();
    let mut restored = Vec::new();
    for _ in 0..12 {
        t.feed(b"\x1b[23;2t");
        if let Some(Notice::Title(s)) = t.take_notices().pop() {
            restored.push(s);
        }
    }
    let want: Vec<String> = (2..12).rev().map(|i| format!("t{i}")).collect();
    assert_eq!(
        restored, want,
        "ten restores, newest first; t0 and t1 are gone"
    );
}

#[test]
fn the_other_window_operations_are_ignored() {
    let mut t = run(b"\x1b]2;a\x07\x1b[1t\x1b[8;24;80t\x1b[14t\x1b[22;1t\x1b[23;1t\x1b[21t");
    t.take_notices();
    t.feed(b"\x1b[23;2t");
    assert_eq!(
        t.title(),
        Some("a"),
        "22;1 saved nothing, so 23;2 restored nothing"
    );
    assert!(t.take_notices().is_empty());
}

#[test]
fn a_bell_is_heard_once_and_draws_nothing() {
    let mut t = run(b"x\x07\x07\x07y");
    assert_eq!(row_text(&t, 0).trim_end(), "xy");
    assert_eq!(t.take_notices(), [Notice::Bell]);
    t.feed(b"\x07");
    assert_eq!(
        t.take_notices(),
        [Notice::Bell],
        "a later bell is heard again"
    );
}

#[test]
fn the_bell_that_ends_an_osc_string_is_not_a_bell() {
    let mut t = run(b"\x1b]2;quiet\x07");
    assert_eq!(t.take_notices(), [Notice::Title("quiet".into())]);
}

#[test]
fn bells_between_other_notices_are_each_heard() {
    let mut t = run(b"\x07\x1b]2;a\x07\x07");
    assert_eq!(
        t.take_notices(),
        [Notice::Bell, Notice::Title("a".into()), Notice::Bell]
    );
}

#[test]
fn osc_9_is_a_toast() {
    let mut t = run(b"\x1b]9;build done\x07\x1b]9;\x07");
    assert_eq!(
        t.take_notices(),
        [Notice::Toast("build done".into()), Notice::Toast("".into())]
    );
    assert_eq!(t.title(), None);
}

#[test]
fn osc_9_4_is_progress() {
    let mut t =
        run(b"\x1b]9;4;1;40\x07\x1b]9;4;3\x07\x1b]9;4;2;100\x07\x1b]9;4;0;0\x07\x1b]9;4;4;7\x1b\\");
    assert_eq!(
        t.take_notices(),
        [
            Notice::Progress {
                state: 1,
                percent: 40
            },
            Notice::Progress {
                state: 3,
                percent: 0
            },
            Notice::Progress {
                state: 2,
                percent: 100
            },
            Notice::Progress {
                state: 0,
                percent: 0
            },
            Notice::Progress {
                state: 4,
                percent: 7
            },
        ]
    );
}

#[test]
fn progress_is_clamped_and_nonsense_is_dropped() {
    let mut t = run(b"\x1b]9;4;1;250\x07\x1b]9;4;7;10\x07\x1b]9;4;x;10\x07\x1b]9;4;1;abc\x07\x1b]9;4;;5\x07\x1b]9;4;99999999999;5\x07");
    assert_eq!(
        t.take_notices(),
        [
            Notice::Progress {
                state: 1,
                percent: 100
            },
            Notice::Progress {
                state: 1,
                percent: 0
            },
        ]
    );
}

#[test]
fn a_flood_of_notices_keeps_the_newest_sixty_four() {
    let mut t = Term::new(4, 20);
    for i in 0..200 {
        t.feed(format!("\x1b]9;n{i}\x07").as_bytes());
    }
    let got = t.take_notices();
    assert_eq!(got.len(), 64);
    assert_eq!(got[0], Notice::Toast("n136".into()));
    assert_eq!(got[63], Notice::Toast("n199".into()));
}

#[test]
fn a_title_survives_a_resize_but_not_a_checkpoint() {
    let mut t = run(b"\x1b]2;kept\x07");
    t.resize(10, 40);
    assert_eq!(t.title(), Some("kept"));
    let bytes = t.checkpoint().expect("quiescent");
    let back = Term::restore(&bytes).expect("restores");
    assert_eq!(back.title(), None, "titles are not in a checkpoint");
}
