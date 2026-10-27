use std::collections::HashSet;

use anyhow::Result;
use corral_core::{Event, Status, StuckProcess, Task};
use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use ratatui::Frame;

use crate::artifacts::list::ArtList;
use crate::artifacts::ui::{self, PreviewPopup, Scroll};
use crate::hit::{self, Hit};
use crate::shelf;
use crate::theme::{self, FG, RED, YELLOW};
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View};

mod acceptance;
mod comments;
mod header;
mod hints;
mod lessons;
mod mouse;
#[cfg(test)]
mod obligations_tests;
mod overview;
#[cfg(test)]
mod overview_tests;
#[cfg(test)]
mod panels_tests;
mod progress;
#[cfg(test)]
mod progress_tests;
mod routing;
#[cfg(test)]
mod routing_tests;
mod run_state;
#[path = "detail_runs.rs"]
mod runs;
mod tabs;
mod timeline;
#[cfg(test)]
mod timeline_tests;
mod unstick;
mod usage;
use usage::summary as cost_summary;

/// Mouse zones ([`Hit::Item`]): the tab content box, the live tail, run rows, artifact rows and acceptance
/// record rows.
const Z_CONTENT: u8 = 0;
const Z_LIVE: u8 = 1;
const Z_RUN: u8 = 2;
const Z_ART: u8 = 3;
const Z_ACC: u8 = 4;

const TABS: [&str; 8] = [
    "Spec",
    "Runs",
    "Artifacts",
    "Diff",
    "Acceptance",
    "Comments",
    "Timeline",
    "Plan",
];
const ACCEPTANCE: usize = 4;
const COMMENTS: usize = 5;
const TIMELINE: usize = 6;
/// Shown only for plan-first tasks.
const PLAN: usize = 7;
const LIVE_LINES: usize = 200;

/// Card detail: header (id, title, project, status, priority, agent); left box "Task" with tabs
/// 1 Spec (task body), 2 Runs (table of runs), 3 Artifacts (list; ⏎ opens preview), 4 Diff (current run diff via
/// review_info when a run exists), 5 Acceptance (review policy and its sources, pipeline stage, gate and review
/// results, records; ⏎ previews a record); right box "Live" with `live_tail` (refreshed on `refresh`).
/// Keys: number / Tab switch tabs, j/k scroll or select, a -> Action::Jump (Timeline: toggle noise), Esc -> Back,
/// 6 Comments shows the thread; c opens an owner comment input (Enter saves, Tab toggles live send).
/// 7 Timeline shows task events; Enter opens raw JSON. Conditional 8 Plan stays last.
/// u -> confirm popup (pid + argv) that calls `corral_ops::unstick` for the current run's stuck process,
/// * pins the selected artifact (Artifacts tab) or the task.
pub struct DetailView {
    attention: crate::attention::Snapshot,
    overview: bool,
    show_live: bool,
    routing: routing::Routing,
    task: Task,
    stages: Option<crate::stages::StripState>,
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
    list: ArtList,
    preview: Option<PreviewPopup>,
    /// Open unstick confirmation: the stuck process as shown when `u` was pressed; ⏎ passes exactly this one.
    confirm: Option<StuckProcess>,
    /// Selected acceptance record (index into `acceptance::records`).
    acc_sel: usize,
    /// The task's review-pipeline events; loaded on visiting the Acceptance tab, reset on refresh.
    history: Option<Result<Vec<Event>, String>>,
    operating: Vec<Line<'static>>,
    timeline: timeline::TimelineTab,
    comment: Option<comments::Composer>,
    /// Pin refs, for the ★ markers.
    pins: HashSet<String>,
    plan: crate::plan::tab::PlanTab,
}

impl DetailView {
    pub fn new(ctx: &Ctx, task_id: &str) -> Result<DetailView> {
        corral_ops::refresh_usage(&ctx.store, task_id)?;
        let task = ctx.store.get(task_id)?;
        let live = load_live(ctx, &task);
        let list = ArtList::new(ctx, &task.artifacts);
        let art_sel = list.first(&task.artifacts);
        let stages = crate::stages::load_strip(ctx, &task);
        let plan = crate::plan::tab::PlanTab::new(ctx, &task);
        let mut routing = routing::Routing::default();
        routing.refresh(ctx, &task);
        let tab = if task.tracker { PLAN } else { 0 };
        let operating = acceptance::effective_lines(ctx, &task);
        Ok(DetailView {
            attention: crate::attention::load(ctx, jiff::Timestamp::now()),
            overview: true,
            show_live: false,
            routing,
            plan,
            task,
            stages,
            tab,
            scroll: Scroll::default(),
            live,
            live_scroll: 0,
            run_sel: None,
            diff: None,
            art_sel,
            list,
            preview: None,
            confirm: None,
            acc_sel: 0,
            history: None,
            operating,
            timeline: timeline::TimelineTab::default(),
            comment: None,
            pins: shelf::pinned_refs(ctx),
        })
    }

    /// The detail view opened on its Acceptance tab (from the review screen).
    pub fn acceptance(ctx: &Ctx, task_id: &str) -> Result<DetailView> {
        let mut v = DetailView::new(ctx, task_id)?;
        v.set_tab(ACCEPTANCE, ctx);
        Ok(v)
    }

    /// The detail view opened on its Plan tab; tasks without a plan open on Spec.
    pub fn plan(ctx: &Ctx, task_id: &str) -> Result<DetailView> {
        let mut v = DetailView::new(ctx, task_id)?;
        if v.plan.enabled {
            v.set_tab(PLAN, ctx);
        } else {
            v.set_tab(0, ctx);
        }
        Ok(v)
    }

    fn tab_names(&self) -> &'static [&'static str] {
        if self.plan.enabled {
            &TABS
        } else {
            &TABS[..PLAN]
        }
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

    fn spec_lines(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = &self.task;
        let mut lines = if t.body.trim().is_empty() {
            vec![Line::from(Span::styled("(empty spec)", theme::dim()))]
        } else {
            ui::text_lines(&t.body)
        };
        lines.extend(lessons::lines(ctx, t));
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

    fn artifact_lines(&self, width: usize) -> Vec<Line<'static>> {
        if self.task.artifacts.is_empty() {
            return vec![Line::from(Span::styled(
                "No artifacts yet. They appear on corral artifact add and at corral done.",
                theme::dim(),
            ))];
        }
        let mut lines = self.list.lines(
            &self.task.artifacts,
            &self.task.id,
            self.art_sel,
            true,
            &self.pins,
            width,
        );
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
        if self.overview {
            self.draw_overview(f, area, hits);
            return;
        }
        ui::paint_bg(f, area);
        let [head, stages, _, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(crate::stages::height(
                &self.task,
                self.stages.as_ref(),
                area.width,
            )),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .horizontal_margin(1)
        .areas(area);
        header::draw(f, head, self.header(), self.task.parent.as_deref(), hits);
        crate::stages::draw(f, stages, &self.task, None, self.stages.as_ref(), hits);

        let full_width = !self.show_live || matches!(self.tab, PLAN | TIMELINE);
        let [left, right] = Layout::horizontal([
            Constraint::Fill(5),
            if full_width {
                Constraint::Length(0)
            } else {
                Constraint::Fill(4)
            },
        ])
        .spacing(u16::from(!full_width))
        .areas(body);
        let inner = ui::boxed(f, left, "Task", true, None);
        let [tabs, _, content] = Layout::vertical([
            Constraint::Length(tabs::height(inner.width, self.tab_names())),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);
        hits.push(left, Hit::Item(Z_CONTENT, 0));
        tabs::draw(f, tabs, self.tab_names(), self.tab, hits);
        let content = if self.tab == COMMENTS {
            comments::toolbar(f, content, hits)
        } else if self.tab == PLAN {
            self.plan.toolbar(f, content, hits)
        } else {
            content
        };
        let mut acc_first = 0;
        let (lines, wrap) = match self.tab {
            0 => (self.spec_lines(ctx), true),
            1 => (self.runs_lines(ctx, usize::from(content.width)), false),
            2 => (self.artifact_lines(usize::from(content.width)), false),
            COMMENTS => (comments::lines(&self.task), true),
            TIMELINE => (self.timeline.lines(usize::from(content.width)), false),
            PLAN => (self.plan.lines(ctx, &self.task, content.width), false),
            ACCEPTANCE => {
                let history = self
                    .history
                    .as_ref()
                    .map(|h| h.as_deref().map_err(String::as_str));
                let (lines, first) =
                    acceptance::lines(&self.task, &ctx.store.config, history, self.acc_sel);
                let mut effective: Vec<Line<'static>> = self
                    .operating
                    .iter()
                    .flat_map(|line| {
                        let segments = line
                            .spans
                            .iter()
                            .map(|span| (span.content.to_string(), span.style))
                            .collect::<Vec<_>>();
                        crate::render::wrap::wrap(
                            &crate::render::wrap::chunks(&segments),
                            usize::from(content.width),
                        )
                        .into_iter()
                        .map(Line::from)
                    })
                    .collect();
                acc_first = first + effective.len();
                effective.extend(lines);
                (effective, false)
            }
            _ => (self.diff_lines(), false),
        };
        if self.tab == TIMELINE {
            self.timeline
                .follow_selection(&mut self.scroll, content.height);
        }
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
            2 => {
                let n = self.list.rows(&self.task.artifacts).len();
                hit::rows(hits, content, Z_ART, 0, n, offset);
            }
            ACCEPTANCE => {
                let n = acceptance::records(&self.task).len();
                hit::rows(hits, content, Z_ACC, acc_first, n, offset);
            }
            PLAN => self.plan.hits(hits, content, offset),
            TIMELINE => self.timeline.hits(hits, content, offset),
            _ => {}
        }

        if right.width > 0 {
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
        }

        if let Some(p) = &mut self.preview {
            p.draw(f, area, hits);
        }
        self.timeline.draw_overlay(f, area, hits);
        if let Some(s) = &self.confirm {
            self.draw_unstick(f, area, s, hits);
        }
        if let Some(composer) = &self.comment {
            composer.draw(f, area, hits);
        }
        self.plan.draw_sheet(f, area, hits);
        self.routing.draw(f, area, &self.task, hits);
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if self.overview {
            return self.overview_key(key, ctx);
        }
        if !ui::is_press(&key) {
            return Action::None;
        }
        if (self.timeline.overlay_open()
            || (self.tab == TIMELINE
                && self.preview.is_none()
                && self.confirm.is_none()
                && !self.routing.open))
            && let Some(action) = self.timeline.on_key(&key, &mut self.scroll)
        {
            return action;
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
        if self.comment.is_some() {
            return self.on_comment_key(key, ctx);
        }
        if self.routing.open
            && let Some(action) = self.routing.on_key(key, ctx, &self.task)
        {
            return action;
        }
        let on_plan = self.tab == PLAN;
        let was_details = self.plan.showing_details();
        if let Some(action) = self.plan.on_key(&key, ctx, &self.task, on_plan) {
            if self.plan.showing_details() != was_details {
                self.scroll = Scroll::default();
            }
            return action;
        }
        if key.code == KeyCode::F(5) && self.plan.enabled {
            self.set_tab(PLAN, ctx);
            return Action::None;
        }
        if let Some(action) = crate::stages::action(&key, &self.task.id) {
            return action;
        }
        if self.tab == 2
            && let Some(toast) =
                self.list
                    .on_key(ctx, &self.task.artifacts, &mut self.art_sel, &key)
        {
            return toast.map_or(Action::None, Action::Toast);
        }
        if let Some(action) = self.routing.on_key(key, ctx, &self.task) {
            return action;
        }
        match key.code {
            KeyCode::Char('0') => {
                self.overview = true;
                self.scroll = Scroll::default();
                Action::None
            }
            KeyCode::Char('L') => {
                self.show_live = !self.show_live;
                Action::None
            }
            KeyCode::Esc => Action::Back,
            KeyCode::Char('P') if self.task.parent.is_some() => self
                .task
                .parent
                .as_ref()
                .map_or(Action::None, |id| Action::OpenDetail(id.clone())),
            KeyCode::Char(c @ '1'..='8')
                if usize::from(c as u8 - b'1') < self.tab_names().len() =>
            {
                self.set_tab(usize::from(c as u8 - b'1'), ctx);
                Action::None
            }
            KeyCode::Tab => {
                self.set_tab(self.tab + 1, ctx);
                Action::None
            }
            KeyCode::BackTab => {
                self.set_tab(self.tab + self.tab_names().len() - 1, ctx);
                Action::None
            }
            KeyCode::Char('a') => Action::Jump(self.task.id.clone()),
            KeyCode::Char('c') if self.tab == COMMENTS => {
                self.comment = Some(comments::Composer::new(&self.task));
                Action::None
            }
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
            KeyCode::Enter | KeyCode::Char('o') if self.tab == 2 => match self
                .task
                .artifacts
                .get(self.art_sel)
                .map(|a| ui::activate(ctx, a))
            {
                Some(ui::Activation::Opened(toast)) => Action::Toast(toast),
                Some(ui::Activation::Preview(p)) => {
                    self.preview = Some(*p);
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
        self.detail_hints()
    }

    fn refresh(&mut self, ctx: &Ctx) {
        self.refresh_with_attention(ctx, crate::attention::load(ctx, jiff::Timestamp::now()));
    }

    fn refresh_board_snapshot(&mut self, ctx: &Ctx, snapshot: &crate::board::BoardSnapshot) {
        self.refresh_with_attention(ctx, snapshot.attention.clone());
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.mouse(ev, m)
    }

    fn captures_text(&self) -> bool {
        self.comment.is_some() || self.plan.sheet_open()
    }

    fn has_unsaved_text(&self) -> bool {
        self.comment
            .as_ref()
            .is_some_and(|c| !c.input.text.is_empty())
            || self.plan.has_unsaved_text()
    }
}

impl DetailView {
    fn refresh_with_attention(&mut self, ctx: &Ctx, attention: crate::attention::Snapshot) {
        self.attention = attention;
        if let Err(error) = corral_ops::refresh_usage(&ctx.store, &self.task.id) {
            corral_ops::report_message(
                Some(&self.task.id),
                "usage",
                format!("refreshing usage for {}: {error:#}", self.task.id),
            );
        }
        if let Ok(t) = ctx.store.get(&self.task.id) {
            if self.tab == PLAN
                && self.task.plans.last().map(|p| &p.id) != t.plans.last().map(|p| &p.id)
            {
                self.scroll = Scroll::default();
            }
            self.task = t;
            self.operating = acceptance::effective_lines(ctx, &self.task);
            self.routing.refresh(ctx, &self.task);
            self.stages = crate::stages::load_strip(ctx, &self.task);
            self.plan.refresh(ctx, &self.task);
        }
        self.list.sync(ctx, &self.task.artifacts);
        self.list.clamp(&self.task.artifacts, &mut self.art_sel);
        let records = acceptance::records(&self.task).len();
        self.acc_sel = self.acc_sel.min(records.saturating_sub(1));
        self.live = load_live(ctx, &self.task);
        self.pins = shelf::pinned_refs(ctx);
        self.diff = None;
        self.history = None;
        self.timeline.reset();
        if !self.overview && self.tab == TIMELINE {
            self.set_tab(TIMELINE, ctx);
        }
        if !self.overview && self.tab == ACCEPTANCE {
            self.set_tab(ACCEPTANCE, ctx);
        }
        if !self.overview && self.tab == 3 {
            self.set_tab(3, ctx);
        }
    }
}
