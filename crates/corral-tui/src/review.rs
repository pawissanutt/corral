use std::collections::HashSet;

use anyhow::Result;
use corral_core::ProfileChoice;
use corral_core::RunOutcome;
use corral_ops::{ApproveStrategy, Ctx, DispatchOutcome, ReviewInfo};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use ratatui::Frame;

use crate::artifacts::list::ArtList;
use crate::artifacts::ui::{self, PreviewPopup, Scroll};
use crate::choice::{self, RunChoice};
use crate::hit::{self, Hit};
use crate::overlays;
use crate::shelf;
use crate::theme::{self, CYAN, DIM, FG, GREEN, RED, SELECTED_BG, YELLOW};
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View};

mod mouse;
#[cfg(test)]
mod panels_tests;

/// Mouse zones ([`Hit::Item`]): the three boxes, file rows, artifact rows and merge strategies.
const Z_SUMMARY: u8 = 0;
const Z_FILES: u8 = 1;
const Z_ARTS: u8 = 2;
const Z_FILE: u8 = 3;
const Z_ART: u8 = 4;
const Z_STRATEGY: u8 = 5;
const BACK: &str = "‹ back";

const STRATEGIES: [ApproveStrategy; 3] = [
    ApproveStrategy::FastForward,
    ApproveStrategy::Rebase,
    ApproveStrategy::KeepBranch,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Summary,
    Files,
    Artifacts,
}

struct MergeSheet {
    opt: usize,
    remove_worktree: bool,
    /// Worktree cleanliness, checked once when the sheet opens: Ok(clean) or the git error.
    worktree_clean: Result<bool, String>,
}

enum Mode {
    Normal,
    Merge(MergeSheet),
    Changes(String),
    Preview(PreviewPopup),
    /// Discard asks first: ⏎/y discards, esc/n cancels.
    ConfirmDiscard,
    /// Retry sheet: profile, model and effort for the new run, starting from the current run's.
    Retry(Box<RunChoice>),
}

/// Review screen (task in Review): three boxes Summary (run summary, outcome, runs history), Files (diff stat
/// list + scrollable diff), Artifacts (list; ⏎ preview). Keys: a -> merge sheet overlay inside this view
/// (strategies: fast-forward / rebase+ff / keep branch; w toggles remove worktree; shows main_ready preflight;
/// ⏎ calls corral_ops::approve -> BackWithToast), c -> one-line "changes ›" input (⏎ request_changes ->
/// BackWithToast), R -> retry sheet (profile/model/effort, defaulting to the current run's; ⏎ retry ->
/// BackWithToast), x -> discard -> BackWithToast, * pins the selected artifact (Artifacts focus) or the task,
/// Esc -> Back.
pub struct ReviewView {
    info: ReviewInfo,
    focus: Focus,
    summary_scroll: Scroll,
    files_scroll: Scroll,
    file_sel: Option<usize>,
    art_sel: usize,
    list: ArtList,
    mode: Mode,
    /// Pin refs, for the ★ markers.
    pins: HashSet<String>,
}

impl ReviewView {
    pub fn new(ctx: &Ctx, task_id: &str) -> Result<ReviewView> {
        let info = corral_ops::review_info(ctx, task_id)?;
        let list = ArtList::new(ctx, &info.artifacts);
        let lead = corral_ops::lead_artifact(&info.artifacts);
        let art_sel = lead.unwrap_or_else(|| list.first(&info.artifacts));
        let mode = lead.map_or(Mode::Normal, |i| {
            Mode::Preview(PreviewPopup::open(ctx, &info.artifacts[i]))
        });
        Ok(ReviewView {
            info,
            focus: Focus::Files,
            summary_scroll: Scroll::default(),
            files_scroll: Scroll::default(),
            file_sel: None,
            art_sel,
            list,
            mode,
            pins: shelf::pinned_refs(ctx),
        })
    }

    /// The ref `*` toggles: the selected artifact while the Artifacts box has focus, else the task.
    fn pin_target(&self) -> String {
        match self.info.artifacts.get(self.art_sel) {
            Some(a) if self.focus == Focus::Artifacts => shelf::artifact_ref(&self.info.task.id, a),
            _ => self.info.task.id.clone(),
        }
    }

    fn id(&self) -> String {
        self.info.task.id.clone()
    }

    fn header(&self) -> Line<'static> {
        let t = &self.info.task;
        let r = &self.info.run;
        let mut spans = vec![
            Span::styled(format!("Review {}", t.id), theme::bold()),
            Span::raw(" "),
            if self.pins.contains(&t.id) {
                shelf::star(true)
            } else {
                Span::raw(" ")
            },
            Span::styled(t.title.clone(), theme::bold()),
            Span::raw("  "),
        ];
        if let Some(p) = &t.project {
            spans.push(Span::styled(
                p.clone(),
                Style::new().fg(theme::project_color(p)),
            ));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!(
                "run {} · {} · {} · {} → {}",
                r.id,
                choice::run_title(r),
                ui::elapsed(r.started, r.ended),
                r.branch,
                r.base
            ),
            theme::dim(),
        ));
        Line::from(spans)
    }

    fn summary_lines(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let r = &self.info.run;
        let mut lines = match r.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(s) => ui::text_lines(s),
            None => vec![Line::from(Span::styled(
                "(no summary; the agent did not call corral done)",
                theme::dim(),
            ))],
        };
        lines.push(Line::default());
        let (outcome, color) = outcome_label(r.outcome);
        lines.push(Line::from(vec![
            Span::styled("outcome ", theme::dim()),
            Span::styled(outcome, Style::new().fg(color)),
        ]));
        let s = &self.info.stat;
        lines.push(Line::from(vec![
            Span::styled("diff    ", theme::dim()),
            Span::styled(format!("+{}", s.added), Style::new().fg(GREEN)),
            Span::raw(" "),
            Span::styled(format!("−{}", s.removed), Style::new().fg(RED)),
            Span::styled(format!(" across {} files", s.files.len()), theme::dim()),
        ]));
        lines.push(Line::from(Span::styled(
            format!("worktree {}", ui::rel_path(ctx, &r.worktree)),
            theme::dim(),
        )));
        lines.push(Line::default());
        let history: Vec<String> = self
            .info
            .task
            .runs
            .iter()
            .map(|run| {
                let label = choice::run_label(run);
                format!("{} {label} {}", run.id, outcome_label(run.outcome).0)
            })
            .collect();
        lines.push(Line::from(Span::styled(
            format!("Runs: {}", history.join(" · ")),
            theme::dim(),
        )));
        lines
    }

    fn files_lines(&self) -> Vec<Line<'static>> {
        let files = &self.info.stat.files;
        if files.is_empty() {
            return vec![Line::from(Span::styled(
                "No changes relative to the run base.",
                theme::dim(),
            ))];
        }
        let w = files
            .iter()
            .map(|f| f.path.chars().count())
            .max()
            .unwrap_or(0)
            .min(48)
            + 2;
        let mut lines: Vec<Line<'static>> = files
            .iter()
            .map(|f| {
                let mut spans = vec![
                    Span::raw(ui::fit(&f.path, w)),
                    Span::styled(format!("+{:<5}", f.added), Style::new().fg(GREEN)),
                    Span::styled(format!("−{}", f.removed), Style::new().fg(RED)),
                ];
                if f.untracked {
                    spans.push(Span::styled("  new", theme::dim()));
                }
                Line::from(spans)
            })
            .collect();
        lines.push(Line::default());
        lines.extend(ui::diff_lines(&self.info.diff));
        lines
    }

    fn artifact_lines(&self, width: usize) -> Vec<Line<'static>> {
        if self.info.artifacts.is_empty() {
            return vec![Line::from(Span::styled(
                "No artifacts for this run.",
                theme::dim(),
            ))];
        }
        let mut lines = self.list.lines(
            &self.info.artifacts,
            &self.info.task.id,
            self.art_sel,
            self.focus == Focus::Artifacts,
            &self.pins,
            width,
        );
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "⏎ open selected · A all artifacts",
            theme::dim(),
        )));
        lines
    }

    fn draw_merge(
        &self,
        f: &mut Frame,
        area: Rect,
        sheet: &MergeSheet,
        ctx: &Ctx,
        hits: &mut HitMap,
    ) {
        let t = &self.info.task;
        let r = &self.info.run;
        let labels = [
            format!("fast-forward {} ← {}", r.base, r.branch),
            format!("rebase onto {}, then fast-forward", r.base),
            "keep branch, mark done only".to_string(),
        ];
        let mut lines: Vec<Line<'static>> = labels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let on = i == sheet.opt;
                let text = format!(" {} {l} ", if on { "(•)" } else { "( )" });
                if on {
                    Line::from(Span::styled(
                        text,
                        Style::new()
                            .fg(theme::FG)
                            .bg(SELECTED_BG)
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ))
                } else {
                    Line::from(Span::raw(text))
                }
            })
            .collect();
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::raw(format!(
                " {} remove worktree {} ",
                if sheet.remove_worktree { "[x]" } else { "[ ]" },
                ui::rel_path(ctx, &r.worktree)
            )),
            Span::styled("w", Style::new().fg(CYAN)),
        ]));
        lines.push(Line::from(vec![
            Span::raw(" [x] keep artifact snapshots "),
            Span::styled("(always)", theme::dim()),
        ]));
        lines.push(Line::default());
        let mut pre = vec![Span::styled(" preflight: ", theme::dim())];
        match &sheet.worktree_clean {
            Ok(true) => pre.extend([
                Span::styled("✓", Style::new().fg(GREEN)),
                Span::raw(" worktree clean"),
            ]),
            Ok(false) => pre.extend([
                Span::styled("⚠", Style::new().fg(YELLOW)),
                Span::raw(" worktree has uncommitted changes"),
            ]),
            Err(e) => pre.extend([
                Span::styled("?", Style::new().fg(YELLOW)),
                Span::styled(format!(" worktree status: {e}"), theme::dim()),
            ]),
        }
        pre.push(Span::styled(" · ", theme::dim()));
        if self.info.main_ready {
            pre.extend([
                Span::styled("✓", Style::new().fg(GREEN)),
                Span::raw(format!(" main checkout on {} and clean", r.base)),
            ]);
        } else {
            pre.extend([
                Span::styled("✗", Style::new().fg(RED)),
                Span::styled(
                    format!(
                        " main checkout not on {} or has uncommitted changes",
                        r.base
                    ),
                    Style::new().fg(RED),
                ),
            ]);
        }
        lines.push(Line::from(pre));
        if !self.info.main_ready && sheet.opt < 2 {
            lines.push(Line::from(Span::styled(
                " merging will fail until the main checkout is ready; keep branch still works",
                theme::dim(),
            )));
        }
        lines.push(Line::default());
        let buttons_row = lines.len() as u16;
        lines.push(Line::default());
        let rect = ui::centered(area, 86, lines.len() as u16 + 2);
        let inner = ui::popup(f, rect, &format!("approve {}", t.id), None);
        hit::modal_popup(f, rect, hits);
        f.render_widget(Paragraph::new(lines).style(Style::new().fg(FG)), inner);
        hit::rows(hits, inner, Z_STRATEGY, 0, STRATEGIES.len(), 0);
        let worktree_row = STRATEGIES.len() as u16 + 1;
        if worktree_row < inner.height {
            let row = Rect::new(inner.x, inner.y + worktree_row, inner.width, 1);
            hits.push(row, Hit::Key(hit::press(KeyCode::Char('w'))));
        }
        if buttons_row < inner.height {
            let buttons = [
                ("Approve", hit::press(KeyCode::Enter)),
                ("Cancel", hit::press(KeyCode::Esc)),
            ];
            let y = inner.y + buttons_row;
            hit::buttons(f, hits, inner.x + 1, y, inner.right(), &buttons);
        }
    }

    fn on_merge_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        let Mode::Merge(sheet) = &mut self.mode else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                sheet.opt = (sheet.opt + 1).min(STRATEGIES.len() - 1)
            }
            KeyCode::Char('k') | KeyCode::Up => sheet.opt = sheet.opt.saturating_sub(1),
            KeyCode::Char('w') => sheet.remove_worktree = !sheet.remove_worktree,
            KeyCode::Enter => {
                let strategy = STRATEGIES
                    .get(sheet.opt)
                    .copied()
                    .unwrap_or(ApproveStrategy::KeepBranch);
                let remove = sheet.remove_worktree;
                let id = self.id();
                return match corral_ops::approve(ctx, &id, strategy, remove, false) {
                    Ok(msg) => {
                        Action::BackWithToast(Toast::new(Tone::Good, format!("✓ {id} done"), msg))
                    }
                    Err(e) => {
                        self.mode = Mode::Normal;
                        Action::Toast(Toast::new(
                            Tone::Bad,
                            format!("approve {id} failed"),
                            format!("{e:#}"),
                        ))
                    }
                };
            }
            _ => {}
        }
        Action::None
    }

    fn on_changes_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        let Mode::Changes(text) = &mut self.mode else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                let msg = text.trim().to_string();
                if msg.is_empty() {
                    return Action::Toast(Toast::new(
                        Tone::Warn,
                        "Nothing to send",
                        "type what the agent should change",
                    ));
                }
                let id = self.id();
                return match corral_ops::request_changes(ctx, &id, &msg) {
                    Ok(_) => Action::BackWithToast(Toast::new(
                        Tone::Info,
                        format!("↩ changes sent to {id}"),
                        "same session, context kept; back in Running",
                    )),
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        format!("request changes {id} failed"),
                        corral_herdr::friendly_error(&e),
                    )),
                };
            }
            _ => {
                ui::edit_text(text, key);
            }
        }
        Action::None
    }

    fn on_normal_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Char('a') => {
                let worktree_clean =
                    corral_git::is_clean(&self.info.run.worktree).map_err(|e| format!("{e:#}"));
                self.mode = Mode::Merge(MergeSheet {
                    opt: 1,
                    remove_worktree: true,
                    worktree_clean,
                });
                Action::None
            }
            KeyCode::Char('c') => {
                self.mode = Mode::Changes(String::new());
                Action::None
            }
            KeyCode::Char('R') => {
                let r = &self.info.run;
                self.mode = Mode::Retry(Box::new(RunChoice::new(
                    &ctx.store.config,
                    &r.agent,
                    r.model.clone(),
                    r.effort.clone(),
                )));
                Action::None
            }
            KeyCode::Char('x') => {
                self.mode = Mode::ConfirmDiscard;
                Action::None
            }
            KeyCode::Char('A') => Action::OpenArtifacts,
            KeyCode::Char('i') => Action::OpenAcceptance(self.id()),
            KeyCode::Char('*') => shelf::toggle(ctx, &self.pin_target()),
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let len = self.info.stat.files.len();
                if len > 0 {
                    let next = match (self.file_sel, key.code) {
                        (None, KeyCode::Char(']')) => 0,
                        (None, _) => len - 1,
                        (Some(i), KeyCode::Char(']')) => (i + 1) % len,
                        (Some(i), _) => (i + len - 1) % len,
                    };
                    self.focus = Focus::Files;
                    self.jump_to_file(next);
                }
                Action::None
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let order = [Focus::Summary, Focus::Files, Focus::Artifacts];
                let i = order.iter().position(|f| *f == self.focus).unwrap_or(1);
                let next = if key.code == KeyCode::Tab {
                    i + 1
                } else {
                    i + order.len() - 1
                };
                self.focus = order[next % order.len()];
                Action::None
            }
            KeyCode::Enter => match self.info.artifacts.get(self.art_sel) {
                Some(a) => {
                    self.mode = Mode::Preview(PreviewPopup::open(ctx, a));
                    Action::None
                }
                None => Action::Toast(Toast::new(
                    Tone::Info,
                    "No artifacts",
                    "this run has no artifacts",
                )),
            },
            _ => {
                match self.focus {
                    Focus::Summary => {
                        self.summary_scroll.on_key(key, 10);
                    }
                    Focus::Files => {
                        self.files_scroll.on_key(key, 20);
                    }
                    Focus::Artifacts => {
                        let arts = &self.info.artifacts;
                        if let Some(toast) = self.list.on_key(ctx, arts, &mut self.art_sel, key) {
                            return toast.map_or(Action::None, Action::Toast);
                        }
                    }
                }
                Action::None
            }
        }
    }
}

impl ReviewView {
    fn on_retry_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        let Mode::Retry(c) = &mut self.mode else {
            return Action::None;
        };
        if let Some(action) = c.on_key(key, ctx) {
            return action;
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                Action::None
            }
            KeyCode::Enter => {
                let (choice, name) = (c.choice(), c.display_name());
                self.retry(ctx, &choice, &name)
            }
            _ => Action::None,
        }
    }

    fn retry(&mut self, ctx: &Ctx, choice: &ProfileChoice, name: &str) -> Action {
        let id = self.id();
        match corral_ops::retry(ctx, &id, choice) {
            Ok(DispatchOutcome::Started { run_id, task, .. }) => {
                let label = task
                    .current_run()
                    .map(choice::run_label)
                    .unwrap_or_default();
                Action::BackWithToast(Toast::new(
                    Tone::Info,
                    format!("↻ {id} retry"),
                    format!(
                        "new run {run_id} with {name} · {label} in a fresh worktree; {} kept for comparison",
                        self.info.run.id
                    ),
                ))
            }
            Ok(DispatchOutcome::Queued { reason, .. }) => Action::BackWithToast(Toast::new(
                Tone::Warn,
                format!("⏳ {id} retry queued"),
                reason,
            )),
            Err(e) => {
                self.mode = Mode::Normal;
                Action::Toast(Toast::new(
                    Tone::Bad,
                    format!("retry {id} failed"),
                    corral_herdr::friendly_error(&e),
                ))
            }
        }
    }
}

/// The retry sheet popup: what happens to the runs, the choice rows, and the previous run for comparison.
fn draw_retry(f: &mut Frame, area: Rect, info: &ReviewInfo, c: &mut RunChoice, hits: &mut HitMap) {
    let r = &info.run;
    let rect = overlays::centered(area, 84, choice::ROWS + 5);
    let inner = overlays::popup(f, rect, &format!("retry {}", info.task.id), hits);
    let w = usize::from(inner.width);
    let next = info.task.runs.len() + 1;
    let lines = vec![
        Line::from(Span::styled(
            overlays::truncate(
                &format!(
                    "new run R{next} in a fresh worktree from {}; {} is superseded and kept",
                    r.base, r.id
                ),
                w,
            ),
            theme::dim(),
        )),
        Line::default(),
        Line::default(),
        Line::default(),
        Line::default(),
        Line::from(vec![
            Span::styled(format!("{:<10}", "previous"), theme::dim()),
            Span::raw(format!("{} · {}", r.id, choice::run_title(r))),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), inner);
    let rows = Rect {
        y: inner.y + 1,
        height: choice::ROWS.min(inner.height.saturating_sub(1)),
        ..inner
    };
    c.draw(f, rows, 10, hits);
    c.draw_popups(f, area, hits);
}

fn outcome_label(o: Option<RunOutcome>) -> (&'static str, ratatui::style::Color) {
    match o {
        Some(RunOutcome::Done) => ("done", GREEN),
        Some(RunOutcome::Superseded) => ("superseded", DIM),
        Some(RunOutcome::Abandoned) => ("abandoned", DIM),
        Some(RunOutcome::Discarded) => ("discarded", RED),
        Some(RunOutcome::Interrupted) => ("interrupted", DIM),
        None => ("in progress", theme::BLUE),
    }
}

impl View for ReviewView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        ui::paint_bg(f, area);
        let changes = matches!(self.mode, Mode::Changes(_));
        let [head, actions, body, line] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(if changes { 3 } else { 0 }),
        ])
        .horizontal_margin(1)
        .areas(area);
        let mut header = vec![Span::styled(BACK, Style::new().fg(CYAN)), Span::raw("  ")];
        header.extend(self.header().spans);
        f.render_widget(Paragraph::new(Line::from(header)), head);
        let back = Rect {
            width: (BACK.chars().count() as u16).min(head.width),
            ..head
        };
        hits.push(back, Hit::Key(hit::press(KeyCode::Esc)));
        if matches!(self.mode, Mode::Normal) {
            let buttons = [
                ("Approve", hit::press(KeyCode::Char('a'))),
                ("Request changes", hit::press(KeyCode::Char('c'))),
                ("Retry", hit::press(KeyCode::Char('R'))),
                ("Discard", hit::press(KeyCode::Char('x'))),
                ("All artifacts", hit::press(KeyCode::Char('A'))),
            ];
            hit::buttons(f, hits, actions.x, actions.y, actions.right(), &buttons);
        }

        let [sum, files, arts] = Layout::horizontal([
            Constraint::Fill(19),
            Constraint::Fill(22),
            Constraint::Fill(22),
        ])
        .spacing(1)
        .areas(body);
        let inner = ui::boxed(f, sum, "Summary", self.focus == Focus::Summary, None);
        let lines = self.summary_lines(ctx);
        ui::scrolled(f, inner, lines, &mut self.summary_scroll, true);
        hits.push(sum, Hit::Item(Z_SUMMARY, 0));

        let title = format!("Files {}", self.info.stat.files.len());
        let inner = ui::boxed(f, files, &title, self.focus == Focus::Files, None);
        let lines = self.files_lines();
        ui::scrolled(f, inner, lines, &mut self.files_scroll, false);
        hits.push(files, Hit::Item(Z_FILES, 0));
        let n = self.info.stat.files.len();
        hit::rows(hits, inner, Z_FILE, 0, n, self.files_scroll.offset);

        let todo = self.list.unreviewed_keep(&self.info.artifacts);
        let title = if todo > 0 {
            format!("Artifacts {} · {todo} to review", self.info.artifacts.len())
        } else {
            format!("Artifacts {}", self.info.artifacts.len())
        };
        let inner = ui::boxed(f, arts, &title, self.focus == Focus::Artifacts, None);
        let lines = self.artifact_lines(usize::from(inner.width));
        let mut s = Scroll {
            offset: self
                .list
                .row_of(&self.info.artifacts, self.art_sel)
                .saturating_sub(usize::from(inner.height.saturating_sub(3))),
            ..Scroll::default()
        };
        ui::scrolled(f, inner, lines, &mut s, false);
        hits.push(arts, Hit::Item(Z_ARTS, 0));
        let n = self.list.rows(&self.info.artifacts).len();
        hit::rows(hits, inner, Z_ART, 0, n, s.offset);

        match &mut self.mode {
            Mode::Changes(text) => {
                let inner = ui::boxed(f, line, "Request changes → same session", true, None);
                let labels = ["Send", "Cancel"];
                let bw = hit::buttons_width(&labels);
                let field = Rect {
                    width: inner.width.saturating_sub(bw + 1),
                    ..inner
                };
                let mut spans = vec![Span::styled("changes › ", Style::new().fg(YELLOW))];
                spans.extend(ui::input_spans(text, "what should the agent change?"));
                f.render_widget(Paragraph::new(Line::from(spans)), field);
                hits.push(line, Hit::Inert);
                let keys = [hit::press(KeyCode::Enter), hit::press(KeyCode::Esc)];
                let buttons: Vec<_> = labels.into_iter().zip(keys).collect();
                let x = inner.right().saturating_sub(bw);
                hit::buttons(f, hits, x, inner.y, inner.right(), &buttons);
            }
            Mode::Preview(p) => p.draw(f, area, hits),
            Mode::ConfirmDiscard => self.draw_confirm(f, area, hits),
            Mode::Retry(c) => draw_retry(f, area, &self.info, c, hits),
            Mode::Merge(_) | Mode::Normal => {}
        }
        if let Mode::Merge(sheet) = &self.mode {
            self.draw_merge(f, area, sheet, ctx, hits);
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        match &mut self.mode {
            Mode::Normal => self.on_normal_key(&key, ctx),
            Mode::Merge(_) => self.on_merge_key(&key, ctx),
            Mode::Changes(_) => self.on_changes_key(&key, ctx),
            Mode::ConfirmDiscard => self.on_confirm_key(&key, ctx),
            Mode::Retry(_) => self.on_retry_key(&key, ctx),
            Mode::Preview(p) => {
                if key.code == KeyCode::Esc {
                    self.mode = Mode::Normal;
                    return Action::None;
                }
                p.on_key(&key, ctx).map_or(Action::None, Action::Toast)
            }
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match &self.mode {
            Mode::Merge(_) => vec![
                ("j/k", "strategy"),
                ("w", "toggle worktree removal"),
                ("⏎", "confirm"),
                ("esc", "cancel"),
            ],
            Mode::Changes(_) => vec![("⏎", "send"), ("esc", "cancel")],
            Mode::ConfirmDiscard => vec![("⏎", "discard"), ("esc", "cancel")],
            Mode::Retry(c) => {
                let mut h = c.hints();
                if !c.modal() {
                    h.extend([("⏎", "retry"), ("esc", "cancel")]);
                }
                h
            }
            Mode::Preview(_) => vec![("j/k", "scroll"), ("⏎", "open"), ("esc", "close")],
            Mode::Normal => {
                let mut h = vec![
                    ("a", "approve"),
                    ("c", "request changes"),
                    ("R", "retry"),
                    ("x", "discard"),
                    ("i", "acceptance"),
                    (
                        "*",
                        if self.pins.contains(&self.pin_target()) {
                            "unpin"
                        } else {
                            "pin"
                        },
                    ),
                    ("⇥", "focus"),
                    ("j/k", "scroll"),
                    ("[/]", "file"),
                    ("A", "artifacts"),
                    ("⏎", "open artifact"),
                    ("esc", "back"),
                ];
                if self.focus == Focus::Artifacts {
                    h.extend([
                        ("n", "next important"),
                        ("v", "reviewed"),
                        ("f", "raw fold"),
                    ]);
                }
                h
            }
        }
    }

    fn refresh(&mut self, ctx: &Ctx) {
        self.pins = shelf::pinned_refs(ctx);
        if let Ok(info) = corral_ops::review_info(ctx, &self.info.task.id) {
            self.info = info;
            self.list.sync(ctx, &self.info.artifacts);
            self.list.clamp(&self.info.artifacts, &mut self.art_sel);
        }
    }

    fn captures_text(&self) -> bool {
        matches!(self.mode, Mode::Changes(_) | Mode::Retry(_))
    }

    fn has_unsaved_text(&self) -> bool {
        match &self.mode {
            Mode::Retry(c) => c.has_unsaved_text(),
            _ => false,
        }
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.mouse(ev, m)
    }
}
