//! Model picker popup of the profile editor: the harness's discovered models, fuzzy-filtered by what is typed,
//! with "unset" first and the typed text itself as a free-text choice when no model matches it exactly.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::fields::Target;
use crate::hit::{Hit, HitMap};
use crate::overlays::{self, LineInput, centered};
use crate::theme;

const W: u16 = 72;
const Z_PICKER: u8 = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Choice {
    Unset,
    Model(String),
    Typed(String),
}

impl Choice {
    pub fn model(&self) -> Option<String> {
        match self {
            Choice::Unset => None,
            Choice::Model(m) | Choice::Typed(m) => Some(m.clone()),
        }
    }
}

pub(crate) enum Outcome {
    Stay,
    Cancel,
    Pick(Option<String>),
}

pub(crate) struct Picker {
    pub profile: String,
    pub harness: String,
    models: Vec<String>,
    note: String,
    current: Option<String>,
    pub input: LineInput,
    sel: usize,
    scroll: usize,
}

/// Rank of `model` for query `q` (lower is better): exact, prefix, substring, then in-order subsequence.
fn rank(model: &str, q: &str) -> Option<(u8, usize)> {
    let m = model.to_lowercase();
    if m == q {
        return Some((0, 0));
    }
    if let Some(at) = m.find(q) {
        return Some((if at == 0 { 1 } else { 2 }, at));
    }
    let mut chars = m.char_indices();
    let mut first = None;
    for qc in q.chars() {
        let (at, _) = chars.find(|(_, c)| *c == qc)?;
        first.get_or_insert(at);
    }
    Some((3, first.unwrap_or(0)))
}

impl Picker {
    pub fn new(
        profile: String,
        harness: String,
        models: Vec<String>,
        note: String,
        current: Option<String>,
    ) -> Picker {
        let mut p = Picker {
            profile,
            harness,
            models,
            note,
            current,
            input: LineInput::default(),
            sel: 0,
            scroll: 0,
        };
        p.sel = p
            .items()
            .iter()
            .position(|c| c.model() == p.current)
            .unwrap_or(0);
        p
    }

    /// Replaces the model list (detection finished while the picker was open), keeping the selection in range.
    pub fn update(&mut self, models: Vec<String>, note: String) {
        self.models = models;
        self.note = note;
        let n = self.items().len();
        self.sel = self.sel.min(n.saturating_sub(1));
    }

    pub fn items(&self) -> Vec<Choice> {
        let q = self.input.text.trim().to_lowercase();
        if q.is_empty() {
            let mut out = vec![Choice::Unset];
            out.extend(self.models.iter().cloned().map(Choice::Model));
            return out;
        }
        let mut hits: Vec<((u8, usize), &String)> = self
            .models
            .iter()
            .filter_map(|m| rank(m, &q).map(|r| (r, m)))
            .collect();
        hits.sort_by_key(|(r, _)| *r);
        let exact = hits.first().is_some_and(|(r, _)| r.0 == 0);
        let mut out: Vec<Choice> = hits
            .into_iter()
            .map(|(_, m)| Choice::Model(m.clone()))
            .collect();
        if !exact {
            out.push(Choice::Typed(self.input.text.trim().to_string()));
        }
        out
    }

    fn pick(&self, i: usize) -> Outcome {
        match self.items().get(i) {
            Some(c) => Outcome::Pick(c.model()),
            None => Outcome::Stay,
        }
    }

    pub fn key(&mut self, key: &KeyEvent) -> Outcome {
        let n = self.items().len();
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter => return self.pick(self.sel),
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            _ => {
                if self.input.edit(key) {
                    self.sel = 0;
                    self.scroll = 0;
                }
            }
        }
        Outcome::Stay
    }

    pub fn click(&mut self, i: usize) -> Outcome {
        self.sel = i;
        self.pick(i)
    }

    pub fn wheel(&mut self, down: bool) {
        let n = self.items().len();
        self.sel = if down {
            (self.sel + 1).min(n.saturating_sub(1))
        } else {
            self.sel.saturating_sub(1)
        };
    }

    fn label(&self, c: &Choice) -> String {
        let text = match c {
            Choice::Unset => "unset (the harness's default model)".to_string(),
            Choice::Model(m) => m.clone(),
            Choice::Typed(t) => format!("use \"{t}\" (not in the list)"),
        };
        if c.model() == self.current {
            format!("{text}  ●")
        } else {
            text
        }
    }

    /// Draws the popup over `area`; clickable choices go to `regions` (and `hits`, so the modal body does not
    /// swallow them).
    pub fn draw(
        &mut self,
        f: &mut Frame,
        area: Rect,
        hits: &mut HitMap,
        regions: &mut Vec<(Rect, Target)>,
    ) {
        let items = self.items();
        let want = items.len() as u16 + 5;
        let rect = centered(area, W, want.clamp(8, area.height.max(8)));
        let title = format!("model for {}", self.profile);
        let inner = overlays::popup(f, rect, &title, hits);
        if inner.height < 3 {
            return;
        }
        let filter = Rect { height: 1, ..inner };
        self.input
            .draw(f, filter, "filter ›", "type to filter, or any model name");
        let note = format!("{} · {}", self.harness, self.note);
        f.render_widget(
            Paragraph::new(Line::styled(note, theme::dim())),
            Rect {
                y: inner.y + 1,
                height: 1,
                ..inner
            },
        );
        let list = Rect {
            y: inner.y + 2,
            height: inner.height - 2,
            ..inner
        };
        regions.push((list, Target::PickerList));
        let h = list.height as usize;
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + h {
            self.scroll = self.sel + 1 - h;
        }
        for (n, (i, c)) in items
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(h)
            .enumerate()
        {
            let row = Rect {
                y: list.y + n as u16,
                height: 1,
                ..list
            };
            let on = i == self.sel;
            let style = match (on, c) {
                (true, _) => Style::new()
                    .fg(theme::CYAN)
                    .bg(theme::SELECTED_BG)
                    .add_modifier(Modifier::BOLD),
                (false, Choice::Model(_)) => Style::new().fg(theme::FG),
                (false, _) => theme::dim(),
            };
            let marker = if on { "▸ " } else { "  " };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{marker}{}", self.label(c)),
                    style,
                ))),
                row,
            );
            regions.push((row, Target::PickerRow(i)));
            hits.push(row, Hit::Item(Z_PICKER, i));
        }
    }
}
