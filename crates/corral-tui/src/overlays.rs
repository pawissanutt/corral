//! Capture popup, dispatch sheet, reply line, help. Owned by the board worker.
//!
//! The board hosts these as its own overlays (the `Action` enum has no "push overlay" variant), drawing them
//! over a dimmed copy of itself. `CaptureView` is also the whole UI in `Mode::Capture`.

use std::path::PathBuf;

use corral_core::{RunMode, Status, Task};
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::choice::{self, RunChoice};
use crate::hit::{self, Hit, HitMap};
use crate::theme;
use crate::{Action, MouseCtx, Toast, Tone, View};

mod mouse;

/// Mouse zones ([`Hit::Item`]) of the capture popup and dispatch sheet pill rows.
const Z_PROJECT: u8 = 0;
const Z_STATUS: u8 = 1;
const Z_MODE: u8 = 3;
/// What the capture popup's status pills save as, in pill order.
const CAPTURE_STATUS: [(&str, Status); 2] = [("idea", Status::Idea), ("task", Status::Backlog)];

pub(crate) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

pub(crate) fn dim_backdrop(buf: &mut Buffer, area: Rect) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::reset().fg(theme::BORDER).bg(theme::BG));
            }
        }
    }
}

/// Draws a modal popup frame with a clickable `×` and returns its inner area.
pub(crate) fn popup(f: &mut Frame, area: Rect, title: &str, hits: &mut HitMap) -> Rect {
    f.render_widget(Clear, area);
    let block = theme::titled(title)
        .border_style(Style::new().fg(theme::DIM))
        .style(Style::new().bg(theme::BG2).fg(theme::FG));
    let inner = block.inner(area);
    f.render_widget(block, area);
    hit::modal_popup(f, area, hits);
    inner
}

pub(crate) fn focused_block(title: impl Into<String>) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::BORDER))
        .title(Span::styled(
            format!(" {} ", title.into()),
            Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD),
        ))
}

pub(crate) fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

pub(crate) fn hint_spans(hints: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (k, label) in hints {
        spans.push(Span::styled(k.to_string(), Style::new().fg(theme::CYAN)));
        spans.push(Span::styled(format!(" {label}  "), theme::dim()));
    }
    spans
}

fn plain_key(key: &KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LineInput {
    pub text: String,
}

impl LineInput {
    pub fn edit(&mut self, key: &KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.clear()
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let trimmed = self.text.trim_end().len();
                self.text.truncate(trimmed);
                let cut = self.text.rfind(' ').map_or(0, |i| i + 1);
                self.text.truncate(cut);
            }
            KeyCode::Char(c) if plain_key(key) => self.text.push(c),
            KeyCode::Backspace => {
                self.text.pop();
            }
            _ => return false,
        }
        true
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, prompt: &str, placeholder: &str) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let pw = prompt.width() + 1;
        let avail = (area.width as usize).saturating_sub(pw + 1);
        // Show the tail of long input so the cursor stays visible.
        let shown: String = if self.text.width() > avail {
            let mut w = 0;
            let mut tail: Vec<char> = Vec::new();
            for c in self.text.chars().rev() {
                w += c.width().unwrap_or(0);
                if w > avail {
                    break;
                }
                tail.push(c);
            }
            tail.into_iter().rev().collect()
        } else {
            self.text.clone()
        };
        let body = if shown.is_empty() {
            Span::styled(placeholder.to_string(), theme::dim())
        } else {
            Span::styled(shown.clone(), Style::new().fg(theme::FG))
        };
        let line = Line::from(vec![
            Span::styled(format!("{prompt} "), Style::new().fg(theme::CYAN)),
            body,
        ]);
        f.render_widget(Paragraph::new(line), area);
        let cx = area.x + (pw + shown.width()).min(area.width as usize - 1) as u16;
        f.set_cursor_position((cx, area.y));
    }
}

/// Capture popup: one line; ⏎ saves with the chosen status (Idea unless the `task` pill is picked), ⇥ saves a
/// Backlog task. The project starts as the one inferred from `cwd`; ↑↓ or a pill click picks another.
pub struct CaptureView {
    pub(crate) input: LineInput,
    cwd: PathBuf,
    project: Option<String>,
    projects: Vec<String>,
    status: Status,
}

impl CaptureView {
    pub fn new(ctx: &Ctx, cwd: PathBuf) -> CaptureView {
        let project = corral_ops::infer_project(ctx, &cwd);
        CaptureView {
            input: LineInput::default(),
            cwd,
            project,
            projects: ctx.store.config.projects.keys().cloned().collect(),
            status: Status::Idea,
        }
    }

    fn cycle_project(&mut self, delta: isize) {
        let n = self.projects.len();
        if n == 0 {
            return;
        }
        let at = self
            .projects
            .iter()
            .position(|p| Some(p) == self.project.as_ref());
        let next = match at {
            Some(i) => (i as isize + delta).rem_euclid(n as isize) as usize,
            None if delta > 0 => 0,
            None => n - 1,
        };
        self.project = self.projects.get(next).cloned();
    }

    fn save(&mut self, ctx: &Ctx, status: Status) -> Action {
        let title = self.input.text.trim().to_string();
        if title.is_empty() {
            return Action::Toast(Toast::new(
                Tone::Warn,
                "Nothing to capture",
                "type a title first",
            ));
        }
        match corral_ops::new_task(ctx, &title, self.project.as_deref(), &self.cwd, status, "") {
            Ok(t) => {
                let body = if status == Status::Idea {
                    "saved as an idea · i shows ideas only"
                } else {
                    "saved as a task in Backlog"
                };
                Action::BackWithToast(Toast::new(Tone::Good, format!("{} captured", t.id), body))
            }
            Err(e) => Action::Toast(Toast::new(Tone::Bad, "Capture failed", format!("{e:#}"))),
        }
    }
}

impl View for CaptureView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let rect = centered(area, area.width.saturating_sub(4).clamp(20, 76), 7);
        let inner = popup(f, rect, "capture", hits);
        if inner.height == 0 {
            return;
        }
        self.input
            .draw(f, Rect { height: 1, ..inner }, "›", "what's the idea?");
        if inner.height < 5 {
            return;
        }
        let row = |dy: u16| Rect::new(inner.x, inner.y + dy, inner.width, 1);
        let label = |s: &str| Span::styled(format!("{s:<9}"), theme::dim());
        let (px, right) = (inner.x + 9, inner.right());
        let mut project = vec![label("project")];
        if self.projects.is_empty() {
            project.push(Span::styled("none configured", theme::dim()));
        } else {
            let names: Vec<&str> = self.projects.iter().map(String::as_str).collect();
            let sel = self
                .project
                .as_ref()
                .and_then(|p| self.projects.iter().position(|x| x == p));
            project.extend(pills(&names, sel.unwrap_or(usize::MAX), "↑↓").spans);
            hit::pill_hits(hits, &names, Z_PROJECT, px, row(2).y, right);
        }
        f.render_widget(Paragraph::new(Line::from(project)), row(2));

        let names = CAPTURE_STATUS.map(|(n, _)| n);
        let sel = CAPTURE_STATUS.iter().position(|(_, s)| *s == self.status);
        let mut status = vec![label("save as")];
        status.extend(pills(&names, sel.unwrap_or(0), "").spans);
        hit::pill_hits(hits, &names, Z_STATUS, px, row(3).y, right);
        f.render_widget(Paragraph::new(Line::from(status)), row(3));

        let buttons = [
            ("Save", hit::press(KeyCode::Enter)),
            ("Cancel", hit::press(KeyCode::Esc)),
        ];
        let y = row(4).y;
        let hx = hit::buttons(f, hits, inner.x, y, right, &buttons) + 1;
        let hint = [("⇥", "task")];
        if hx < right {
            hit::hint_hits(hits, &hint, hx, y, right);
            let r = Rect::new(hx, y, right - hx, 1);
            f.render_widget(Paragraph::new(Line::from(hint_spans(&hint))), r);
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Enter => self.save(ctx, self.status),
            KeyCode::Tab => self.save(ctx, Status::Backlog),
            KeyCode::Up => {
                self.cycle_project(-1);
                Action::None
            }
            KeyCode::Down => {
                self.cycle_project(1);
                Action::None
            }
            KeyCode::Left | KeyCode::Right => {
                self.status = if key.code == KeyCode::Left {
                    Status::Idea
                } else {
                    Status::Backlog
                };
                Action::None
            }
            _ => {
                self.input.edit(&key);
                Action::None
            }
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        let save = if self.status == Status::Idea {
            "save idea"
        } else {
            "save task"
        };
        vec![
            ("⏎", save),
            ("⇥", "save as task"),
            ("↑↓", "project"),
            ("←→", "save as"),
            ("esc", "cancel"),
        ]
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.mouse(ev, m)
    }

    fn is_overlay(&self) -> bool {
        true
    }

    fn captures_text(&self) -> bool {
        true
    }

    fn has_unsaved_text(&self) -> bool {
        !self.input.text.is_empty()
    }
}

/// Dispatch sheet for a Ready/Backlog/Idea task: profile (h/l) with per-run model (M) and effort (e) overrides,
/// mode (m), base, task body preview; ⏎ dispatches with `queue_if_blocked`.
pub struct DispatchSheet {
    pub(crate) task_id: String,
    title: String,
    project: Option<String>,
    pub(crate) choice: RunChoice,
    pub(crate) mode: RunMode,
    base: String,
    worktree: String,
    blocked_by: Vec<String>,
    slots: (usize, usize),
    body: Vec<String>,
}

impl DispatchSheet {
    pub fn new(ctx: &Ctx, task: &Task, all: &[Task]) -> DispatchSheet {
        let cfg = &ctx.store.config;
        let project_cfg = task.project.as_ref().and_then(|p| cfg.projects.get(p));
        let preferred = cfg
            .route_task(task, &corral_core::ProfileChoice::default(), |_| None)
            .map(|r| r.profile)
            .unwrap_or_else(|_| {
                cfg.default_profile(task.profile.as_deref(), task.project.as_deref())
            });
        let base = match (project_cfg.and_then(|p| p.base.clone()), &task.project) {
            (Some(b), _) => b,
            (None, Some(p)) => ctx
                .store
                .project_path(p)
                .and_then(|path| corral_git::current_branch(&path))
                .unwrap_or_else(|_| "current branch".into()),
            (None, None) => "no project".into(),
        };
        let n = task.runs.len() + 1;
        let suffix = if n > 1 {
            format!("-r{n}")
        } else {
            String::new()
        };
        let worktree = match &task.project {
            Some(p) => format!("{p}/.worktrees/{}{suffix}", task.id),
            None => "needs a project".into(),
        };
        let blocked_by = task
            .depends_on
            .iter()
            .filter(|d| !all.iter().any(|t| &t.id == *d && t.status == Status::Done))
            .cloned()
            .collect();
        let used = all.iter().filter(|t| t.status.holds_slot()).count();
        let body = task
            .body
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .take(6)
            .map(String::from)
            .collect();
        DispatchSheet {
            task_id: task.id.clone(),
            title: task.title.clone(),
            project: task.project.clone(),
            choice: RunChoice::new(cfg, &preferred, None, None),
            mode: RunMode::Interactive,
            base,
            worktree,
            blocked_by,
            slots: (used, cfg.limits.running),
            body,
        }
    }

    pub(crate) fn dispatch_opts(&self) -> DispatchOpts {
        let choice = self.choice.choice();
        DispatchOpts {
            profile: choice.profile.filter(|_| self.choice.profile_selected),
            model: choice.model,
            effort: choice.effort,
            mode: self.mode,
            queue_if_blocked: true,
            ..DispatchOpts::default()
        }
    }

    fn dispatch(&self, ctx: &Ctx) -> Action {
        let opts = self.dispatch_opts();
        match corral_ops::dispatch(ctx, &self.task_id, &opts) {
            Ok(DispatchOutcome::Started { task, .. }) => {
                let run = task
                    .current_run()
                    .map(choice::run_label)
                    .unwrap_or_default();
                Action::BackWithToast(Toast::new(
                    Tone::Info,
                    format!("⇢ {} dispatched", task.id),
                    format!(
                        "{} · {run} in {}",
                        self.choice.display_name(),
                        self.worktree
                    ),
                ))
            }
            Ok(DispatchOutcome::Queued { task, reason }) => Action::BackWithToast(Toast::new(
                Tone::Warn,
                format!("⏳ {} queued", task.id),
                reason,
            )),
            Err(e) => Action::BackWithToast(Toast::new(
                Tone::Bad,
                format!("Dispatch {} failed", self.task_id),
                corral_herdr::friendly_error(&e),
            )),
        }
    }
}

/// Dispatch sheet run modes, in pill order.
const MODES: [&str; 2] = ["interactive", "headless"];

pub(crate) fn pills(options: &[&str], selected: usize, key: &str) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, o) in options.iter().enumerate() {
        let style = if i == selected {
            Style::new()
                .fg(theme::CYAN)
                .bg(theme::SELECTED_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            theme::dim()
        };
        let text = if i == selected {
            format!("[{o}]")
        } else {
            format!(" {o} ")
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(format!(" {key}"), theme::dim()));
    Line::from(spans)
}

impl View for DispatchSheet {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let warn_rows =
            u16::from(!self.blocked_by.is_empty()) + u16::from(self.slots.0 >= self.slots.1);
        let body_rows = self.body.len().max(1) as u16;
        let rect = centered(
            area,
            area.width.saturating_sub(4).min(84),
            8 + choice::ROWS - 1 + body_rows + 2 + warn_rows + 1,
        );
        let inner = popup(f, rect, &format!("dispatch {}", self.task_id), hits);
        let label = |s: &str| Span::styled(format!("{s:<10}"), theme::dim());
        let proj = match &self.project {
            Some(p) => Span::styled(p.clone(), Style::new().fg(theme::project_color(p))),
            None => Span::styled("no project", Style::new().fg(theme::RED)),
        };
        let mode_idx = usize::from(self.mode == RunMode::Headless);
        let head = 5 + choice::ROWS;
        let mut lines = vec![
            Line::from(vec![
                label("task"),
                Span::styled(self.title.clone(), theme::bold()),
                Span::styled(" · ", theme::dim()),
                proj,
            ]),
            Line::from(""),
            Line::from(""),
            Line::from(""),
            Line::from(vec![label("mode")]),
            Line::from(vec![label("base"), Span::raw(self.base.clone())]),
            Line::from(vec![
                label("worktree"),
                Span::raw(self.worktree.clone()),
                Span::styled(format!("  branch corral/{}", self.task_id), theme::dim()),
            ]),
            Line::from(""),
        ];
        let mode_row = 1 + choice::ROWS;
        lines[usize::from(mode_row)]
            .spans
            .extend(pills(&MODES, mode_idx, "m").spans);
        f.render_widget(Paragraph::new(lines), inner);
        let choice_area = Rect {
            y: inner.y + 1,
            height: choice::ROWS.min(inner.height.saturating_sub(1)),
            ..inner
        };
        self.choice.draw(f, choice_area, 10, hits);
        if mode_row < inner.height {
            let (x, y) = (inner.x + 10, inner.y + mode_row);
            hit::pill_hits(hits, &MODES, Z_MODE, x, y, inner.right());
        }
        if inner.height > 7 {
            let buttons = [
                ("Dispatch", hit::press(KeyCode::Enter)),
                ("Cancel", hit::press(KeyCode::Esc)),
            ];
            let y = inner.bottom() - 1;
            hit::buttons(f, hits, inner.x, y, inner.right(), &buttons);
        }

        let box_area = Rect {
            y: inner.y + head,
            height: (body_rows + 2).min(inner.height.saturating_sub(head)),
            ..inner
        };
        let body_block = theme::titled("task body").style(Style::new().bg(theme::BG2));
        let body_inner = body_block.inner(box_area);
        f.render_widget(body_block, box_area);
        let body: Vec<Line> = if self.body.is_empty() {
            vec![Line::styled("(empty task body)", theme::dim())]
        } else {
            self.body
                .iter()
                .map(|l| Line::styled(truncate(l, body_inner.width as usize), theme::dim()))
                .collect()
        };
        f.render_widget(Paragraph::new(body), body_inner);

        let mut warns = Vec::new();
        if !self.blocked_by.is_empty() {
            let deps = self.blocked_by.join(", ");
            warns.push(Line::styled(
                format!("⛓ blocked by {deps}; ⏎ queues it until they are done"),
                Style::new().fg(theme::RED),
            ));
        }
        if self.slots.0 >= self.slots.1 {
            let (u, l) = self.slots;
            warns.push(Line::styled(
                format!("⚠ slots {u}/{l}. ⏎ queues the task; it dispatches automatically when a slot frees."),
                Style::new().fg(theme::ORANGE),
            ));
        }
        let wy = box_area.bottom();
        let warn_bottom = inner.bottom().saturating_sub(1);
        if wy < warn_bottom {
            f.render_widget(
                Paragraph::new(warns),
                Rect {
                    y: wy,
                    height: warn_bottom - wy,
                    ..inner
                },
            );
        }
        self.choice.draw_popups(f, area, hits);
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.mouse(ev, m)
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if let Some(action) = self.choice.on_key(&key, ctx) {
            return action;
        }
        match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Char('m') => {
                self.mode = match self.mode {
                    RunMode::Interactive => RunMode::Headless,
                    RunMode::Headless => RunMode::Interactive,
                };
                Action::None
            }
            KeyCode::Enter => self.dispatch(ctx),
            _ => Action::None,
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut h = self.choice.hints();
        if !self.choice.modal() {
            h.extend([("m", "mode"), ("⏎", "dispatch"), ("esc", "cancel")]);
        }
        h
    }

    fn is_overlay(&self) -> bool {
        true
    }

    fn has_unsaved_text(&self) -> bool {
        self.choice.has_unsaved_text()
    }
}

pub(crate) const HELP_ROWS: [(&str, &str); 21] = [
    ("h j k l / arrows", "move selection"),
    ("⏎", "open card / review"),
    ("a", "jump to agent pane"),
    ("r", "reply to blocked agent inline"),
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
    ("n / c", "capture an idea"),
    ("A", "artifact finder"),
    ("Y", "history: finished and dropped tasks"),
    ("P", "PM agent pane"),
    ("i / D", "ideas only / show done"),
    (",", "settings"),
    (
        "O",
        "review autopilot on / off (also click it in the header)",
    ),
    ("esc", "back"),
    ("q", "quit (board)"),
    ("ctrl+c", "quit from anywhere"),
    ("F8 F9 F10", "activate toast (newest first)"),
    (
        "",
        "herdr: ctrl+b k board · ctrl+b n capture · ctrl+b a artifacts",
    ),
];

pub struct HelpView;

impl View for HelpView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let rect = centered(area, 72, HELP_ROWS.len() as u16 + 2);
        let inner = popup(f, rect, "keys", hits);
        for (dy, (k, _)) in HELP_ROWS.iter().enumerate() {
            let y = inner.y + dy as u16;
            if y >= inner.bottom() {
                break;
            }
            for (off, w, key) in hit::row_keys(k) {
                let x = inner.x + off;
                if x < inner.right() {
                    let w = w.min(inner.right() - x);
                    hits.push(Rect::new(x, y, w, 1), Hit::Behind(key));
                }
            }
        }
        let lines: Vec<Line> = HELP_ROWS
            .iter()
            .map(|(k, v)| {
                Line::from(vec![
                    Span::styled(format!("{k:<18}"), Style::new().fg(theme::CYAN)),
                    Span::raw(v.to_string()),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn on_key(&mut self, _key: KeyEvent, _ctx: &Ctx) -> Action {
        Action::Back
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("any key", "close")]
    }

    fn is_overlay(&self) -> bool {
        true
    }
}

/// Inline reply to a NeedsMe task, drawn at the bottom of the board ("Reply → T-xxx").
pub(crate) struct ReplyLine {
    pub task_id: String,
    excerpt: String,
    pub input: LineInput,
}

impl ReplyLine {
    pub const HEIGHT: u16 = 4;

    pub fn new(task: &Task) -> ReplyLine {
        let excerpt = task
            .current_run()
            .and_then(|r| r.blocked_excerpt.as_deref())
            .and_then(|e| e.lines().rev().find(|l| !l.trim().is_empty()))
            .unwrap_or("")
            .trim()
            .to_string();
        ReplyLine {
            task_id: task.id.clone(),
            excerpt,
            input: LineInput::default(),
        }
    }

    /// Draws the reply box; a click inside it stays there, and `[Send]` `[Cancel]` press ⏎ and Esc.
    pub fn draw(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        f.render_widget(Clear, area);
        let block = focused_block(format!("Reply → {} · sent with agent.prompt", self.task_id))
            .style(Style::new().bg(theme::BG));
        let inner = block.inner(area);
        f.render_widget(block, area);
        hits.push(area, Hit::Inert);
        if inner.height == 0 {
            return;
        }
        let ex = truncate(&self.excerpt, inner.width as usize);
        f.render_widget(
            Paragraph::new(Line::styled(ex, theme::dim())),
            Rect { height: 1, ..inner },
        );
        if inner.height >= 2 {
            let labels = ["Send", "Cancel"];
            let bw = hit::buttons_width(&labels);
            let row = Rect {
                y: inner.y + 1,
                height: 1,
                width: inner.width.saturating_sub(bw + 1),
                ..inner
            };
            self.input.draw(f, row, "reply ›", "type your answer…");
            let keys = [hit::press(KeyCode::Enter), hit::press(KeyCode::Esc)];
            let buttons: Vec<_> = labels.into_iter().zip(keys).collect();
            let x = inner.right().saturating_sub(bw);
            hit::buttons(f, hits, x, row.y, inner.right(), &buttons);
        }
    }

    /// Handles a key; Some(action) ends the reply line.
    pub fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Option<Action> {
        match key.code {
            KeyCode::Esc => Some(Action::None),
            KeyCode::Enter => {
                let text = self.input.text.trim();
                if text.is_empty() {
                    return None;
                }
                Some(match corral_ops::reply(ctx, &self.task_id, text) {
                    Ok(t) => Action::Toast(Toast::new(
                        Tone::Info,
                        format!("↩ replied to {}", t.id),
                        "sent with agent.prompt; the card is back in Running",
                    )),
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        format!("Reply to {} failed", self.task_id),
                        corral_herdr::friendly_error(&e),
                    )),
                })
            }
            _ => {
                self.input.edit(&key);
                None
            }
        }
    }
}
