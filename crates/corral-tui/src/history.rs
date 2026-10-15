//! History view (`Y` on the board, or the Done column's `+N older` foot): every Done and Dropped task, newest
//! first, with a finder-style filter. ⏎ opens the task detail, whose Runs, Artifacts and Diff tabs read the
//! snapshots once the worktree is gone. `*` pins or unpins the selected task (pinned rows show ★).

use std::collections::HashSet;

use anyhow::Result;
use corral_core::query::{Query, SearchContext};
use corral_core::{Status, Task};
use corral_ops::{Ctx, Finish};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::artifacts::ui::{self, fit};
use crate::hit::{self, Hit};
use crate::shelf;
use crate::theme::{self, CYAN, GREEN, RED, SELECTED_BG};
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View};

#[cfg(test)]
mod tests;

/// Mouse zones ([`Hit::Item`]): the results box and a result row.
const Z_LIST: u8 = 0;
const Z_ROW: u8 = 1;
const BACK: &str = "‹ back";
const PLACEHOLDER: &str = "filter: text, project:<name>, kind:<name>";
/// Column widths after the status glyph: id, project, finished, sha, artifacts (title takes the rest).
const ID_W: usize = 7;
const PROJECT_W: usize = 15;
const FINISHED_W: usize = 11;
const SHA_W: usize = 15;
const ART_W: usize = 4;

pub struct HistoryView {
    /// Done and Dropped tasks with their finish records, newest first.
    rows: Vec<(Task, Finish)>,
    query: String,
    error: Option<String>,
    /// Indexes into `rows` matching `query`.
    matches: Vec<usize>,
    sel: usize,
    /// First visible result row.
    offset: usize,
    /// Pin refs, for the ★ markers.
    pins: HashSet<String>,
}

impl HistoryView {
    pub fn new(ctx: &Ctx) -> Result<HistoryView> {
        let mut v = HistoryView {
            rows: corral_ops::finished_tasks(&ctx.store)?,
            query: String::new(),
            error: None,
            matches: Vec::new(),
            sel: 0,
            offset: 0,
            pins: shelf::pinned_refs(ctx),
        };
        v.refilter();
        Ok(v)
    }

    /// Use the shared query language while retaining newest-first history order.
    fn refilter(&mut self) {
        let query = match Query::parse(&self.query, Timestamp::now()) {
            Ok(query) => {
                self.error = None;
                query
            }
            Err(error) => {
                self.error = Some(format!("Invalid query: {error:#}"));
                self.matches.clear();
                self.sel = 0;
                return;
            }
        };
        self.matches = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, (t, f))| {
                let extra = f.landed.as_ref().map_or_else(String::new, |l| {
                    format!("{} {} {}", l.sha, l.strategy, l.strategy_label())
                });
                let pinned = self.pins.iter().any(|pin| {
                    pin == &t.id || pin.strip_prefix(&t.id).is_some_and(|s| s.starts_with(':'))
                });
                query
                    .score(
                        t,
                        SearchContext {
                            pinned,
                            extra_text: &extra,
                        },
                    )
                    .is_some()
            })
            .map(|(i, _)| i)
            .collect();
        self.sel = self.sel.min(self.matches.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&Task> {
        self.matches
            .get(self.sel)
            .and_then(|&i| self.rows.get(i))
            .map(|(t, _)| t)
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.matches.len();
        if n > 0 {
            self.sel = self.sel.saturating_add_signed(delta).min(n - 1);
        }
    }

    fn title(&self) -> String {
        let dropped = self
            .rows
            .iter()
            .filter(|(t, _)| t.status == Status::Dropped)
            .count();
        format!(
            "history · {} done · {dropped} dropped",
            self.rows.len() - dropped
        )
    }

    fn header_line(title_w: usize) -> Line<'static> {
        let cols = [
            ("", 2),
            ("id", ID_W),
            ("title", title_w),
            ("project", PROJECT_W),
            ("finished", FINISHED_W),
            ("sha", SHA_W),
            ("art", ART_W),
        ];
        Line::from(
            cols.iter()
                .map(|(h, w)| Span::styled(fit(h, *w), theme::dim()))
                .collect::<Vec<_>>(),
        )
    }

    fn compact_line(t: &Task, width: usize, selected: bool, pinned: bool) -> Line<'static> {
        let text = format!(
            "{} {} · {} · {}",
            t.id,
            t.title,
            t.status.as_str(),
            t.project.as_deref().unwrap_or("—")
        );
        let line = Line::from(vec![
            shelf::star(pinned),
            Span::raw(fit(&text, width.saturating_sub(2))),
        ]);
        if selected {
            line.style(Style::new().bg(SELECTED_BG).add_modifier(Modifier::BOLD))
        } else {
            line
        }
    }

    fn row_line(
        t: &Task,
        f: &Finish,
        title_w: usize,
        selected: bool,
        pinned: bool,
    ) -> Line<'static> {
        let (glyph, color) = match t.status {
            Status::Dropped => ("✗ ", RED),
            _ => ("✓ ", GREEN),
        };
        let project = t.project.as_deref().unwrap_or("—");
        let sha = f.landed.as_ref().map_or_else(
            || "—".to_string(),
            |l| format!("{} {}", l.short_sha(), l.strategy_label()),
        );
        let mut title_style = Style::new().fg(theme::FG);
        if selected {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let mut spans = vec![
            Span::styled(glyph, Style::new().fg(color)),
            Span::styled(fit(&t.id, ID_W), theme::bold()),
            shelf::star(pinned),
            Span::styled(fit(&t.title, title_w.saturating_sub(2)), title_style),
            Span::styled(
                fit(project, PROJECT_W),
                Style::new().fg(t
                    .project
                    .as_deref()
                    .map_or(theme::DIM, theme::project_color)),
            ),
            Span::styled(
                fit(&ui::local_date(f.at, "%Y-%m-%d"), FINISHED_W),
                theme::dim(),
            ),
            Span::styled(fit(&sha, SHA_W), Style::new().fg(color)),
            Span::styled(
                fit(&format!("◆ {}", t.artifacts.len()), ART_W),
                theme::dim(),
            ),
        ];
        if selected {
            spans
                .iter_mut()
                .for_each(|s| s.style = s.style.bg(SELECTED_BG));
        }
        Line::from(spans)
    }
}

impl View for HistoryView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        ui::paint_bg(f, area);
        let [head, _, input, error, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .horizontal_margin(1)
        .areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(BACK, Style::new().fg(CYAN)),
                Span::raw("  "),
                Span::styled(self.title(), theme::bold()),
            ])),
            head,
        );
        hits.push(
            Rect {
                width: (BACK.width() as u16).min(head.width),
                ..head
            },
            Hit::Key(hit::press(KeyCode::Esc)),
        );
        let mut spans = vec![Span::styled("/ ", Style::new().fg(CYAN))];
        spans.extend(ui::input_spans(&self.query, PLACEHOLDER));
        f.render_widget(Paragraph::new(Line::from(spans)), input);
        if let Some(message) = &self.error {
            f.render_widget(
                Paragraph::new(message.as_str()).style(Style::new().fg(RED)),
                error,
            );
        }

        let inner = ui::boxed(
            f,
            body,
            &format!("results {}", self.matches.len()),
            true,
            None,
        );
        hits.push(body, Hit::Item(Z_LIST, 0));
        if inner.height < 2 {
            return;
        }
        let fixed = 2 + ID_W + PROJECT_W + FINISHED_W + SHA_W + ART_W;
        let compact = usize::from(inner.width) < fixed + 8;
        let title_w = usize::from(inner.width).saturating_sub(fixed).max(8);
        let [header, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        let columns = if compact {
            Line::styled(
                fit(
                    "  id     title · status · project",
                    usize::from(inner.width),
                ),
                theme::dim(),
            )
        } else {
            Self::header_line(title_w)
        };
        f.render_widget(Paragraph::new(columns), header);
        if self.matches.is_empty() {
            let msg = if self.error.is_some() {
                ""
            } else if self.rows.is_empty() {
                "Nothing finished yet. Approved and dropped tasks appear here."
            } else {
                "no tasks match"
            };
            f.render_widget(Paragraph::new(Line::styled(msg, theme::dim())), list);
            return;
        }
        let h = usize::from(list.height).max(1);
        self.offset = self
            .offset
            .min(self.sel)
            .max((self.sel + 1).saturating_sub(h));
        let lines: Vec<Line<'static>> = self
            .matches
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(h)
            .filter_map(|(n, &i)| self.rows.get(i).map(|(t, fin)| (n, t, fin)))
            .map(|(n, t, fin)| {
                let pinned = self.pins.contains(&t.id);
                if compact {
                    Self::compact_line(t, usize::from(list.width), n == self.sel, pinned)
                } else {
                    Self::row_line(t, fin, title_w, n == self.sel, pinned)
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), list);
        hit::rows(hits, list, Z_ROW, 0, self.matches.len(), self.offset);
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Action::Back,
            KeyCode::Down => self.move_sel(1),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('n') if ctrl => self.move_sel(1),
            KeyCode::Char('p') if ctrl => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::Char('*') => {
                return match self.selected() {
                    Some(t) => shelf::toggle(ctx, &t.id),
                    None => Action::None,
                };
            }
            KeyCode::Enter => {
                return match self.selected() {
                    Some(t) => Action::OpenDetail(t.id.clone()),
                    None => Action::Toast(Toast::new(
                        Tone::Warn,
                        "Nothing selected",
                        "no finished task matches the filter",
                    )),
                };
            }
            _ => {
                if ui::edit_text(&mut self.query, &key) {
                    self.sel = 0;
                    self.offset = 0;
                    self.refilter();
                }
            }
        }
        Action::None
    }

    /// Wheel moves the selection; a click selects a row, a click on the selected row (or a double-click) opens
    /// it like ⏎. The filter always has focus.
    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(dir) = hit::wheel(&ev) {
            if matches!(m.hit, Some(Hit::Item(Z_LIST | Z_ROW, _))) {
                self.move_sel(dir);
            }
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_ROW, n)) if hit::left(&ev) => {
                let open = n == self.sel || m.double;
                self.sel = n;
                if open {
                    return self.on_key(hit::press(KeyCode::Enter), m.ctx);
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("type", "filter"),
            ("↑↓", "select"),
            ("⏎", "open"),
            ("*", "pin"),
            ("esc", "back"),
        ]
    }

    fn refresh(&mut self, ctx: &Ctx) {
        let keep = self.selected().map(|t| t.id.clone());
        let Ok(rows) = corral_ops::finished_tasks(&ctx.store) else {
            return;
        };
        self.rows = rows;
        self.pins = shelf::pinned_refs(ctx);
        self.refilter();
        if let Some(id) = keep
            && let Some(n) = self
                .matches
                .iter()
                .position(|&i| self.rows.get(i).is_some_and(|(t, _)| t.id == id))
        {
            self.sel = n;
        }
    }

    fn captures_text(&self) -> bool {
        true
    }
}
