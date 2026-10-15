//! Mouse plumbing shared by every view (contract: docs/DESIGN.md §10 "Input: keyboard and mouse").
//!
//! Each frame rebuilds a [`HitMap`]: views push `(Rect, Hit)` pairs while drawing, later pushes sit on top, and
//! [`HitMap::at`] returns the topmost hit under a cell. A [`Hit::Key`] stands for a key press, so the app feeds it
//! through the ordinary key path and mouse and keyboard share one implementation.

use std::time::{Duration, Instant};

use corral_core::Status;
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::theme;

/// Two left presses on the same cell within this window are a double-click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// What a screen region means to the mouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// A left click acts exactly like pressing this key (footer hints, buttons, `×`).
    Key(KeyEvent),
    /// Outside the topmost popup: a left click closes it like Esc unless it holds unsaved text.
    Dismiss,
    /// Swallows clicks (a popup's body) so they never reach what is drawn below.
    Inert,
    /// A header counter: focus this board column.
    Column(Status),
}

/// Per-frame hit regions, searched topmost-last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HitMap {
    hits: Vec<(Rect, Hit)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.hits.clear();
    }

    pub fn push(&mut self, rect: Rect, hit: Hit) {
        if rect.width > 0 && rect.height > 0 {
            self.hits.push((rect, hit));
        }
    }

    /// The topmost hit containing cell (`x`, `y`).
    pub fn at(&self, x: u16, y: u16) -> Option<&Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains((x, y).into()))
            .map(|(_, h)| h)
    }

    /// Makes `popup` modal: drops every hit drawn below it, maps the rest of `screen` to [`Hit::Dismiss`] and
    /// the popup body to [`Hit::Inert`]. Hits pushed afterwards (the popup's own buttons) sit on top.
    pub fn modal(&mut self, screen: Rect, popup: Rect) {
        self.hits.clear();
        self.push(screen, Hit::Dismiss);
        self.push(popup, Hit::Inert);
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Rect, Hit)> {
        self.hits.iter()
    }
}

/// What a view's [`crate::View::on_mouse`] gets besides the raw event.
pub struct MouseCtx<'a> {
    pub ctx: &'a Ctx,
    /// The topmost hit under the pointer, if any.
    pub hit: Option<Hit>,
    /// True when this left press completes a double-click.
    pub double: bool,
}

/// Source of "now" for double-click detection; tests inject a fake one.
pub trait Clock {
    fn now(&self) -> Instant;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Detects double-clicks: a press is double when the previous single press hit the same cell within
/// [`DOUBLE_CLICK`]. A double consumes the pair, so a third quick press starts over.
#[derive(Debug, Clone, Default)]
pub struct ClickTracker {
    last: Option<(Instant, u16, u16)>,
}

impl ClickTracker {
    pub fn press(&mut self, now: Instant, x: u16, y: u16) -> bool {
        let double = self.last.is_some_and(|(at, lx, ly)| {
            (lx, ly) == (x, y) && now.saturating_duration_since(at) <= DOUBLE_CLICK
        });
        self.last = if double { None } else { Some((now, x, y)) };
        double
    }
}

fn single_key(token: &str) -> Option<KeyEvent> {
    let plain = |code| Some(KeyEvent::new(code, KeyModifiers::NONE));
    match token {
        "⏎" => plain(KeyCode::Enter),
        "esc" | "any key" => plain(KeyCode::Esc),
        "⇥" => plain(KeyCode::Tab),
        "↑" => plain(KeyCode::Up),
        "↓" => plain(KeyCode::Down),
        "←" => plain(KeyCode::Left),
        "→" => plain(KeyCode::Right),
        "pgup" => plain(KeyCode::PageUp),
        "pgdn" => plain(KeyCode::PageDown),
        "ctrl+c" => Some(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        _ => {
            let mut chars = token.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => plain(KeyCode::Char(c)),
                _ => None,
            }
        }
    }
}

/// The keys a footer hint label names, with each key's column offset and width inside the label.
/// `"d"` → one key; `"h/l"`, `"1-4"` and `"↑↓"` → one key per glyph; `"type"` → none.
pub fn hint_keys(label: &str) -> Vec<(u16, u16, KeyEvent)> {
    if let Some(k) = single_key(label) {
        return vec![(0, label.width() as u16, k)];
    }
    let mut out = Vec::new();
    let mut x = 0u16;
    let separated = label.contains('/') || label.contains('-');
    let tokens: Vec<String> = if separated {
        label
            .split_inclusive(['/', '-'])
            .map(|t| t.trim_end_matches(['/', '-']).to_string())
            .collect()
    } else if label.chars().all(|c| "↑↓←→".contains(c)) {
        label.chars().map(String::from).collect()
    } else {
        return Vec::new();
    };
    for t in tokens {
        let w = t.width() as u16;
        if let Some(k) = single_key(&t) {
            out.push((x, w, k));
        }
        x += w + u16::from(separated);
    }
    out
}

/// Registers hits for a hint row as rendered by [`crate::overlays::hint_spans`] starting at (`x`, `y`), clipped
/// to `right`. A single-key hint is clickable over its key and label; a multi-key hint over each key glyph.
pub fn hint_hits(hits: &mut HitMap, hints: &[(&str, &str)], x: u16, y: u16, right: u16) {
    let mut cx = x;
    for (k, label) in hints {
        let kw = k.width() as u16;
        let total = kw + format!(" {label}  ").width() as u16;
        let keys = hint_keys(k);
        let clip = |from: u16, w: u16| {
            let from = from.min(right);
            Rect::new(from, y, w.min(right - from), 1)
        };
        match keys.as_slice() {
            [(0, w, key)] if *w == kw => hits.push(clip(cx, total - 2), Hit::Key(*key)),
            _ => {
                for (off, w, key) in &keys {
                    hits.push(clip(cx + off, *w), Hit::Key(*key));
                }
            }
        }
        cx = cx.saturating_add(total);
    }
}

/// Draws a clickable `×` on the popup's top border and makes the popup modal (see [`HitMap::modal`]).
pub fn modal_popup(f: &mut Frame, rect: Rect, hits: &mut HitMap) {
    hits.modal(f.area(), rect);
    if rect.width < 8 || rect.height == 0 {
        return;
    }
    let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
    f.render_widget(
        Span::styled(" × ", Style::new().fg(theme::FG).bg(theme::BG2)),
        close,
    );
    hits.push(
        close,
        Hit::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );
}

#[cfg(test)]
mod tests;
