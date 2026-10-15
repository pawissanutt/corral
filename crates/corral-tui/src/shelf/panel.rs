//! Shelf panel (`G` on the board, or a chip's menu): every pin in order with its target, ⏎ opens, `K`/`J`
//! reorder, `r` renames, `*` unpins.

use corral_core::PinTarget;
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Entry, icon, label};
use crate::artifacts::ui;
use crate::hit::{self, Hit};
use crate::overlays::{centered, popup, truncate};
use crate::theme;
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View};

/// [`Hit::Item`] zone of a panel row.
pub(crate) const Z_ROW: u8 = 24;

pub(crate) struct ShelfPanel {
    pub(crate) entries: Vec<Entry>,
    pub(crate) sel: usize,
    offset: usize,
    /// The label being typed for the selected pin (`r`).
    rename: Option<String>,
}

impl ShelfPanel {
    pub(crate) fn new(ctx: &Ctx, sel: usize) -> ShelfPanel {
        let mut p = ShelfPanel {
            entries: Vec::new(),
            sel,
            offset: 0,
            rename: None,
        };
        p.reload(ctx);
        p
    }

    fn reload(&mut self, ctx: &Ctx) {
        let tasks = ctx.store.list().unwrap_or_default();
        self.entries = super::load(ctx, &tasks);
        self.sel = self.sel.min(self.entries.len().saturating_sub(1));
    }

    fn row_line(&self, i: usize, e: &Entry, width: usize) -> Line<'static> {
        let selected = i == self.sel;
        let detail = match &e.target {
            PinTarget::Task { status, .. } => status.label().to_string(),
            PinTarget::Artifact {
                artifact, followed, ..
            } => {
                let latest = if *followed { " · latest" } else { "" };
                format!("{} {}{latest}", artifact.kind.as_str(), artifact.run)
            }
            PinTarget::Broken(reason) => format!("broken: {reason}"),
        };
        let name = match (&self.rename, selected) {
            (Some(text), true) => {
                let mut spans = vec![Span::styled(
                    format!("{:>2}  ", i + 1),
                    Style::new().fg(theme::CYAN),
                )];
                spans.push(Span::styled("label › ", Style::new().fg(theme::CYAN)));
                spans.extend(ui::input_spans(text, "type a label, ⏎ saves, empty clears"));
                return Line::from(spans).style(Style::new().bg(theme::SELECTED_BG));
            }
            _ => label(e),
        };
        let fixed = 4 + 2 + 2 + e.pin.target.len() + 2 + detail.len();
        let name_w = width.saturating_sub(fixed).clamp(8, 40);
        let color = match e.target {
            PinTarget::Broken(_) => theme::RED,
            _ => theme::FG,
        };
        let mut name_style = Style::new().fg(color);
        if selected {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        let mut spans = vec![
            Span::styled(format!("{:>2}  ", i + 1), Style::new().fg(theme::CYAN)),
            Span::styled(
                format!("{} ", icon(&e.target)),
                Style::new().fg(theme::MAGENTA),
            ),
            Span::styled(ui::fit(&truncate(&name, name_w), name_w), name_style),
            Span::styled(format!("  {}", e.pin.target), theme::dim()),
            Span::styled(format!("  {detail}"), theme::dim()),
        ];
        if selected {
            spans
                .iter_mut()
                .for_each(|s| s.style = s.style.bg(theme::SELECTED_BG));
            return Line::from(spans).style(Style::new().bg(theme::SELECTED_BG));
        }
        Line::from(spans)
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.entries.len();
        if n > 0 {
            self.sel = self.sel.saturating_add_signed(delta).min(n - 1);
        }
    }

    /// Moves the selected pin up (`-1`) or down (`+1`) in the shelf order.
    fn reorder(&mut self, delta: isize, ctx: &Ctx) -> Action {
        if self.entries.is_empty() {
            return Action::None;
        }
        let to = self.sel.saturating_add_signed(delta);
        if to == self.sel || to >= self.entries.len() {
            return Action::None;
        }
        match ctx.store.move_pin(self.sel, to) {
            Ok(at) => {
                self.sel = at;
                self.reload(ctx);
                Action::Reload
            }
            Err(e) => Action::Toast(Toast::new(
                Tone::Bad,
                "Could not move the pin",
                format!("{e:#}"),
            )),
        }
    }

    fn rename_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        let Some(text) = &mut self.rename else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => self.rename = None,
            KeyCode::Enter => {
                let text = std::mem::take(text);
                self.rename = None;
                return match ctx.store.label_pin(self.sel, Some(&text)) {
                    Ok(_) => {
                        self.reload(ctx);
                        Action::Reload
                    }
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        "Could not rename the pin",
                        format!("{e:#}"),
                    )),
                };
            }
            _ => {
                ui::edit_text(text, key);
            }
        }
        Action::None
    }
}

impl View for ShelfPanel {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let w = (u32::from(area.width) * 80 / 100).clamp(40, 110) as u16;
        let h = (self.entries.len() as u16 + 4).clamp(6, area.height.saturating_sub(2).max(6));
        let rect = centered(area, w, h);
        let inner = popup(f, rect, &format!("pinned · {}", self.entries.len()), hits);
        let [list, _, foot] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Fill(1),
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(1),
        ])
        .areas(inner);
        if self.entries.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled(
                    "Nothing pinned. * pins the selected card or artifact.",
                    theme::dim(),
                )),
                list,
            );
            return;
        }
        let rows = usize::from(list.height).max(1);
        self.offset = self
            .offset
            .min(self.sel)
            .max((self.sel + 1).saturating_sub(rows));
        let width = usize::from(list.width);
        let lines: Vec<Line<'static>> = self
            .entries
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(rows)
            .map(|(i, e)| self.row_line(i, e, width))
            .collect();
        f.render_widget(Paragraph::new(lines), list);
        hit::rows(hits, list, Z_ROW, 0, self.entries.len(), self.offset);
        f.render_widget(
            Paragraph::new(Line::styled(
                "1-9 open · K/J reorder · r rename · * unpin",
                theme::dim(),
            )),
            foot,
        );
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        if self.rename.is_some() {
            return self.rename_key(&key, ctx);
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('G') => Action::Back,
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_sel(1);
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_sel(-1);
                Action::None
            }
            KeyCode::Char('J') => self.reorder(1, ctx),
            KeyCode::Char('K') => self.reorder(-1, ctx),
            KeyCode::Char('r') if !self.entries.is_empty() => {
                self.rename = Some(self.entries[self.sel].pin.label.clone().unwrap_or_default());
                Action::None
            }
            KeyCode::Char('*') | KeyCode::Char('x') | KeyCode::Delete => {
                match self.entries.get(self.sel).map(|e| e.pin.target.clone()) {
                    Some(target) => {
                        let action = super::toggle(ctx, &target);
                        self.reload(ctx);
                        action
                    }
                    None => Action::None,
                }
            }
            KeyCode::Enter if !self.entries.is_empty() => Action::OpenPin(self.sel),
            KeyCode::Char(c @ '1'..='9') => Action::OpenPin(usize::from(c as u8 - b'1')),
            _ => Action::None,
        }
    }

    /// Wheel moves the selection; a click selects a row, a click on the selected row (or a double-click) opens it.
    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if self.rename.is_some() {
            return Action::None;
        }
        if let Some(dir) = hit::wheel(&ev) {
            self.move_sel(dir);
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_ROW, i)) if hit::left(&ev) => {
                if i == self.sel || m.double {
                    return Action::OpenPin(i);
                }
                self.sel = i;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.rename.is_some() {
            return vec![("⏎", "save label"), ("esc", "cancel")];
        }
        vec![
            ("j/k", "select"),
            ("⏎", "open"),
            ("K/J", "reorder"),
            ("r", "rename"),
            ("*", "unpin"),
            ("esc", "close"),
        ]
    }

    fn is_overlay(&self) -> bool {
        true
    }

    fn captures_text(&self) -> bool {
        self.rename.is_some()
    }

    fn has_unsaved_text(&self) -> bool {
        self.rename.as_ref().is_some_and(|t| !t.is_empty())
    }
}
