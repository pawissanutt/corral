use anyhow::Result;
use corral_core::{ArtifactKind, Config, Status, Task};
use corral_ops::Ctx;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::overlays::{self, CaptureView, DispatchSheet, HelpView, ReplyLine, truncate};
use crate::theme;
use crate::{Action, Hit, HitMap, MouseCtx, Toast, Tone, View};

mod menu;
mod mouse;
#[cfg(test)]
pub(crate) mod tests;

const MOVE_RULE: &str = "H/L moves cards between Backlog and Ready. Dispatch, herdr state and `corral done` move tasks into \
Running, Needs me and Review; approve moves Review to Done.";

pub(crate) enum Overlay {
    Capture(CaptureView),
    Dispatch(DispatchSheet),
    Help(HelpView),
}

impl Overlay {
    fn view(&mut self) -> &mut dyn View {
        match self {
            Overlay::Capture(v) => v,
            Overlay::Dispatch(v) => v,
            Overlay::Help(v) => v,
        }
    }

    fn view_ref(&self) -> &dyn View {
        match self {
            Overlay::Capture(v) => v,
            Overlay::Dispatch(v) => v,
            Overlay::Help(v) => v,
        }
    }
}

/// The kanban board (see mock "Board" scene).
pub struct BoardView {
    now: Box<dyn Fn() -> Timestamp>,
    tasks: Vec<Task>,
    col: usize,
    row: usize,
    selected: Option<String>,
    show_done: bool,
    ideas_only: bool,
    offsets: [usize; 6],
    /// Keep the selected card in view; the wheel turns this off until the selection moves again.
    follow: bool,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) reply: Option<ReplyLine>,
    menu: Option<menu::CardMenu>,
    drag: Option<mouse::Drag>,
    /// Column and card rects from the last frame, for wheel, drop targets and menu placement.
    col_rects: Vec<(Status, Rect)>,
    card_rects: Vec<(String, Rect)>,
}

impl BoardView {
    pub fn new(ctx: &Ctx) -> Result<BoardView> {
        Self::with_clock(ctx, Box::new(Timestamp::now))
    }

    pub(crate) fn with_clock(ctx: &Ctx, now: Box<dyn Fn() -> Timestamp>) -> Result<BoardView> {
        let tasks = ctx.store.list()?;
        let mut board = BoardView {
            now,
            tasks,
            col: 0,
            row: 0,
            selected: None,
            show_done: false,
            ideas_only: false,
            offsets: [0; 6],
            follow: true,
            overlay: None,
            reply: None,
            menu: None,
            drag: None,
            col_rects: Vec::new(),
            card_rects: Vec::new(),
        };
        board.select_current();
        Ok(board)
    }

    fn columns(&self) -> Vec<Status> {
        Status::COLUMNS
            .into_iter()
            .filter(|s| *s != Status::Done || self.show_done)
            .collect()
    }

    fn column_tasks(&self, col: Status) -> Vec<&Task> {
        let mut v: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.status.column() == Some(col))
            .filter(|t| !(self.ideas_only && col == Status::Backlog) || t.is_idea())
            .collect();
        v.sort_by(|a, b| a.priority.cmp(&b.priority));
        v
    }

    pub(crate) fn selected_task(&self) -> Option<&Task> {
        let cols = self.columns();
        let col = *cols.get(self.col.min(cols.len().saturating_sub(1)))?;
        let tasks = self.column_tasks(col);
        tasks
            .get(self.row.min(tasks.len().saturating_sub(1)))
            .copied()
    }

    fn select_current(&mut self) {
        let cols = self.columns();
        self.col = self.col.min(cols.len().saturating_sub(1));
        let n = cols
            .get(self.col)
            .map_or(0, |c| self.column_tasks(*c).len());
        self.row = self.row.min(n.saturating_sub(1));
        self.selected = self.selected_task().map(|t| t.id.clone());
    }

    fn restore_selection(&mut self) {
        if let Some(id) = self.selected.clone() {
            for (ci, col) in self.columns().into_iter().enumerate() {
                if let Some(ri) = self.column_tasks(col).iter().position(|t| t.id == id) {
                    self.col = ci;
                    self.row = ri;
                    return;
                }
            }
        }
        self.select_current();
    }

    /// What the board returns after its open overlay handled an input: Back closes the overlay.
    fn overlay_outcome(&mut self, action: Action, ctx: &Ctx) -> Action {
        match action {
            Action::Back => {
                self.overlay = None;
                Action::None
            }
            Action::BackWithToast(t) => {
                self.overlay = None;
                self.refresh(ctx);
                Action::Toast(t)
            }
            other => other,
        }
    }

    fn move_card(&mut self, ctx: &Ctx, right: bool) -> Action {
        let Some(t) = self.selected_task() else {
            return Action::None;
        };
        let target = match (t.status, right) {
            (Status::Idea | Status::Backlog, true) => Status::Ready,
            (Status::Ready, false) => Status::Backlog,
            _ => return Action::Toast(Toast::new(Tone::Warn, "Not a manual move", MOVE_RULE)),
        };
        let id = t.id.clone();
        match corral_ops::move_task(ctx, &id, target, false) {
            Ok(_) => {
                self.refresh(ctx);
                Action::None
            }
            Err(e) => Action::Toast(Toast::new(
                Tone::Bad,
                format!("Move {id} failed"),
                format!("{e:#}"),
            )),
        }
    }

    fn board_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let ncols = self.columns().len();
        let status = self.selected_task().map(|t| t.status);
        let id = self.selected_task().map(|t| t.id.clone());
        self.follow = true;
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.col = self.col.saturating_sub(1);
                self.row = 0;
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.col = (self.col + 1).min(ncols.saturating_sub(1));
                self.row = 0;
            }
            KeyCode::Char('j') | KeyCode::Down => self.row += 1,
            KeyCode::Char('k') | KeyCode::Up => self.row = self.row.saturating_sub(1),
            KeyCode::Enter => {
                return match (status, id) {
                    (Some(Status::Review), Some(id)) => Action::OpenReview(id),
                    (Some(_), Some(id)) => Action::OpenDetail(id),
                    _ => Action::None,
                };
            }
            KeyCode::Char('a') => {
                return match (status, id) {
                    (Some(s), Some(id)) if s.holds_slot() => Action::Jump(id),
                    _ => Action::Toast(Toast::new(
                        Tone::Info,
                        "No live run",
                        "a jumps to Running and Needs me cards",
                    )),
                };
            }
            KeyCode::Char('r') => match self.selected_task() {
                Some(t) if t.status == Status::NeedsMe => self.reply = Some(ReplyLine::new(t)),
                _ => {
                    return Action::Toast(Toast::new(
                        Tone::Info,
                        "Nothing to reply to",
                        "r replies to Needs me cards",
                    ));
                }
            },
            KeyCode::Char('d') => match self.selected_task() {
                Some(t) if matches!(t.status, Status::Idea | Status::Backlog | Status::Ready) => {
                    self.overlay = Some(Overlay::Dispatch(DispatchSheet::new(ctx, t, &self.tasks)));
                }
                _ => {
                    return Action::Toast(Toast::new(
                        Tone::Info,
                        "Not dispatchable",
                        "d dispatches Backlog, Ready and idea cards",
                    ));
                }
            },
            KeyCode::Char('H') => return self.move_card(ctx, false),
            KeyCode::Char('L') => return self.move_card(ctx, true),
            KeyCode::Char('n') | KeyCode::Char('c') => {
                let cwd =
                    std::env::current_dir().unwrap_or_else(|_| ctx.store.root().to_path_buf());
                self.overlay = Some(Overlay::Capture(CaptureView::new(ctx, cwd)));
            }
            KeyCode::Char('P') => {
                return match corral_ops::open_pm(ctx) {
                    Ok(pane) => Action::Toast(Toast::new(
                        Tone::Info,
                        "PM pane focused",
                        format!("corral PM · {pane}"),
                    )),
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        "Could not open the PM",
                        format!("{e:#}"),
                    )),
                };
            }
            KeyCode::Char('A') => return Action::OpenArtifacts,
            KeyCode::Char('.') => self.open_menu(),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help(HelpView)),
            KeyCode::Char('i') => self.ideas_only = !self.ideas_only,
            KeyCode::Char('D') => self.show_done = !self.show_done,
            _ => return Action::None,
        }
        self.select_current();
        Action::None
    }

    /// The `○ ideas` / `○ done` switch drawn on a column's top border after its title; a click presses its key.
    fn draw_toggle(&self, f: &mut Frame, area: Rect, col: Status, right_w: u16, hits: &mut HitMap) {
        let (key, on, label) = match col {
            Status::Backlog => ('i', self.ideas_only, "ideas"),
            _ if self.columns().last() == Some(&col) => ('D', self.show_done, "done"),
            _ => return,
        };
        let text = format!(" {} {label} ", if on { "◉" } else { "○" });
        let x = area.x + col.label().width() as u16 + 4;
        let w = text.width() as u16;
        if x + w + right_w + 1 >= area.right() {
            return;
        }
        let style = if on {
            Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD)
        } else {
            theme::dim()
        };
        let rect = Rect::new(x, area.y, w, 1);
        f.render_widget(Span::styled(text, style), rect);
        hits.push(
            rect,
            Hit::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
        );
    }

    fn draw_column(
        &mut self,
        f: &mut Frame,
        area: Rect,
        col: Status,
        config: &Config,
        now: Timestamp,
        hits: &mut HitMap,
    ) {
        let focused = self.columns().get(self.col) == Some(&col);
        let title = col.label().to_string();
        let tasks = self.column_tasks(col);
        let right = match col {
            Status::Running => Span::styled(
                format!(" {}/{} ", tasks.len(), config.limits.running),
                theme::dim(),
            ),
            Status::NeedsMe if !tasks.is_empty() => {
                Span::styled(" ● ", Style::new().fg(theme::YELLOW))
            }
            _ => Span::styled(format!(" {} ", tasks.len()), theme::dim()),
        };
        let block = if focused {
            overlays::focused_block(title)
        } else {
            theme::titled(title)
        };
        let drop_ok = self.drop_target(col);
        let block = match drop_ok {
            Some(true) => block.border_style(Style::new().fg(theme::GREEN)),
            _ => block,
        };
        let right_w = right.width() as u16;
        let block = block.title(Line::from(right).right_aligned());
        let inner = block.inner(area);
        f.render_widget(block, area);
        hits.push(area, Hit::Column(col));
        self.draw_toggle(f, area, col, right_w, hits);
        if inner.width < 4 || inner.height == 0 {
            return;
        }
        if tasks.is_empty() {
            f.render_widget(Paragraph::new(Line::styled(" —", theme::dim())), inner);
            return;
        }
        let cw = inner.width as usize - 2;
        let sel_row = if focused {
            Some(self.row.min(tasks.len() - 1))
        } else {
            None
        };
        let cards: Vec<Vec<Line<'static>>> = tasks
            .iter()
            .enumerate()
            .map(|(i, t)| card_lines(t, &self.tasks, config, cw, sel_row == Some(i), now))
            .collect();
        let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();

        let slot = Status::COLUMNS.iter().position(|s| *s == col).unwrap_or(0);
        let mut offset = self.offsets[slot].min(cards.len() - 1);
        if let (Some(sel), true) = (sel_row, self.follow) {
            offset = offset.min(sel);
            while offset < sel
                && cards[offset..=sel]
                    .iter()
                    .map(|c| c.len() + 1)
                    .sum::<usize>()
                    > inner.height as usize + 1
            {
                offset += 1;
            }
        }
        self.offsets[slot] = offset;

        let mut y = inner.y;
        for (i, lines) in cards.into_iter().enumerate().skip(offset) {
            if y >= inner.bottom() {
                break;
            }
            let h = (lines.len() as u16).min(inner.bottom() - y);
            let mut p = Paragraph::new(lines);
            if sel_row == Some(i) {
                p = p.style(Style::new().bg(theme::SELECTED_BG));
            }
            let rect = Rect {
                y,
                height: h,
                ..inner
            };
            f.render_widget(p, rect);
            hits.push(rect, Hit::Card(ids[i].clone()));
            hits.push(
                Rect::new(inner.right() - 2, y, 2, 1),
                Hit::CardMenu(ids[i].clone()),
            );
            self.card_rects.push((ids[i].clone(), rect));
            y += h + 1;
        }
    }
}

fn ago(ts: Timestamp, now: Timestamp) -> String {
    let s = (now.as_second() - ts.as_second()).max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d", s / 86400),
    }
}

fn wrap2(text: &str, width: usize) -> Vec<String> {
    let mut first = String::new();
    let mut rest = text;
    for (i, word) in text.split(' ').enumerate() {
        let candidate = if i == 0 {
            word.to_string()
        } else {
            format!("{first} {word}")
        };
        if candidate.width() > width {
            break;
        }
        first = candidate;
        rest = text[first.len()..].trim_start();
    }
    if first.is_empty() {
        return vec![truncate(text, width)];
    }
    if rest.is_empty() {
        vec![first]
    } else {
        vec![first, truncate(rest, width)]
    }
}

fn agent_span(agent: &str) -> Span<'static> {
    Span::styled(
        agent.to_string(),
        Style::new().fg(theme::agent_color(agent)),
    )
}

/// "+A −R" parsed from the run's "Final diff +A −R" artifact title.
fn diff_counts(task: &Task, run_id: &str) -> Option<(String, String)> {
    let a = task
        .artifacts
        .iter()
        .rev()
        .find(|a| a.run == run_id && a.kind == ArtifactKind::Diff)?;
    let mut words = a
        .title
        .split_whitespace()
        .filter(|w| w.starts_with('+') || w.starts_with('−') || w.starts_with('-'));
    Some((words.next()?.to_string(), words.next()?.to_string()))
}

fn meta_lines(
    t: &Task,
    all: &[Task],
    config: &Config,
    width: usize,
    now: Timestamp,
) -> Vec<Line<'static>> {
    let dim = theme::dim();
    let sep = || Span::styled(" · ", theme::dim());
    let open_deps: Vec<&String> = t
        .depends_on
        .iter()
        .filter(|d| !all.iter().any(|x| &x.id == *d && x.status == Status::Done))
        .collect();
    let deps_queued = |spans: &mut Vec<Span<'static>>| {
        if !open_deps.is_empty() {
            let deps = open_deps
                .iter()
                .map(|d| d.as_str())
                .collect::<Vec<_>>()
                .join(",");
            if !spans.is_empty() {
                spans.push(sep());
            }
            spans.push(Span::styled(
                format!("⛓ {deps}"),
                Style::new().fg(theme::RED),
            ));
        }
        if t.queued {
            if !spans.is_empty() {
                spans.push(sep());
            }
            spans.push(Span::styled("⏳ queued", Style::new().fg(theme::YELLOW)));
        }
    };
    let run = t.current_run();
    let agent = run
        .map(|r| r.agent.clone())
        .unwrap_or_else(|| config.default_profile(t.profile.as_deref(), t.project.as_deref()));
    let mut out = Vec::new();
    match t.status {
        Status::Idea => {
            let mut spans = vec![Span::styled("◌ idea", dim)];
            deps_queued(&mut spans);
            out.push(Line::from(spans));
        }
        Status::Backlog => {
            let mut spans = Vec::new();
            deps_queued(&mut spans);
            if !spans.is_empty() {
                out.push(Line::from(spans));
            }
        }
        Status::Ready => {
            let mut spans = vec![Span::styled("⇢ ", dim), agent_span(&agent)];
            deps_queued(&mut spans);
            out.push(Line::from(spans));
        }
        Status::Running => {
            let mut spans = vec![
                agent_span(&agent),
                Span::styled(" ⟳ ", Style::new().fg(theme::BLUE)),
            ];
            if let Some(r) = run {
                spans.push(Span::styled(ago(r.started, now), dim));
                if r.idle_no_report {
                    spans.push(sep());
                    spans.push(Span::styled(
                        "⚠ idle, no report",
                        Style::new().fg(theme::YELLOW),
                    ));
                } else if let Some(state) = &r.agent_state {
                    spans.push(sep());
                    spans.push(Span::styled(state.clone(), dim));
                }
            }
            out.push(Line::from(spans));
        }
        Status::NeedsMe => {
            out.push(Line::from(vec![
                agent_span(&agent),
                Span::styled(" ? ", Style::new().fg(theme::YELLOW)),
                Span::styled(format!("waiting {}", ago(t.updated, now)), dim),
            ]));
            let question = run
                .and_then(|r| r.blocked_excerpt.as_deref())
                .and_then(|e| e.lines().map(str::trim).rfind(|l| !l.is_empty()))
                .unwrap_or("the agent is waiting for you");
            let style = Style::new()
                .fg(theme::YELLOW)
                .add_modifier(Modifier::ITALIC);
            out.extend(
                wrap2(&format!("\"{question}\""), width)
                    .into_iter()
                    .map(|l| Line::styled(l, style)),
            );
        }
        Status::Review => {
            let mut spans = vec![agent_span(&agent)];
            if let Some(r) = run {
                if let Some((add, del)) = diff_counts(t, &r.id) {
                    spans.push(sep());
                    spans.push(Span::styled(add, Style::new().fg(theme::GREEN)));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(del, Style::new().fg(theme::RED)));
                }
                let n = t.artifacts.iter().filter(|a| a.run == r.id).count();
                spans.push(sep());
                spans.push(Span::styled(format!("◆ {n} art"), dim));
            }
            out.push(Line::from(spans));
        }
        Status::Done => out.push(Line::styled("✓", Style::new().fg(theme::GREEN))),
        Status::Dropped => {}
    }
    out
}

fn card_lines(
    t: &Task,
    all: &[Task],
    config: &Config,
    width: usize,
    selected: bool,
    now: Timestamp,
) -> Vec<Line<'static>> {
    let bar = if selected {
        Span::styled("▌ ", Style::new().fg(theme::CYAN))
    } else if t.status == Status::NeedsMe {
        // Distinct from the selection bar so the focused card is never ambiguous.
        Span::styled("▲ ", Style::new().fg(theme::YELLOW))
    } else {
        Span::raw("  ")
    };
    let id_w = t.id.width();
    let prio_w = t.priority.width();
    let proj_room = width.saturating_sub(id_w + prio_w + 4);
    let proj = t
        .project
        .as_deref()
        .map(|p| truncate(p, proj_room))
        .unwrap_or_default();
    let pad = width.saturating_sub(id_w + 1 + proj.width() + prio_w + 2);
    let proj_style = Style::new().fg(t
        .project
        .as_deref()
        .map_or(theme::DIM, theme::project_color));
    let r1 = Line::from(vec![
        bar.clone(),
        Span::styled(t.id.clone(), theme::bold()),
        Span::raw(" "),
        Span::styled(proj, proj_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(t.priority.clone(), theme::dim()),
        Span::styled(" ⋯", theme::dim()),
    ]);
    let mut title_style = Style::new().fg(theme::FG);
    if selected {
        title_style = title_style.add_modifier(Modifier::BOLD);
    }
    let mut lines = vec![
        r1,
        Line::from(vec![
            bar.clone(),
            Span::styled(truncate(&t.title, width), title_style),
        ]),
    ];
    for meta in meta_lines(t, all, config, width, now) {
        let mut spans = vec![bar.clone()];
        let mut used = 0;
        for s in meta.spans {
            let room = width.saturating_sub(used);
            if room == 0 {
                break;
            }
            let text = truncate(&s.content, room);
            used += text.width();
            spans.push(Span::styled(text, s.style.patch(meta.style)));
        }
        lines.push(Line::from(spans));
    }
    lines
}

impl View for BoardView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        let now = (self.now)();
        let (board, reply_area) = match &self.reply {
            Some(_) if area.height > ReplyLine::HEIGHT + 4 => {
                let [b, r] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(ReplyLine::HEIGHT)])
                        .areas(area);
                (b, Some(r))
            }
            _ => (area, None),
        };
        let cols = self.columns();
        let n = cols.len() as u32;
        let rects = Layout::horizontal(cols.iter().map(|_| Constraint::Ratio(1, n)))
            .spacing(1)
            .split(board);
        self.col_rects.clear();
        self.card_rects.clear();
        for (ci, col) in cols.into_iter().enumerate() {
            self.col_rects.push((col, rects[ci]));
            self.draw_column(f, rects[ci], col, &ctx.store.config, now, hits);
            if self.drop_target(col) == Some(false) {
                overlays::dim_backdrop(f.buffer_mut(), rects[ci]);
            }
        }
        if let (Some(reply), Some(r)) = (&self.reply, reply_area) {
            reply.draw(f, r, hits);
        }
        if let Some(menu) = &self.menu {
            menu.draw(f, board, hits);
        }
        if let Some(ov) = &mut self.overlay {
            overlays::dim_backdrop(f.buffer_mut(), area);
            ov.view().draw(f, area, ctx, hits);
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if let Some(ov) = &mut self.overlay {
            let action = ov.view().on_key(key, ctx);
            return self.overlay_outcome(action, ctx);
        }
        if let Some(menu) = &mut self.menu {
            let pick = menu.key(key);
            return self.menu_pick(pick, ctx);
        }
        if self.cancel_drag() {
            return Action::None;
        }
        if let Some(reply) = &mut self.reply {
            return match reply.on_key(key, ctx) {
                Some(action) => {
                    self.reply = None;
                    if matches!(action, Action::Toast(_)) {
                        self.refresh(ctx);
                    }
                    action
                }
                None => Action::None,
            };
        }
        self.board_key(key, ctx)
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if let Some(ov) = &self.overlay {
            return ov.view_ref().hints();
        }
        if let Some(menu) = &self.menu {
            return menu.hints();
        }
        if self.dragging() {
            return vec![("esc", "cancel drag")];
        }
        if self.reply.is_some() {
            return vec![("⏎", "send"), ("esc", "cancel")];
        }
        let status = self.selected_task().map(|t| t.status);
        let mut h = vec![(
            "⏎",
            if status == Some(Status::Review) {
                "review"
            } else {
                "open"
            },
        )];
        match status {
            Some(Status::NeedsMe) => h.extend([("r", "reply"), ("a", "jump")]),
            Some(Status::Running) => h.push(("a", "jump")),
            Some(Status::Ready | Status::Backlog | Status::Idea) => h.push(("d", "dispatch")),
            _ => {}
        }
        h.extend([
            (".", "menu"),
            ("H/L", "move"),
            ("n", "capture"),
            ("A", "artifacts"),
            ("P", "PM"),
            ("i", "ideas"),
            ("D", "done"),
            ("?", "keys"),
            ("q", "quit"),
        ]);
        h
    }

    fn refresh(&mut self, ctx: &Ctx) {
        if let Ok(tasks) = ctx.store.list() {
            self.tasks = tasks;
        }
        self.restore_selection();
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.mouse(ev, m)
    }

    fn captures_text(&self) -> bool {
        self.overlay.is_some() || self.reply.is_some() || self.menu.is_some()
    }

    fn has_unsaved_text(&self) -> bool {
        match &self.overlay {
            Some(ov) => ov.view_ref().has_unsaved_text(),
            None => self
                .reply
                .as_ref()
                .is_some_and(|r| !r.input.text.is_empty()),
        }
    }
}
