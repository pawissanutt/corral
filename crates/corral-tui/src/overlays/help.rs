//! The `?` help: "getting around" in plain words with the configured terminal-engine keys, then every
//! board key. Any key closes it; `t` replays the welcome tour.

use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{centered, popup, truncate};
use crate::hit::{self, Hit, HitMap};
use crate::nav::NavKeys;
use crate::theme;
use crate::{Action, View};

const KEY_COL: usize = 18;

/// Board keys below the "getting around" block. Clicking a key presses it on the board.
pub(crate) const HELP_ROWS: [(&str, &str); 12] = [
    ("h j k l / arrows", "move selection"),
    ("d", "dispatch (profile, model, effort, mode)"),
    ("H / L", "move card left / right (backlog ⇄ ready)"),
    (
        ".",
        "card menu (also right-click or ⋯; drag a card to move it)",
    ),
    (
        "*",
        "pin / unpin the card (also artifacts, history, detail)",
    ),
    ("1-9 g G", "open pin 1-9 · focus the shelf · shelf panel"),
    ("n / c", "capture a new task or idea"),
    (
        "A Y M P ,",
        "artifacts · history · messages · PM · settings",
    ),
    ("i / D", "ideas only / show done"),
    (
        "O",
        "review autopilot on / off (also click it in the header)",
    ),
    ("esc ctrl+c", "back · quit from anywhere"),
    ("F8 F9 F10", "activate toast (newest first)"),
];

pub struct HelpView {
    nav: NavKeys,
}

impl HelpView {
    pub fn new(nav: NavKeys) -> HelpView {
        HelpView { nav }
    }

    /// (key label, meaning, clickable). Terminal-engine keys are not clickable: the board cannot press them.
    fn rows(&self) -> Vec<(String, String, bool)> {
        let row = |k: &str, v: &str, click| (k.to_string(), v.to_string(), click);
        let mut rows = vec![
            row("⏎", "open a card or its review", true),
            row("a", "open the agent's pane", true),
            row(
                &self.nav.board_short(),
                "back to this board from any pane",
                false,
            ),
            row("r", "answer an agent that needs you", true),
            row("q", "close board; agents keep running", true),
            row(
                &self.nav.detach.short(),
                "leave; `corral up` comes back",
                false,
            ),
            row("t", "replay the welcome tour", true),
            row("mouse", "click 'corral board' in the sidebar", false),
        ];
        rows.extend(HELP_ROWS.iter().map(|(k, v)| row(k, v, true)));
        rows
    }
}

impl View for HelpView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let rows = self.rows();
        let rect = centered(area, 76, rows.len() as u16 + 2);
        let inner = popup(f, rect, "help · getting around", hits);
        let text_w = (inner.width as usize).saturating_sub(KEY_COL);
        for (dy, (k, _, click)) in rows.iter().enumerate() {
            let y = inner.y + dy as u16;
            if y >= inner.bottom() {
                break;
            }
            if !click {
                continue;
            }
            for (off, w, key) in hit::row_keys(k) {
                let x = inner.x + off;
                if x < inner.right() {
                    let w = w.min(inner.right() - x);
                    hits.push(Rect::new(x, y, w, 1), Hit::Behind(key));
                }
            }
        }
        let lines: Vec<Line> = rows
            .into_iter()
            .map(|(k, v, clickable)| {
                // The getting-around lines fit a 55-column pane; longer board rows end in "…" there.
                let v = truncate(&v, text_w);
                let k = if clickable {
                    crate::keymap::label(&k, &_ctx.store.config.ui)
                } else {
                    k
                };
                Line::from(vec![
                    Span::styled(format!("{k:<KEY_COL$}"), Style::new().fg(theme::CYAN)),
                    Span::raw(v),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn on_key(&mut self, key: KeyEvent, _ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Char(c) if c == 't' || c == _ctx.store.config.ui.bound_key('t') => {
                Action::OpenTour
            }
            _ => Action::Back,
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("t", "tour"), ("any key", "close")]
    }

    fn is_overlay(&self) -> bool {
        true
    }
}
