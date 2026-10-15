use anyhow::Result;
use corral_core::RunOutcome;
use corral_ops::{ApproveStrategy, Ctx, DispatchOutcome, ReviewInfo};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::artifacts::ui::{self, PreviewPopup, Scroll};
use crate::theme::{self, CYAN, DIM, FG, GREEN, RED, SELECTED_BG, YELLOW};
use crate::{Action, HitMap, Toast, Tone, View};

#[cfg(test)]
mod panels_tests;

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
}

/// Review screen (task in Review): three boxes Summary (run summary, outcome, runs history), Files (diff stat
/// list + scrollable diff), Artifacts (list; ⏎ preview). Keys: a -> merge sheet overlay inside this view
/// (strategies: fast-forward / rebase+ff / keep branch; w toggles remove worktree; shows main_ready preflight;
/// ⏎ calls corral_ops::approve -> BackWithToast), c -> one-line "changes ›" input (⏎ request_changes ->
/// BackWithToast), R -> retry (same agent) -> BackWithToast, x -> discard -> BackWithToast, Esc -> Back.
pub struct ReviewView {
    info: ReviewInfo,
    focus: Focus,
    summary_scroll: Scroll,
    files_scroll: Scroll,
    art_sel: usize,
    mode: Mode,
}

impl ReviewView {
    pub fn new(ctx: &Ctx, task_id: &str) -> Result<ReviewView> {
        let info = corral_ops::review_info(ctx, task_id)?;
        Ok(ReviewView {
            info,
            focus: Focus::Files,
            summary_scroll: Scroll::default(),
            files_scroll: Scroll::default(),
            art_sel: 0,
            mode: Mode::Normal,
        })
    }

    fn id(&self) -> String {
        self.info.task.id.clone()
    }

    fn header(&self) -> Line<'static> {
        let t = &self.info.task;
        let r = &self.info.run;
        let mut spans = vec![
            Span::styled(format!("Review {}", t.id), theme::bold()),
            Span::raw("  "),
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
                r.agent,
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
            .map(|run| format!("{} {} {}", run.id, run.agent, outcome_label(run.outcome).0))
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

    fn artifact_lines(&self) -> Vec<Line<'static>> {
        if self.info.artifacts.is_empty() {
            return vec![Line::from(Span::styled(
                "No artifacts for this run.",
                theme::dim(),
            ))];
        }
        let mut lines: Vec<Line<'static>> = self
            .info
            .artifacts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                ui::artifact_line(a, i == self.art_sel && self.focus == Focus::Artifacts, None)
            })
            .collect();
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "⏎ open selected · A all artifacts",
            theme::dim(),
        )));
        lines
    }

    fn draw_merge(&self, f: &mut Frame, area: Rect, sheet: &MergeSheet, ctx: &Ctx) {
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
        let rect = ui::centered(area, 86, lines.len() as u16 + 2);
        let inner = ui::popup(f, rect, &format!("approve {}", t.id), None);
        f.render_widget(Paragraph::new(lines).style(Style::new().fg(FG)), inner);
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
                return match corral_ops::approve(ctx, &id, strategy, remove) {
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

    fn on_normal_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        let id = self.id();
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
                match corral_ops::retry(ctx, &id, &corral_core::ProfileChoice::default()) {
                    Ok(DispatchOutcome::Started { run_id, task, .. }) => {
                        let agent = task
                            .current_run()
                            .map(|r| r.agent.clone())
                            .unwrap_or_default();
                        Action::BackWithToast(Toast::new(
                            Tone::Info,
                            format!("↻ {id} retry"),
                            format!(
                                "new run {run_id} with {agent} in a fresh worktree; {} kept for comparison",
                                self.info.run.id
                            ),
                        ))
                    }
                    Ok(DispatchOutcome::Queued { reason, .. }) => Action::BackWithToast(
                        Toast::new(Tone::Warn, format!("⏳ {id} retry queued"), reason),
                    ),
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        format!("retry {id} failed"),
                        format!("{e:#}"),
                    )),
                }
            }
            KeyCode::Char('x') => match corral_ops::discard(ctx, &id) {
                Ok(_) => Action::BackWithToast(Toast::new(
                    Tone::Warn,
                    format!("✗ {id} discarded"),
                    "branch, worktree and artifacts kept; card moved to dropped",
                )),
                Err(e) => Action::Toast(Toast::new(
                    Tone::Bad,
                    format!("discard {id} failed"),
                    format!("{e:#}"),
                )),
            },
            KeyCode::Char('A') => Action::OpenArtifacts,
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
                    Focus::Artifacts => match key.code {
                        KeyCode::Char('j') | KeyCode::Down => {
                            self.art_sel =
                                (self.art_sel + 1).min(self.info.artifacts.len().saturating_sub(1));
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            self.art_sel = self.art_sel.saturating_sub(1)
                        }
                        _ => {}
                    },
                }
                Action::None
            }
        }
    }
}

fn outcome_label(o: Option<RunOutcome>) -> (&'static str, ratatui::style::Color) {
    match o {
        Some(RunOutcome::Done) => ("done", GREEN),
        Some(RunOutcome::Superseded) => ("superseded", DIM),
        Some(RunOutcome::Abandoned) => ("abandoned", DIM),
        Some(RunOutcome::Discarded) => ("discarded", RED),
        None => ("in progress", theme::BLUE),
    }
}

impl View for ReviewView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, _hits: &mut HitMap) {
        ui::paint_bg(f, area);
        let changes = matches!(self.mode, Mode::Changes(_));
        let [head, _, body, line] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(if changes { 3 } else { 0 }),
        ])
        .horizontal_margin(1)
        .areas(area);
        f.render_widget(Paragraph::new(self.header()), head);

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

        let title = format!("Files {}", self.info.stat.files.len());
        let inner = ui::boxed(f, files, &title, self.focus == Focus::Files, None);
        let lines = self.files_lines();
        ui::scrolled(f, inner, lines, &mut self.files_scroll, false);

        let title = format!("Artifacts {}", self.info.artifacts.len());
        let inner = ui::boxed(f, arts, &title, self.focus == Focus::Artifacts, None);
        let lines = self.artifact_lines();
        let mut s = Scroll {
            offset: self
                .art_sel
                .saturating_sub(usize::from(inner.height.saturating_sub(3))),
        };
        ui::scrolled(f, inner, lines, &mut s, false);

        match &mut self.mode {
            Mode::Changes(text) => {
                let inner = ui::boxed(f, line, "Request changes → same session", true, None);
                let mut spans = vec![Span::styled("changes › ", Style::new().fg(YELLOW))];
                spans.extend(ui::input_spans(text, "what should the agent change?"));
                f.render_widget(Paragraph::new(Line::from(spans)), inner);
            }
            Mode::Preview(p) => p.draw(f, area),
            Mode::Merge(_) | Mode::Normal => {}
        }
        if let Mode::Merge(sheet) = &self.mode {
            self.draw_merge(f, area, sheet, ctx);
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
            Mode::Preview(p) => {
                if key.code == KeyCode::Esc {
                    self.mode = Mode::Normal;
                    return Action::None;
                }
                p.on_key(&key).map_or(Action::None, Action::Toast)
            }
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.mode {
            Mode::Merge(_) => vec![
                ("j/k", "strategy"),
                ("w", "toggle worktree removal"),
                ("⏎", "confirm"),
                ("esc", "cancel"),
            ],
            Mode::Changes(_) => vec![("⏎", "send"), ("esc", "cancel")],
            Mode::Preview(_) => vec![
                ("j/k", "scroll"),
                ("⏎", "open externally"),
                ("esc", "close"),
            ],
            Mode::Normal => vec![
                ("a", "approve"),
                ("c", "request changes"),
                ("R", "retry"),
                ("x", "discard"),
                ("⇥", "focus"),
                ("j/k", "scroll"),
                ("⏎", "open artifact"),
                ("esc", "back"),
            ],
        }
    }

    fn refresh(&mut self, ctx: &Ctx) {
        if let Ok(info) = corral_ops::review_info(ctx, &self.info.task.id) {
            self.info = info;
            self.art_sel = self
                .art_sel
                .min(self.info.artifacts.len().saturating_sub(1));
        }
    }

    fn captures_text(&self) -> bool {
        matches!(self.mode, Mode::Changes(_))
    }
}
