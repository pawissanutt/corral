//! Card menu (right-click a card, click its `⋯`, or press `.`): every board action valid for the card's status.
//! Choosing an item presses that item's key on the board, so the menu adds no logic of its own. A shelf chip's
//! menu (right-click a chip) works the same way with the focused-shelf keys.

use corral_core::{Status, Task};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::overlays::popup;
use crate::theme;
use crate::{Hit, HitMap};

/// One menu row: the board key it presses, how that key is shown, and what it does.
pub(crate) type Item = (KeyCode, &'static str, &'static str);

/// `Hit::Item` zone of the menu rows.
pub(crate) const ZONE: u8 = 0;

/// What a key does to an open menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pick {
    Stay,
    Close,
    /// Close the menu and press this key on the board.
    Choose(KeyEvent),
}

pub(crate) struct CardMenu {
    pub(crate) task_id: String,
    /// Set for a shelf chip's menu: the chip's 0-based pin position (items press focused-shelf keys).
    pub(crate) chip: Option<usize>,
    title: String,
    pub(crate) items: Vec<Item>,
    pub(crate) idx: usize,
    /// The card's rect on screen; the menu opens just below its first row.
    anchor: Option<Rect>,
}

/// The board actions valid for a card in `status`, in menu order.
pub(crate) fn items_for(status: Status, pinned: bool) -> Vec<Item> {
    let mut items = vec![if status == Status::Review {
        (KeyCode::Enter, "⏎", "review…")
    } else {
        (KeyCode::Enter, "⏎", "open")
    }];
    match status {
        Status::Idea | Status::Backlog => items.extend([
            (KeyCode::Char('d'), "d", "dispatch…"),
            (KeyCode::Char('L'), "L", "move to Ready"),
        ]),
        Status::Ready => items.extend([
            (KeyCode::Char('d'), "d", "dispatch…"),
            (KeyCode::Char('H'), "H", "move to Backlog"),
        ]),
        Status::Running => items.push((KeyCode::Char('a'), "a", "jump to agent")),
        Status::NeedsMe => items.extend([
            (KeyCode::Char('r'), "r", "reply…"),
            (KeyCode::Char('a'), "a", "jump to agent"),
        ]),
        Status::Review | Status::Done | Status::Dropped => {}
    }
    items.push((
        KeyCode::Char('*'),
        "*",
        if pinned { "unpin" } else { "pin" },
    ));
    items.push((KeyCode::Char('A'), "A", "artifacts"));
    items
}

/// A shelf chip's actions, in menu order (keys of the focused shelf strip).
pub(crate) const CHIP_ITEMS: [Item; 5] = [
    (KeyCode::Enter, "⏎", "open"),
    (KeyCode::Char('*'), "*", "unpin"),
    (KeyCode::Char('H'), "H", "move left"),
    (KeyCode::Char('L'), "L", "move right"),
    (KeyCode::Char('G'), "G", "shelf: rename, reorder…"),
];

impl CardMenu {
    pub(crate) fn new(task: &Task, anchor: Option<Rect>, pinned: bool) -> CardMenu {
        let mut items = items_for(task.status, pinned);
        if task.escalated() {
            items[0] = (KeyCode::Enter, "⏎", "review…");
            items.retain(|(key, _, _)| *key != KeyCode::Char('r'));
        }
        CardMenu {
            task_id: task.id.clone(),
            chip: None,
            title: task.id.clone(),
            items,
            idx: 0,
            anchor,
        }
    }

    /// The menu of shelf chip `i` (0-based), opening under `anchor`.
    pub(crate) fn chip(i: usize, anchor: Option<Rect>) -> CardMenu {
        CardMenu {
            task_id: String::new(),
            chip: Some(i),
            title: format!("pin {}", i + 1),
            items: CHIP_ITEMS.to_vec(),
            idx: 0,
            anchor,
        }
    }

    pub(crate) fn choose(&self, i: usize) -> Pick {
        match self.items.get(i) {
            Some((code, _, _)) => Pick::Choose(KeyEvent::new(*code, KeyModifiers::NONE)),
            None => Pick::Stay,
        }
    }

    /// ↑↓/j k move, ⏎ chooses the highlighted row, an item's own key chooses it, esc closes.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Pick {
        let n = self.items.len();
        match key.code {
            KeyCode::Esc => Pick::Close,
            KeyCode::Down | KeyCode::Char('j') => {
                self.idx = (self.idx + 1).min(n.saturating_sub(1));
                Pick::Stay
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.idx = self.idx.saturating_sub(1);
                Pick::Stay
            }
            KeyCode::Enter => self.choose(self.idx),
            code => match self.items.iter().position(|(c, _, _)| *c == code) {
                Some(i) => self.choose(i),
                None => Pick::Stay,
            },
        }
    }

    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("↑↓", "select"), ("⏎", "choose"), ("esc", "close")]
    }

    pub(crate) fn draw(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        let label_w = self
            .items
            .iter()
            .map(|(_, _, l)| l.width())
            .max()
            .unwrap_or(0);
        let w = (label_w as u16 + 8).max(self.title.width() as u16 + 10);
        let h = self.items.len() as u16 + 2;
        let (w, h) = (w.min(area.width), h.min(area.height));
        let (x, y) = match self.anchor {
            Some(a) => (a.right().saturating_sub(w), a.y.saturating_add(1)),
            None => (
                area.x + (area.width - w) / 2,
                area.y + (area.height - h) / 2,
            ),
        };
        let x = x.clamp(area.x, area.right() - w);
        let y = y.clamp(area.y, area.bottom() - h);
        let inner = popup(f, Rect::new(x, y, w, h), &self.title, hits);
        for (i, (_, key, label)) in self.items.iter().enumerate() {
            let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            if row.y >= inner.bottom() {
                break;
            }
            let mut style = Style::new().fg(theme::FG);
            if i == self.idx {
                style = style.bg(theme::SELECTED_BG).add_modifier(Modifier::BOLD);
            }
            let line = Line::from(vec![
                Span::styled(format!(" {key:<2}"), Style::new().fg(theme::CYAN)),
                Span::raw(format!(" {label}")),
            ]);
            f.render_widget(Paragraph::new(line).style(style), row);
            hits.push(row, Hit::Item(ZONE, i));
        }
    }
}
