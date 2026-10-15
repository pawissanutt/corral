use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use super::{ClickTracker, Hit, HitMap, hint_hits, hint_keys};

fn k(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn c(ch: char) -> KeyEvent {
    k(KeyCode::Char(ch))
}

#[test]
fn hit_map_picks_topmost_rect() {
    let mut hits = HitMap::default();
    hits.push(Rect::new(0, 0, 20, 10), Hit::Inert);
    hits.push(Rect::new(5, 2, 4, 1), Hit::Key(c('d')));
    hits.push(Rect::new(7, 2, 4, 1), Hit::Key(c('a')));
    hits.push(Rect::new(1, 1, 0, 3), Hit::Dismiss);
    assert_eq!(hits.at(5, 2), Some(&Hit::Key(c('d'))));
    assert_eq!(hits.at(8, 2), Some(&Hit::Key(c('a'))), "later push wins");
    assert_eq!(hits.at(1, 1), Some(&Hit::Inert), "empty rects are dropped");
    assert_eq!(hits.at(19, 9), Some(&Hit::Inert));
    assert_eq!(hits.at(20, 9), None, "right edge is exclusive");

    hits.modal(Rect::new(0, 0, 80, 24), Rect::new(10, 5, 20, 5));
    assert_eq!(
        hits.at(5, 2),
        Some(&Hit::Dismiss),
        "modal hides what is below"
    );
    assert_eq!(hits.at(12, 6), Some(&Hit::Inert));
}

#[test]
fn hint_labels_name_their_keys() {
    let keys = |l: &str| -> Vec<(u16, u16, KeyEvent)> { hint_keys(l) };
    assert_eq!(keys("d"), vec![(0, 1, c('d'))]);
    assert_eq!(keys("⏎"), vec![(0, 1, k(KeyCode::Enter))]);
    assert_eq!(keys("esc"), vec![(0, 3, k(KeyCode::Esc))]);
    assert_eq!(keys("any key"), vec![(0, 7, k(KeyCode::Esc))]);
    assert_eq!(keys("⇥"), vec![(0, 1, k(KeyCode::Tab))]);
    assert_eq!(keys("h/l"), vec![(0, 1, c('h')), (2, 1, c('l'))]);
    assert_eq!(keys("H/L"), vec![(0, 1, c('H')), (2, 1, c('L'))]);
    assert_eq!(keys("1-4"), vec![(0, 1, c('1')), (2, 1, c('4'))]);
    assert_eq!(
        keys("↑↓"),
        vec![(0, 1, k(KeyCode::Up)), (1, 1, k(KeyCode::Down))]
    );
    assert_eq!(
        keys("pgup/pgdn"),
        vec![(0, 4, k(KeyCode::PageUp)), (5, 4, k(KeyCode::PageDown))]
    );
    assert_eq!(
        keys("ctrl+c"),
        vec![(
            0,
            6,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        )]
    );
    assert!(keys("type").is_empty(), "a word is not a key");
}

#[test]
fn hint_hits_follow_rendered_spans() {
    let mut hits = HitMap::default();
    // Rendered as "⏎ open  h/l agent  q quit  " from x = 10.
    hint_hits(
        &mut hits,
        &[("⏎", "open"), ("h/l", "agent"), ("q", "quit")],
        10,
        3,
        33,
    );
    let at = |x| hits.at(x, 3).cloned();
    assert_eq!(at(10), Some(Hit::Key(k(KeyCode::Enter))));
    assert_eq!(
        at(15),
        Some(Hit::Key(k(KeyCode::Enter))),
        "label clicks too"
    );
    assert_eq!(at(16), None, "gap between hints");
    assert_eq!(at(18), Some(Hit::Key(c('h'))));
    assert_eq!(at(19), None, "the slash is not a key");
    assert_eq!(at(20), Some(Hit::Key(c('l'))));
    assert_eq!(at(22), None, "a two-key hint's label is not a button");
    assert_eq!(at(29), Some(Hit::Key(c('q'))));
    assert_eq!(at(33), None, "clipped at the right edge");
    assert_eq!(hits.at(10, 4), None);
}

#[test]
fn double_click_uses_the_injected_time() {
    let t0 = Instant::now();
    let ms = |n| t0 + Duration::from_millis(n);
    let mut clicks = ClickTracker::default();
    assert!(!clicks.press(t0, 4, 4));
    assert!(clicks.press(ms(400), 4, 4), "400 ms is still a double");
    assert!(!clicks.press(ms(450), 4, 4), "a double consumes the pair");
    assert!(!clicks.press(ms(851), 4, 4), "401 ms later is a new click");
    assert!(!clicks.press(ms(900), 5, 4), "another cell");
    assert!(clicks.press(ms(1000), 5, 4));
}
