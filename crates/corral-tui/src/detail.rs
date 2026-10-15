use std::collections::HashSet;

use anyhow::Result;
use corral_core::{Event, RunOutcome, Status, StuckProcess, Task};
use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use ratatui::Frame;

use crate::artifacts::ui::{self, PreviewPopup, Scroll};
use crate::choice::run_label;
use crate::hit::{self, Hit};
use crate::shelf;
use crate::theme::{self, BLUE, CYAN, DIM, FG, GREEN, RED, SELECTED_BG, YELLOW};
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View};

mod acceptance;
mod mouse;
#[cfg(test)]
mod obligations_tests;
#[cfg(test)]
mod panels_tests;
mod run_state;
mod unstick;

/// Mouse zones ([`Hit::Item`]): the tab content box, the live tail, run rows, artifact rows and acceptance
/// record rows.
const Z_CONTENT: u8 = 0;
const Z_LIVE: u8 = 1;
const Z_RUN: u8 = 2;
const Z_ART: u8 = 3;
const Z_ACC: u8 = 4;
const BACK: &str = "‹ back";

const TABS: [&str; 5] = ["Spec", "Runs", "Artifacts", "Diff", "Acceptance"];
const ACCEPTANCE: usize = 4;
const LIVE_LINES: usize = 200;

/// Card detail: header (id, title, project, status, priority, agent); left box "Task" with tabs
/// 1 Spec (task body), 2 Runs (table of runs), 3 Artifacts (list; ⏎ opens preview), 4 Diff (current run diff via
/// review_info when a run exists), 5 Acceptance (review policy and its sources, pipeline stage, gate and review
/// results, records; ⏎ previews a record); right box "Live" with `live_tail` (refreshed on `refresh`).
/// Keys: 1-5 / Tab switch tabs, j/k scroll (select on Runs, Artifacts and Acceptance), a -> Action::Jump, Esc -> Back,
/// u -> confirm popup (pid + argv) that calls `corral_ops::unstick` for the current run's stuck process,
/// * pins the selected artifact (Artifacts tab) or the task.
pub struct DetailView {
    task: Task,
    tab: usize,
    scroll: Scroll,
    live: Result<Vec<String>, String>,
    /// Lines the live tail is scrolled up from its newest line.
    live_scroll: usize,
    /// Selected run on the Runs tab; None follows the current (last) run.
    run_sel: Option<usize>,
    /// Loaded on first visit of the Diff tab; reset on refresh.
    diff: Option<Result<(String, String), String>>,
    art_sel: usize,
    preview: Option<PreviewPopup>,
    /// Open unstick confirmation: the stuck process as shown when `u` was pressed; ⏎ passes exactly this one.
    confirm: Option<StuckProcess>,
    /// Selected acceptance record (index into `acceptance::records`).
    acc_sel: usize,
    /// The task's review-pipeline events; loaded on visiting the Acceptance tab, reset on refresh.
    history: Option<Result<Vec<Event>, String>>,
    /// Pin refs, for the ★ markers.
    pins: HashSet<String>,
}

impl DetailView {
    pub fn new(ctx: &Ctx, task_id: &str) -> Result<DetailView> {
        corral_ops::refresh_usage(&ctx.store, task_id)?;
        let task = ctx.store.get(task_id)?;
        let live = load_live(ctx, &task);
        Ok(DetailView {
            task,
            tab: 0,
            scroll: Scroll::default(),
            live,
            live_scroll: 0,
            run_sel: None,
            diff: None,
            art_sel: 0,
            preview: None,
            confirm: None,
            acc_sel: 0,
            history: None,
            pins: shelf::pinned_refs(ctx),
        })
    }

    /// The detail view opened on its Acceptance tab (from the review screen).
    pub fn acceptance(ctx: &Ctx, task_id: &str) -> Result<DetailView> {
        let mut v = DetailView::new(ctx, task_id)?;
        v.set_tab(ACCEPTANCE, ctx);
        Ok(v)
    }

    /// The current run's stuck process, while that run is still open.
    fn stuck(&self) -> Option<&StuckProcess> {
        self.task
            .current_run()
            .filter(|r| r.outcome.is_none())
            .and_then(|r| r.stuck.as_ref())
    }

    /// The ref `*` toggles: the selected artifact on the Artifacts tab, else the task.
    fn pin_target(&self) -> String {
        match self.task.artifacts.get(self.art_sel) {
            Some(a) if self.tab == 2 => shelf::artifact_ref(&self.task.id, a),
            _ => self.task.id.clone(),
        }
    }

    fn run_index(&self) -> usize {
        let last = self.task.runs.len().saturating_sub(1);
        self.run_sel.unwrap_or(last).min(last)
    }

    fn set_tab(&mut self, tab: usize, ctx: &Ctx) {
        self.tab = tab % TABS.len();
        self.scroll = Scroll::default();
        if self.tab == ACCEPTANCE && self.history.is_none() {
            self.history = Some(
                ctx.store
                    .events()
                    .map(|e| acceptance::history(e, &self.task.id))
                    .map_err(|e| format!("{e:#}")),
            );
        }
        if self.tab == 3 && self.diff.is_none() && self.task.current_run().is_some() {
            self.diff = Some(match corral_ops::review_info(ctx, &self.task.id) {
                Ok(info) => Ok((
                    format!(
                        "{} files  +{} −{}",
                        info.stat.files.len(),
                        info.stat.added,
                        info.stat.removed
                    ),
                    info.diff,
                )),
                Err(e) => Err(format!("{e:#}")),
            });
        }
    }

    fn header(&self) -> Line<'static> {
        let t = &self.task;
        let mut spans = vec![
            Span::styled(t.id.clone(), theme::bold()),
            Span::raw(" "),
            match self.pins.contains(&t.id) {
                true => shelf::star(true),
                false => Span::raw(" "),
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
            format!("{} · {}", t.status.label(), t.priority),
            theme::dim(),
        ));
        if let Some(a) = t
            .current_run()
            .map(|r| r.agent.clone())
            .or_else(|| t.profile.clone())
        {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                a.clone(),
                Style::new().fg(theme::agent_color(&a)),
            ));
        }
        if t.queued {
            spans.push(Span::styled("  ⏳ queued", Style::new().fg(YELLOW)));
        }
        spans.extend(
            t.current_run()
                .filter(|r| r.outcome.is_none())
                .and_then(run_state::badge),
        );
        if let Some(s) = self.stuck() {
            spans.push(Span::styled(
                format!("  ⚠ stuck: {}", s.argv.join(" ")),
                Style::new().fg(RED),
            ));
        }
        Line::from(spans)
    }

    fn tabs_line(&self) -> Line<'static> {
        let mut spans = Vec::new();
        for (i, name) in TABS.iter().enumerate() {
            let style = if i == self.tab {
                Style::new()
                    .fg(CYAN)
                    .add_modifier(Modifier::UNDERLINED | Modifier::BOLD)
            } else {
                Style::new().fg(DIM)
            };
            spans.push(Span::styled(format!(" {} {} ", i + 1, name), style));
            spans.push(Span::raw(" "));
        }
        Line::from(spans)
    }

    fn spec_lines(&self) -> Vec<Line<'static>> {
        let t = &self.task;
        let mut lines = if t.body.trim().is_empty() {
            vec![Line::from(Span::styled("(empty spec)", theme::dim()))]
        } else {
            ui::text_lines(&t.body)
        };
        let deps = if t.depends_on.is_empty() {
            "—".to_string()
        } else {
            t.depends_on.join(", ")
        };
        let file = t
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!(
                "depends_on: {deps} · priority {} · file .corral/tasks/{file}",
                t.priority
            ),
            theme::dim(),
        )));
        lines
    }

    fn runs_lines(&self, ctx: &Ctx, width: usize) -> Vec<Line<'static>> {
        let t = &self.task;
        if t.runs.is_empty() {
            return vec![Line::from(Span::styled(
                "No runs yet. Press d on the board to dispatch.",
                theme::dim(),
            ))];
        }
        let cols = [4, 10, 20, 12, 8, 16, 11, 6];
        let head = [
            "run",
            "profile",
            "harness·model·effort",
            "mode",
            "pane",
            "branch",
            "outcome",
            "time",
        ];
        let mut lines = vec![Line::from(
            head.iter()
                .zip(cols)
                .map(|(h, w)| Span::styled(ui::fit(h, w), theme::dim()))
                .collect::<Vec<_>>(),
        )];
        let sel = self.run_index();
        for (i, r) in t.runs.iter().enumerate() {
            let (outcome, color) = match r.outcome {
                Some(RunOutcome::Done) => ("done", GREEN),
                Some(RunOutcome::Superseded) => ("superseded", DIM),
                Some(RunOutcome::Abandoned) => ("abandoned", DIM),
                Some(RunOutcome::Discarded) => ("discarded", RED),
                None if t.status == Status::NeedsMe => ("blocked", YELLOW),
                None => ("running", BLUE),
            };
            let mode = format!("{:?}", r.mode).to_lowercase();
            let pane = r
                .herdr
                .as_ref()
                .map(|h| h.pane_id.clone())
                .unwrap_or_else(|| "—".into());
            let mut spans = vec![
                Span::styled(ui::fit(&r.id, cols[0]), Style::new().fg(FG)),
                Span::styled(
                    ui::fit(&r.agent, cols[1]),
                    Style::new().fg(theme::agent_color(&r.agent)),
                ),
                Span::styled(ui::fit(&run_label(r), cols[2]), Style::new().fg(FG)),
                Span::styled(ui::fit(&mode, cols[3]), Style::new().fg(FG)),
                Span::styled(ui::fit(&pane, cols[4]), Style::new().fg(FG)),
                Span::styled(ui::fit(&r.branch, cols[5]), Style::new().fg(FG)),
                Span::styled(ui::fit(outcome, cols[6]), Style::new().fg(color)),
                Span::styled(ui::elapsed(r.started, r.ended), theme::dim()),
            ];
            if i == sel {
                spans
                    .iter_mut()
                    .for_each(|s| s.style = s.style.bg(SELECTED_BG));
            }
            lines.push(Line::from(spans));
            let usage = cost_summary(r.total_usage());
            lines.push(Line::from(Span::styled(
                format!("  cost: {usage}"),
                theme::dim(),
            )));
        }
        let usage = cost_summary(t.total_usage());
        lines.push(Line::from(Span::styled(
            format!("Total: {usage}"),
            theme::dim(),
        )));
        if let Some(r) = t.runs.get(sel) {
            let sha: String = r.base_sha.chars().take(7).collect();
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                format!(
                    "worktree {}{} · base {}@{sha}",
                    ui::rel_path(ctx, &r.worktree),
                    if r.worktree.exists() {
                        ""
                    } else {
                        " (removed)"
                    },
                    r.base
                ),
                theme::dim(),
            )));
            lines.extend(run_state::lines(r, width));
            if let Some(a) = &r.autopilot {
                let round = if t.rounds > 0 {
                    format!(" · round {}", t.rounds)
                } else {
                    String::new()
                };
                let reason = a
                    .reason
                    .as_deref()
                    .and_then(|x| x.lines().find(|l| !l.trim().is_empty()))
                    .map(|x| format!(": {x}"))
                    .unwrap_or_default();
                lines.push(Line::from(Span::styled(
                    format!("autopilot {}{round}{reason}", a.stage.as_str()),
                    Style::new().fg(BLUE),
                )));
            }
            if let Some(s) = &r.summary {
                lines.push(Line::default());
                lines.push(Line::from(Span::styled("summary", theme::bold())));
                lines.extend(ui::text_lines(s));
            }
        }
        lines
    }

    fn artifact_lines(&self) -> Vec<Line<'static>> {
        if self.task.artifacts.is_empty() {
            return vec![Line::from(Span::styled(
                "No artifacts yet. They appear on corral artifact add and at corral done.",
                theme::dim(),
            ))];
        }
        let mut lines: Vec<Line<'static>> = self
            .task
            .artifacts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let pinned = self.pins.contains(&shelf::artifact_ref(&self.task.id, a));
                ui::artifact_line(a, i == self.art_sel, None, pinned)
            })
            .collect();
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("⏎ preview selected", theme::dim())));
        lines
    }

    fn diff_lines(&self) -> Vec<Line<'static>> {
        match &self.diff {
            None => vec![Line::from(Span::styled(
                "No run yet, so there is no diff.",
                theme::dim(),
            ))],
            Some(Err(e)) => vec![Line::from(Span::styled(
                format!("diff unavailable: {e}"),
                Style::new().fg(RED),
            ))],
            Some(Ok((_, d))) if d.trim().is_empty() => {
                vec![Line::from(Span::styled(
                    "No changes relative to the run base.",
                    theme::dim(),
                ))]
            }
            Some(Ok((stat, d))) => {
                let mut lines = vec![
                    Line::from(Span::styled(stat.clone(), theme::dim())),
                    Line::default(),
                ];
                lines.extend(ui::diff_lines(d));
                lines
            }
        }
    }

    fn live_lines(&self, height: usize, width: usize) -> Vec<Line<'static>> {
        match &self.live {
            Err(e) => vec![Line::from(Span::styled(
                format!("live tail unavailable: {e}"),
                Style::new().fg(RED),
            ))],
            Ok(lines) if !lines.is_empty() => {
                let end = lines.len() - self.live_scroll.min(lines.len().saturating_sub(height));
                let start = end.saturating_sub(height);
                let waiting = self.task.status == Status::NeedsMe;
                lines[start..end]
                    .iter()
                    .map(|l| {
                        let style = if waiting && l.trim_start().starts_with('?') {
                            Style::new().fg(YELLOW)
                        } else {
                            Style::new().fg(FG)
                        };
                        Line::from(Span::styled(l.replace('\t', "    "), style))
                    })
                    .collect()
            }
            Ok(_) => {
                let open = self.task.current_run().filter(|r| r.outcome.is_none());
                if let Some(d) = open.and_then(|r| run_state::decision_fallback(r, width)) {
                    return d;
                }
                let excerpt = self
                    .task
                    .current_run()
                    .and_then(|r| r.blocked_excerpt.clone());
                if let (Status::NeedsMe, Some(x)) = (self.task.status, excerpt) {
                    let mut out = vec![Line::from(Span::styled(
                        "blocked excerpt (pane not live):",
                        theme::dim(),
                    ))];
                    out.extend(
                        x.lines().map(|l| {
                            Line::from(Span::styled(l.to_string(), Style::new().fg(YELLOW)))
                        }),
                    );
                    return out;
                }
                let mut msg = "No live run.".to_string();
                if self.task.status == Status::Review {
                    msg.push_str(" Last run finished; see Review.");
                }
                vec![Line::from(Span::styled(msg, theme::dim()))]
            }
        }
    }
}

fn load_live(ctx: &Ctx, task: &Task) -> Result<Vec<String>, String> {
    let has_pane = task
        .current_run()
        .is_some_and(|r| r.herdr.is_some() && r.outcome.is_none());
    if !has_pane {
        return Ok(Vec::new());
    }
    corral_ops::live_tail(ctx, &task.id, LIVE_LINES).map_err(|e| corral_herdr::friendly_error(&e))
}

impl View for DetailView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        ui::paint_bg(f, area);
        let [head, _, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .horizontal_margin(1)
        .areas(area);
        let mut header = vec![Span::styled(BACK, Style::new().fg(CYAN)), Span::raw("  ")];
        header.extend(self.header().spans);
        f.render_widget(Paragraph::new(Line::from(header)), head);
        let back_w = (BACK.chars().count() as u16).min(head.width);
        hits.push(
            Rect {
                width: back_w,
                ..head
            },
            Hit::Key(hit::press(KeyCode::Esc)),
        );

        let [left, right] = Layout::horizontal([Constraint::Fill(5), Constraint::Fill(4)])
            .spacing(1)
            .areas(body);
        let inner = ui::boxed(f, left, "Task", true, None);
        let [tabs, _, content] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);
        f.render_widget(Paragraph::new(self.tabs_line()), tabs);
        hits.push(left, Hit::Item(Z_CONTENT, 0));
        tab_hits(hits, tabs);
        let mut acc_first = 0;
        let (lines, wrap) = match self.tab {
            0 => (self.spec_lines(), true),
            1 => (self.runs_lines(ctx, usize::from(content.width)), false),
            2 => (self.artifact_lines(), false),
            ACCEPTANCE => {
                let history = self
                    .history
                    .as_ref()
                    .map(|h| h.as_deref().map_err(String::as_str));
                let (lines, first) =
                    acceptance::lines(&self.task, &ctx.store.config, history, self.acc_sel);
                acc_first = first;
                (lines, false)
            }
            _ => (self.diff_lines(), false),
        };
        ui::scrolled(f, content, lines, &mut self.scroll, wrap);
        let offset = self.scroll.offset;
        match self.tab {
            1 => {
                for i in 0..self.task.runs.len() {
                    for row in [1 + i * 2, 2 + i * 2] {
                        if let Some(dy) = row.checked_sub(offset)
                            && dy < usize::from(content.height)
                        {
                            hits.push(
                                Rect::new(content.x, content.y + dy as u16, content.width, 1),
                                Hit::Item(Z_RUN, i),
                            );
                        }
                    }
                }
            }
            2 => hit::rows(hits, content, Z_ART, 0, self.task.artifacts.len(), offset),
            ACCEPTANCE => {
                let n = acceptance::records(&self.task).len();
                hit::rows(hits, content, Z_ACC, acc_first, n, offset);
            }
            _ => {}
        }

        let live_now = matches!(&self.live, Ok(l) if !l.is_empty());
        let right_title = self.task.current_run().filter(|_| live_now).map(|r| {
            let pane = r.herdr.as_ref().map(|h| h.pane_id.as_str()).unwrap_or("");
            format!("{} · {pane}", r.agent)
        });
        let live_inner = ui::boxed(f, right, "Live", false, right_title);
        if let Ok(l) = &self.live {
            let max = l.len().saturating_sub(usize::from(live_inner.height));
            self.live_scroll = self.live_scroll.min(max);
        }
        let lines = self.live_lines(
            usize::from(live_inner.height),
            usize::from(live_inner.width),
        );
        f.render_widget(Paragraph::new(lines), live_inner);
        hits.push(right, Hit::Item(Z_LIVE, 0));

        if let Some(p) = &mut self.preview {
            p.draw(f, area, hits);
        }
        if let Some(s) = &self.confirm {
            self.draw_unstick(f, area, s, hits);
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        if let Some(p) = &mut self.preview {
            if key.code == KeyCode::Esc {
                self.preview = None;
                return Action::None;
            }
            return p.on_key(&key, ctx).map_or(Action::None, Action::Toast);
        }
        if self.confirm.is_some() {
            return self.on_unstick_key(&key, ctx);
        }
        match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Char(c @ '1'..='5') => {
                self.set_tab(usize::from(c as u8 - b'1'), ctx);
                Action::None
            }
            KeyCode::Tab => {
                self.set_tab(self.tab + 1, ctx);
                Action::None
            }
            KeyCode::BackTab => {
                self.set_tab(self.tab + TABS.len() - 1, ctx);
                Action::None
            }
            KeyCode::Char('a') => Action::Jump(self.task.id.clone()),
            KeyCode::Char('u') => match self.stuck() {
                Some(s) => {
                    self.confirm = Some(s.clone());
                    Action::None
                }
                None => Action::Toast(Toast::new(
                    Tone::Info,
                    "Nothing to unstick",
                    "the current run has no stuck process",
                )),
            },
            KeyCode::Char('*') => shelf::toggle(ctx, &self.pin_target()),
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let dir = if key.code == KeyCode::Char('[') {
                    -1
                } else {
                    1
                };
                self.scroll_live(dir);
                Action::None
            }
            KeyCode::Enter if self.tab == 2 => match self.task.artifacts.get(self.art_sel) {
                Some(a) => {
                    self.preview = Some(PreviewPopup::open(ctx, a));
                    Action::None
                }
                None => Action::Toast(Toast::new(
                    Tone::Info,
                    "No artifacts",
                    "this task has no artifacts yet",
                )),
            },
            KeyCode::Enter if self.tab == ACCEPTANCE => {
                let records = acceptance::records(&self.task);
                match records
                    .get(self.acc_sel)
                    .and_then(|i| self.task.artifacts.get(*i))
                {
                    Some(a) => {
                        self.preview = Some(PreviewPopup::open(ctx, a));
                        Action::None
                    }
                    None => Action::Toast(Toast::new(
                        Tone::Info,
                        "No acceptance records",
                        "gate logs and reviews appear once the autopilot runs",
                    )),
                }
            }
            KeyCode::Char('j') | KeyCode::Down if self.tab == ACCEPTANCE => {
                let last = acceptance::records(&self.task).len().saturating_sub(1);
                self.acc_sel = (self.acc_sel + 1).min(last);
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up if self.tab == ACCEPTANCE => {
                self.acc_sel = self.acc_sel.saturating_sub(1);
                Action::None
            }
            KeyCode::Char('j') | KeyCode::Down if self.tab == 2 => {
                self.art_sel = (self.art_sel + 1).min(self.task.artifacts.len().saturating_sub(1));
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up if self.tab == 2 => {
                self.art_sel = self.art_sel.saturating_sub(1);
                Action::None
            }
            KeyCode::Char('j') | KeyCode::Down if self.tab == 1 => {
                let last = self.task.runs.len().saturating_sub(1);
                self.run_sel = Some((self.run_index() + 1).min(last));
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up if self.tab == 1 => {
                self.run_sel = Some(self.run_index().saturating_sub(1));
                Action::None
            }
            _ => {
                self.scroll.on_key(&key, 10);
                Action::None
            }
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.preview.is_some() {
            return vec![
                ("j/k", "scroll"),
                ("⏎", "open externally"),
                ("esc", "close"),
            ];
        }
        if self.confirm.is_some() {
            return vec![("⏎", "unstick"), ("esc", "cancel")];
        }
        let mut h = vec![("1-5", "tabs")];
        match self.tab {
            1 => h.extend([("j/k", "run"), ("pgup/pgdn", "scroll")]),
            2 => h.extend([("j/k", "select"), ("⏎", "preview")]),
            ACCEPTANCE => h.extend([("j/k", "record"), ("⏎", "preview"), ("pgup/pgdn", "scroll")]),
            _ => h.push(("j/k", "scroll")),
        }
        if self.stuck().is_some() {
            h.push(("u", "unstick"));
        }
        let pinned = self.pins.contains(&self.pin_target());
        h.extend([
            ("[/]", "live tail"),
            ("*", if pinned { "unpin" } else { "pin" }),
            ("a", "jump"),
            ("esc", "back"),
        ]);
        h
    }

    fn refresh(&mut self, ctx: &Ctx) {
        if let Err(error) = corral_ops::refresh_usage(&ctx.store, &self.task.id) {
            corral_ops::report_message(
                Some(&self.task.id),
                "usage",
                format!("refreshing usage for {}: {error:#}", self.task.id),
            );
        }
        if let Ok(t) = ctx.store.get(&self.task.id) {
            self.task = t;
        }
        self.art_sel = self
            .art_sel
            .min(self.task.artifacts.len().saturating_sub(1));
        let records = acceptance::records(&self.task).len();
        self.acc_sel = self.acc_sel.min(records.saturating_sub(1));
        self.live = load_live(ctx, &self.task);
        self.pins = shelf::pinned_refs(ctx);
        self.diff = None;
        self.history = None;
        if self.tab == ACCEPTANCE {
            self.set_tab(ACCEPTANCE, ctx);
        }
        if self.tab == 3 {
            self.set_tab(3, ctx);
        }
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.mouse(ev, m)
    }
}

/// Each tab label in the strip drawn by `tabs_line` presses its number key.
fn tab_hits(hits: &mut HitMap, strip: Rect) {
    let mut x = strip.x;
    for (i, name) in TABS.iter().enumerate() {
        let w = format!(" {} {name} ", i + 1).chars().count() as u16;
        if x >= strip.right() {
            break;
        }
        let key = hit::press(KeyCode::Char(char::from(b'1' + i as u8)));
        hits.push(
            Rect::new(x, strip.y, w.min(strip.right() - x), 1),
            Hit::Key(key),
        );
        x = x.saturating_add(w + 1);
    }
}

fn cost_summary(usage: Option<corral_core::Usage>) -> String {
    let Some(usage) = usage else {
        return "unknown".into();
    };
    let dollars = |cost: Option<f64>| {
        cost.map(|value| {
            if value > 0.0 && value < 0.01 {
                format!("${value:.4}")
            } else {
                format!("${value:.2}")
            }
        })
        .unwrap_or_else(|| "-".into())
    };
    let basis = match usage.cost_basis {
        Some(corral_core::CostBasis::Reported) => "reported",
        Some(corral_core::CostBasis::Estimated) => "estimated",
        Some(corral_core::CostBasis::Mixed) => "mixed",
        None => "unpriced",
    };
    format!(
        "reported {}; est. {}; {}t; {basis}; {} unpriced",
        dollars(usage.reported_usd.or(usage.cost_usd)),
        dollars(usage.estimated_usd),
        usage.total_tokens(),
        usage.unpriced_tokens
    )
}
