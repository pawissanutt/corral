use std::collections::{BTreeSet, HashSet};

use anyhow::Result;
use corral_core::{ArtifactTier, Store};
use corral_ops::{ArtifactRow, Ctx};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::hit::Hit;
use crate::shelf;
use crate::theme::{self, CYAN, MAGENTA, SELECTED_BG};
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View, hit};

pub mod image;
pub(crate) mod ui;

#[cfg(test)]
pub(crate) mod panels_fixture;
#[cfg(test)]
mod panels_tests;
#[cfg(test)]
mod tier_tests;

use ui::{KIND_COL, PreviewBody, Scroll};

/// Cycles the sort order (also the `[sort: …]` button).
const SORT_KEY: KeyEvent = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
/// Cycles the tier filter (also the `[tier: …]` button).
const TIER_KEY: KeyEvent = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);

/// Finder order: newest first (as loaded), or grouped by retention tier (keep, normal, transient) with newest
/// first inside each tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Sort {
    #[default]
    Newest,
    Tier,
}

impl Sort {
    fn label(self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::Tier => "tier",
        }
    }

    fn next(self) -> Sort {
        match self {
            Sort::Newest => Sort::Tier,
            Sort::Tier => Sort::Newest,
        }
    }
}

fn tier_rank(t: ArtifactTier) -> u8 {
    match t {
        ArtifactTier::Keep => 0,
        ArtifactTier::Normal => 1,
        ArtifactTier::Transient => 2,
    }
}

/// Tier filter cycle: all -> keep -> normal -> transient -> all.
fn next_tier(t: Option<ArtifactTier>) -> Option<ArtifactTier> {
    match t {
        None => Some(ArtifactTier::Keep),
        Some(ArtifactTier::Keep) => Some(ArtifactTier::Normal),
        Some(ArtifactTier::Normal) => Some(ArtifactTier::Transient),
        Some(ArtifactTier::Transient) => None,
    }
}

/// Mouse zones ([`Hit::Item`]): the results box, a result row and the preview box.
const Z_LIST: u8 = 0;
const Z_ROW: u8 = 1;
const Z_PREVIEW: u8 = 2;

/// Artifact finder (overlay in the board, full screen in Mode::Artifacts): filter input on top (typing
/// filters by task id, kind, project, title, tier), a control row (`[sort: …]` ctrl-t, `[tier: …]` ctrl-f; a
/// click presses the key), results list left (↑/↓, ctrl-n/ctrl-p; each row shows its tier badge), preview
/// right (`artifact_preview`; text and images rendered in place, others show a note) and ⏎ opens the selection
/// with its viewer (`corral_ops::open_artifact`), `*` pins or unpins it (pinned rows show ★). Esc -> Back. When
/// `only_task` is set, lists only that task's artifacts.
pub struct ArtifactsView {
    rows: Vec<ArtifactRow>,
    only_task: Option<String>,
    query: String,
    /// Indexes into `rows` matching `query`.
    matches: Vec<usize>,
    sel: usize,
    /// Snapshot path of the previewed artifact and its loaded preview.
    preview: Option<(String, PreviewBody)>,
    scroll: Scroll,
    /// Pin refs, for the ★ markers.
    pins: HashSet<String>,
    sort: Sort,
    /// Only this retention tier, or every tier.
    tier: Option<ArtifactTier>,
}

impl ArtifactsView {
    pub fn new(ctx: &Ctx, only_task: Option<&str>) -> Result<ArtifactsView> {
        let only_task = only_task.map(Store::normalize_id).transpose()?;
        let mut v = ArtifactsView {
            rows: Vec::new(),
            only_task,
            query: String::new(),
            matches: Vec::new(),
            sel: 0,
            preview: None,
            scroll: Scroll::default(),
            pins: shelf::pinned_refs(ctx),
            sort: Sort::default(),
            tier: None,
        };
        v.rows = v.load(ctx)?;
        v.refilter();
        v.ensure_preview(ctx);
        Ok(v)
    }

    fn load(&self, ctx: &Ctx) -> Result<Vec<ArtifactRow>> {
        let mut rows = corral_ops::all_artifacts(ctx)?;
        if let Some(t) = &self.only_task {
            rows.retain(|r| &r.task_id == t);
        }
        Ok(rows)
    }

    fn refilter(&mut self) {
        let tokens: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        self.matches = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                let tier = r.artifact.tier();
                if self.tier.is_some_and(|t| t != tier) {
                    return false;
                }
                let hay = format!(
                    "{} {} {} {} {} {}",
                    r.task_id,
                    r.artifact.kind.as_str(),
                    r.project.as_deref().unwrap_or(""),
                    r.artifact.title,
                    r.task_title,
                    tier.as_str()
                )
                .to_lowercase();
                tokens.iter().all(|t| hay.contains(t.as_str()))
            })
            .map(|(i, _)| i)
            .collect();
        if self.sort == Sort::Tier {
            // Stable: rows load newest first, so each tier group stays newest first.
            let rows = &self.rows;
            self.matches.sort_by_key(|&i| {
                rows.get(i)
                    .map_or(u8::MAX, |r| tier_rank(r.artifact.tier()))
            });
        }
        self.sel = self.sel.min(self.matches.len().saturating_sub(1));
    }

    /// Refilters and keeps the selected artifact selected when it still matches.
    fn refilter_keeping(&mut self) {
        let keep = self.selected().map(|r| r.artifact.path.clone());
        self.refilter();
        if let Some(p) = keep
            && let Some(n) = self
                .matches
                .iter()
                .position(|&i| self.rows.get(i).is_some_and(|r| r.artifact.path == p))
        {
            self.sel = n;
        }
    }

    /// Applies a sort or tier change (keyboard or button): reorders, keeps the selection, reloads its preview.
    fn reorder(&mut self, ctx: &Ctx) -> Action {
        self.refilter_keeping();
        self.ensure_preview(ctx);
        Action::None
    }

    fn sort_label(&self) -> String {
        format!("sort: {}", self.sort.label())
    }

    fn tier_label(&self) -> String {
        format!("tier: {}", self.tier.map_or("all", |t| t.as_str()))
    }

    /// "keep 1 · normal 3 · transient 2" over the listed rows (before the text and tier filters).
    fn tier_counts(&self) -> String {
        let mut n = [0usize; 3];
        for r in &self.rows {
            n[usize::from(tier_rank(r.artifact.tier()))] += 1;
        }
        [
            ArtifactTier::Keep,
            ArtifactTier::Normal,
            ArtifactTier::Transient,
        ]
        .iter()
        .zip(n)
        .map(|(t, c)| format!("{} {c}", t.as_str()))
        .collect::<Vec<_>>()
        .join(" · ")
    }

    fn selected(&self) -> Option<&ArtifactRow> {
        self.matches.get(self.sel).and_then(|&i| self.rows.get(i))
    }

    fn ensure_preview(&mut self, ctx: &Ctx) {
        let Some(row) = self.selected() else {
            self.preview = None;
            return;
        };
        if self
            .preview
            .as_ref()
            .is_some_and(|(p, _)| p == &row.artifact.path)
        {
            return;
        }
        let a = row.artifact.clone();
        let body = ui::load_preview(ctx, &a);
        self.preview = Some((a.path, body));
        self.scroll = Scroll::default();
    }

    fn move_sel(&mut self, delta: isize, ctx: &Ctx) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        self.sel = self.sel.saturating_add_signed(delta).min(n - 1);
        self.ensure_preview(ctx);
    }

    fn title(&self) -> String {
        let tasks: BTreeSet<&str> = self.rows.iter().map(|r| r.task_id.as_str()).collect();
        match &self.only_task {
            Some(t) => format!("artifacts · {t} · {}", self.rows.len()),
            None => format!(
                "artifacts · {} across {} tasks",
                self.rows.len(),
                tasks.len()
            ),
        }
    }

    fn result_lines(&self) -> Vec<Line<'static>> {
        if self.matches.is_empty() {
            let msg = if self.rows.is_empty() {
                "No artifacts yet. They appear on corral artifact add and at corral done."
            } else {
                "no artifacts match"
            };
            return vec![Line::from(Span::styled(msg, theme::dim()))];
        }
        self.matches
            .iter()
            .enumerate()
            .filter_map(|(n, &i)| self.rows.get(i).map(|r| (n, r)))
            .map(|(n, r)| {
                let selected = n == self.sel;
                let pinned = self
                    .pins
                    .contains(&shelf::artifact_ref(&r.task_id, &r.artifact));
                let mut spans = vec![
                    shelf::star(pinned),
                    ui::tier_badge(r.artifact.tier()),
                    Span::styled(
                        format!("{:<KIND_COL$}", r.artifact.kind.as_str()),
                        Style::new().fg(MAGENTA),
                    ),
                    Span::raw(r.artifact.title.clone()),
                    Span::styled(format!("  {} {}", r.task_id, r.artifact.run), theme::dim()),
                ];
                if let Some(p) = &r.project {
                    spans.push(Span::styled(
                        format!(" {p}"),
                        Style::new().fg(theme::project_color(p)),
                    ));
                }
                if selected {
                    spans
                        .iter_mut()
                        .for_each(|s| s.style = s.style.bg(SELECTED_BG));
                    Line::from(spans).style(Style::new().bg(SELECTED_BG))
                } else {
                    Line::from(spans)
                }
            })
            .collect()
    }
}

impl View for ArtifactsView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        self.ensure_preview(ctx);
        let w = (u32::from(area.width) * 88 / 100) as u16;
        let h = (u32::from(area.height) * 82 / 100) as u16;
        let rect = ui::centered(area, w.max(60), h.max(12));
        let inner = ui::popup(f, rect, &self.title(), None);
        hit::modal_popup(f, rect, hits);
        let [input, controls, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);

        let mut spans = vec![Span::styled("/ ", Style::new().fg(CYAN))];
        spans.extend(ui::input_spans(
            &self.query,
            "filter: kind, task, project, title",
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), input);

        let (sort_label, tier_label) = (self.sort_label(), self.tier_label());
        let end = hit::buttons(
            f,
            hits,
            controls.x,
            controls.y,
            controls.right(),
            &[(&sort_label, SORT_KEY), (&tier_label, TIER_KEY)],
        );
        let counts = Rect::new(
            end.saturating_add(1).min(controls.right()),
            controls.y,
            controls.right().saturating_sub(end.saturating_add(1)),
            1,
        );
        f.render_widget(
            Paragraph::new(Span::styled(self.tier_counts(), theme::dim())),
            counts,
        );

        let [left, right] = Layout::horizontal([Constraint::Fill(10), Constraint::Fill(13)])
            .spacing(1)
            .areas(body);
        let list_inner = ui::boxed(
            f,
            left,
            &format!("results {}", self.matches.len()),
            false,
            None,
        );
        let list = self.result_lines();
        let mut list_scroll = Scroll {
            offset: self
                .sel
                .saturating_sub(usize::from(list_inner.height.saturating_sub(1))),
        };
        ui::scrolled(f, list_inner, list, &mut list_scroll, false);
        hits.push(left, Hit::Item(Z_LIST, 0));
        let n = self.matches.len();
        hit::rows(hits, list_inner, Z_ROW, 0, n, list_scroll.offset);

        // The size/tier note outranks the file name: drop the name when both would overrun " preview ".
        let room = usize::from(right.width).saturating_sub(" preview ".len() + 5);
        let right_title = self
            .selected()
            .map(|r| ui::preview_title(&r.artifact, room));
        let prev_inner = ui::boxed(f, right, "preview", false, right_title);
        if let Some((_, body)) = &mut self.preview {
            ui::draw_preview(f, prev_inner, body, &mut self.scroll);
        }
        hits.push(right, Hit::Item(Z_PREVIEW, 0));
    }

    /// Wheel over the results moves the selection, over the preview scrolls it. A click selects a row; a click
    /// on the selected row (or a double-click) opens it like ⏎. The filter always has focus, so a click on it
    /// changes nothing. The sort and tier buttons are `Hit::Key`s the app feeds through `on_key`.
    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(dir) = hit::wheel(&ev) {
            match m.hit {
                Some(Hit::Item(Z_LIST | Z_ROW, _)) => self.move_sel(dir, m.ctx),
                Some(Hit::Item(Z_PREVIEW, _)) => self.scroll.wheel(dir),
                _ => {}
            }
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_ROW, n)) if hit::left(&ev) => {
                if n == self.sel || m.double {
                    return self.on_key(hit::press(KeyCode::Enter), m.ctx);
                }
                self.sel = n;
                self.ensure_preview(m.ctx);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Down => {
                self.move_sel(1, ctx);
                Action::None
            }
            KeyCode::Up => {
                self.move_sel(-1, ctx);
                Action::None
            }
            KeyCode::Char('n') if ctrl => {
                self.move_sel(1, ctx);
                Action::None
            }
            KeyCode::Char('p') if ctrl => {
                self.move_sel(-1, ctx);
                Action::None
            }
            KeyCode::Char('t') if ctrl => {
                self.sort = self.sort.next();
                self.reorder(ctx)
            }
            KeyCode::Char('f') if ctrl => {
                self.tier = next_tier(self.tier);
                self.reorder(ctx)
            }
            KeyCode::PageDown | KeyCode::PageUp => {
                self.scroll.on_key(&key, 10);
                Action::None
            }
            KeyCode::Char('*') => match self.selected() {
                Some(r) => shelf::toggle(ctx, &shelf::artifact_ref(&r.task_id, &r.artifact)),
                None => Action::None,
            },
            KeyCode::Enter => match self.selected() {
                Some(r) => Action::Toast(ui::open_toast(ctx, &r.artifact)),
                None => Action::Toast(Toast::new(
                    Tone::Warn,
                    "Nothing selected",
                    "no artifact matches the filter",
                )),
            },
            _ => {
                if ui::edit_text(&mut self.query, &key) {
                    self.sel = 0;
                    self.refilter();
                    self.ensure_preview(ctx);
                }
                Action::None
            }
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("type", "filter"),
            ("↑↓", "select"),
            ("ctrl+t", "sort"),
            ("ctrl+f", "tier"),
            ("pgup/pgdn", "scroll preview"),
            ("⏎", "open"),
            ("*", "pin"),
            ("esc", "close"),
        ]
    }

    fn refresh(&mut self, ctx: &Ctx) {
        let keep = self.selected().map(|r| r.artifact.path.clone());
        let Ok(rows) = self.load(ctx) else { return };
        self.rows = rows;
        self.pins = shelf::pinned_refs(ctx);
        self.refilter();
        if let Some(p) = keep
            && let Some(n) = self
                .matches
                .iter()
                .position(|&i| self.rows.get(i).is_some_and(|r| r.artifact.path == p))
        {
            self.sel = n;
        }
        // Tier, compression or purge state may have changed under the same path.
        self.preview = None;
        self.ensure_preview(ctx);
    }

    fn is_overlay(&self) -> bool {
        true
    }

    fn captures_text(&self) -> bool {
        true
    }

    fn has_unsaved_text(&self) -> bool {
        !self.query.is_empty()
    }
}
