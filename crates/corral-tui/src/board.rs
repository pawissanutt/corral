use std::collections::{HashMap, HashSet};

use anyhow::Result;
use corral_core::{Config, PinRef, PinTarget, Status, Task};
use corral_ops::{Ctx, Finish, SlotUsage};
use jiff::{SignedDuration, Timestamp};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::nav::NavKeys;
use crate::overlays::{self, CaptureView, DispatchSheet, HelpView, ReplyLine};
use crate::shelf;
use crate::theme;
use crate::{Action, Hit, HitMap, MouseCtx, Toast, Tone, View};
pub(crate) use cards::{CardExtra, card_lines};
use slots::slot_badge;
pub use slots::slot_summary;

mod autopilot;
#[cfg(test)]
pub(crate) mod card_tests;
mod cards;
mod compact;
mod drop;
#[cfg(test)]
mod drop_tests;
mod empty;
mod menu;
mod mouse;
mod overlay;
pub(crate) use overlay::Overlay;
mod pins;
#[cfg(test)]
mod pins_tests;
mod slots;
#[cfg(test)]
mod slots_tests;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tracker_tests;
#[cfg(test)]
mod usage_refresh_tests;

const MOVE_RULE: &str = "H/L moves cards between Backlog and Ready. Dispatch, herdr state and `corral done` move tasks into \
Running, Needs me and Review; approve moves Review to Done.";

/// The kanban board (see mock "Board" scene).
pub struct BoardView {
    routes: crate::routes::RouteCache,
    resources: crate::resources::ResourceDisplay,
    now: Box<dyn Fn() -> Timestamp>,
    tasks: Vec<Task>,
    /// Finish records of the Done and Dropped tasks (Done column order, window and card fields).
    finishes: HashMap<String, Finish>,
    /// Ids of tasks that are pinned or have a pinned artifact (★ on the card; kept on the board when Done).
    pinned: HashSet<String>,
    /// Keep-tier artifacts not yet marked reviewed, per Review-column task (highlighted on the card).
    unreviewed: HashMap<String, usize>,
    /// Live gate/reviewer labels, loaded during refresh rather than while drawing.
    stages: HashMap<String, String>,
    /// Ready cards' dispatch-queue reasons, sampled with `resources` on refresh.
    waits: HashMap<String, String>,
    automation_blocks: HashMap<String, String>,
    /// The shelf: every pin in order, resolved against `tasks`.
    pins: Vec<shelf::Entry>,
    /// First chip shown on the shelf strip (overflow scrolls it).
    shelf_offset: usize,
    /// The highlighted chip while the strip has focus (`g`).
    shelf_cursor: Option<usize>,
    /// `[ui] done_window` / `done_max`: which finished tasks the Done column shows.
    done_window: SignedDuration,
    done_max: usize,
    col: usize,
    row: usize,
    selected: Option<String>,
    show_done: bool,
    visible_columns: Vec<Status>,
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
    /// The terminal-engine keys the help names (back to board, leave the session).
    pub(crate) nav: NavKeys,
}

impl BoardView {
    pub fn new(ctx: &Ctx) -> Result<BoardView> {
        Self::with_clock(ctx, Box::new(Timestamp::now))
    }

    pub(crate) fn with_clock(ctx: &Ctx, now: Box<dyn Fn() -> Timestamp>) -> Result<BoardView> {
        let tasks = ctx.store.list()?;
        let (resources, waits) = crate::resources::ResourceDisplay::sample_board(ctx, &tasks);
        let mut board = BoardView {
            routes: crate::routes::RouteCache::default(),
            resources,
            waits,
            automation_blocks: HashMap::new(),
            now,
            finishes: corral_ops::finishes(&ctx.store, &tasks)?,
            tasks,
            pinned: HashSet::new(),
            unreviewed: HashMap::new(),
            stages: HashMap::new(),
            pins: Vec::new(),
            shelf_offset: 0,
            shelf_cursor: None,
            done_window: ctx.store.config.ui.done_window(),
            done_max: ctx.store.config.ui.done_max,
            col: 0,
            row: 0,
            selected: None,
            show_done: ctx.store.config.ui.columns.contains(&Status::Done),
            visible_columns: ctx.store.config.ui.columns.clone(),
            ideas_only: false,
            offsets: [0; 6],
            follow: true,
            overlay: None,
            reply: None,
            menu: None,
            drag: None,
            col_rects: Vec::new(),
            card_rects: Vec::new(),
            nav: NavKeys::default(),
        };
        board.load_pins(ctx);
        board.load_reviews(ctx);
        board.refresh_routes(ctx);
        board.select_current();
        Ok(board)
    }

    pub(crate) fn with_nav(mut self, nav: NavKeys) -> BoardView {
        self.nav = nav;
        self
    }

    fn refresh_routes(&mut self, ctx: &Ctx) {
        let tasks: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| !t.tracker)
            .filter(|t| {
                ctx.store
                    .config
                    .card_fields(t.status)
                    .contains(&corral_core::CardField::Profile)
            })
            .cloned()
            .collect();
        self.routes.refresh(ctx, &tasks);
    }

    /// True when no column shows a card, so the board explains what to do instead of showing blank columns.
    fn is_empty(&self) -> bool {
        self.columns()
            .into_iter()
            .all(|c| self.column_tasks(c).is_empty())
    }

    fn load_pins(&mut self, ctx: &Ctx) {
        self.pins = shelf::load(ctx, &self.tasks);
        self.pinned = self
            .pins
            .iter()
            .filter_map(|e| PinRef::parse(&e.pin.target).ok())
            .map(|r| r.task().to_string())
            .collect();
        if let Some(c) = &mut self.shelf_cursor {
            *c = (*c).min(self.pins.len().saturating_sub(1));
        }
        if self.pins.is_empty() {
            self.shelf_cursor = None;
        }
    }

    /// True when the task itself (not only one of its artifacts) is on the shelf.
    fn task_pinned(&self, id: &str) -> bool {
        self.pins
            .iter()
            .any(|e| matches!(&e.target, PinTarget::Task { id: t, .. } if t == id))
    }

    /// The Done column: Done tasks finished within `done_window`, newest first, at most `done_max`, plus every
    /// pinned Done task outside that window; and how many Done tasks it leaves out (the "+N older" foot, which
    /// opens history).
    fn done_cards(&self) -> (Vec<&Task>, usize) {
        let mut done: Vec<(&Task, Timestamp)> = self
            .tasks
            .iter()
            .filter(|t| t.status == Status::Done)
            .map(|t| (t, self.finishes.get(&t.id).map_or(t.updated, |f| f.at)))
            .collect();
        done.sort_by_key(|row| std::cmp::Reverse(row.1));
        let total = done.len();
        let cutoff = (self.now)()
            .checked_sub(self.done_window)
            .unwrap_or(Timestamp::MIN);
        let recent = done
            .iter()
            .take_while(|(_, at)| *at >= cutoff)
            .take(self.done_max)
            .count();
        let shown: Vec<&Task> = done
            .into_iter()
            .enumerate()
            .filter(|(i, (t, _))| *i < recent || self.pinned.contains(&t.id))
            .map(|(_, (t, _))| t)
            .collect();
        let older = total - shown.len();
        (shown, older)
    }

    fn columns(&self) -> Vec<Status> {
        Status::COLUMNS
            .into_iter()
            .filter(|s| match s {
                Status::Done => self.show_done,
                _ => self.visible_columns.contains(s),
            })
            .collect()
    }

    fn column_tasks(&self, col: Status) -> Vec<&Task> {
        if col == Status::Done {
            return self.done_cards().0;
        }
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
            Action::OpenTour => {
                self.overlay = None;
                Action::OpenTour
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
                return match self.selected_task() {
                    Some(t) if crate::plan::owns(t) => Action::OpenPlan(t.id.clone()),
                    Some(t) if t.awaiting_owner() => Action::OpenReview(t.id.clone()),
                    Some(t) => Action::OpenDetail(t.id.clone()),
                    None => Action::None,
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
                Some(t) if crate::plan::owns(t) => return Action::OpenPlan(t.id.clone()),
                Some(t) if t.status == Status::NeedsMe && !t.escalated() => {
                    self.reply = Some(ReplyLine::new(t));
                }
                _ => {
                    return Action::Toast(Toast::new(
                        Tone::Info,
                        "Nothing to reply to",
                        "r replies to Needs me cards",
                    ));
                }
            },
            KeyCode::Char('d') => match self.selected_task() {
                Some(t)
                    if !t.tracker
                        && matches!(t.status, Status::Idea | Status::Backlog | Status::Ready) =>
                {
                    self.overlay = Some(Overlay::Dispatch(Box::new(DispatchSheet::new(
                        ctx,
                        t,
                        &self.tasks,
                    ))));
                }
                _ => {
                    return Action::Toast(Toast::new(
                        Tone::Info,
                        "Not dispatchable",
                        "d dispatches non-tracker Backlog, Ready and idea cards",
                    ));
                }
            },
            KeyCode::Char('H') => return self.move_card(ctx, false),
            KeyCode::Char('L') => return self.move_card(ctx, true),
            KeyCode::Char('X') => {
                if let Some(task) = self
                    .selected_task()
                    .filter(|t| !matches!(t.status, Status::Done | Status::Dropped))
                {
                    self.overlay = Some(Overlay::Drop(drop::ConfirmDrop::new(task)));
                }
            }
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
                        corral_herdr::friendly_error(&e),
                    )),
                };
            }
            KeyCode::Char('A') => return Action::OpenArtifacts,
            KeyCode::Char('V') => return Action::OpenGallery,
            KeyCode::Char('Y') => return Action::OpenHistory,
            KeyCode::Char('/') => return Action::OpenSearch,
            KeyCode::Char('.') => self.open_menu(),
            KeyCode::Char(',') => return Action::OpenSettings,
            KeyCode::Char('O') => return Action::ToggleAutopilot,
            KeyCode::Char('?') => {
                self.overlay = Some(Overlay::Help(HelpView::new(self.nav.clone())))
            }
            KeyCode::Char('t') => return Action::OpenTour,
            KeyCode::Char('*')
            | KeyCode::Char('g')
            | KeyCode::Char('G')
            | KeyCode::Char('1'..='9') => {
                return self.pin_key(key, ctx);
            }
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
            Status::Running if config.ui.show_wip => {
                let usage = SlotUsage::from_tasks(&self.tasks, &config.limits);
                let avail = (area.width as usize).saturating_sub(title.width() + 6);
                let style = if usage.total() >= config.limits.running {
                    Style::new().fg(theme::ORANGE)
                } else {
                    theme::dim()
                };
                Span::styled(slot_badge(&usage, &config.limits, avail), style)
            }
            Status::NeedsMe if !tasks.is_empty() => {
                Span::styled(" ● ", Style::new().fg(theme::YELLOW))
            }
            _ => Span::styled(format!(" {} ", tasks.len()), theme::dim()),
        };
        let right = if config.board.card.column_cost {
            let sum = corral_core::cost_sum_label(tasks.iter().copied());
            let text = format!(" {sum} {}", right.content.trim_start());
            if area.width as usize >= title.len() + text.len() + 4 {
                Span::styled(text, right.style)
            } else {
                right
            }
        } else {
            right
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
        let older = if col == Status::Done {
            self.done_cards().1
        } else {
            0
        };
        let inner = if older > 0 && inner.height > 1 {
            let foot = Rect {
                y: inner.bottom() - 1,
                height: 1,
                ..inner
            };
            draw_older(f, foot, older, hits);
            Rect {
                height: inner.height - 1,
                ..inner
            }
        } else {
            inner
        };
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
            .map(|(i, t)| {
                let extra = CardExtra {
                    finish: self.finishes.get(&t.id),
                    pinned: self.pinned.contains(&t.id),
                    unreviewed: self.unreviewed.get(&t.id).copied().unwrap_or(0),
                    stage: self.stages.get(&t.id).map(String::as_str),
                    wait: self.waits.get(&t.id).map(String::as_str),
                    automation_block: self.automation_blocks.get(&t.id).map(String::as_str),
                    route: self.routes.get(&t.id).map(|r| &r.card),
                };
                let mut lines =
                    card_lines(t, &self.tasks, config, cw, sel_row == Some(i), now, extra);
                if !t.tracker && extra.wait.is_none() {
                    crate::resources::append_hold(
                        &mut lines,
                        t,
                        config,
                        self.resources.reason.as_deref(),
                        cw,
                    );
                }
                lines
            })
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
            if !config.ui.compact_cards
                && config.board.card.density != corral_core::Density::Compact
            {
                hits.push(
                    Rect::new(inner.right() - 2, y, 2, 1),
                    Hit::CardMenu(ids[i].clone()),
                );
            }
            self.card_rects.push((ids[i].clone(), rect));
            y += h + 1;
        }
    }
}

/// The Done column's `+N older ›` foot; a click presses `Y` (history).
fn draw_older(f: &mut Frame, foot: Rect, older: usize, hits: &mut HitMap) {
    let text = format!(" +{older} older ›");
    let w = (text.width() as u16).min(foot.width);
    f.render_widget(
        Paragraph::new(Line::styled(text, Style::new().fg(theme::CYAN))),
        foot,
    );
    hits.push(
        Rect { width: w, ..foot },
        Hit::Key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::NONE)),
    );
}

impl View for BoardView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        self.routes.poll();
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
        let board = if !self.pins.is_empty() && board.height > 4 {
            let [strip, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(board);
            shelf::draw_strip(
                f,
                strip,
                &self.pins,
                &mut self.shelf_offset,
                self.shelf_cursor,
                hits,
            );
            rest
        } else {
            board
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
        if self.is_empty() && self.overlay.is_none() {
            empty::draw(f, board, hits, &ctx.store.config.ui);
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
        if self.shelf_cursor.is_some() && self.reply.is_none() {
            return self.shelf_key(key, ctx);
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
        if self.shelf_cursor.is_some() {
            return pins::SHELF_HINTS.to_vec();
        }
        let task = self.selected_task();
        let status = task.map(|t| t.status);
        let awaiting = task.is_some_and(Task::awaiting_owner);
        // Most useful first: narrow terminals cut the tail of the footer.
        let mut h = Vec::new();
        if task.is_some() {
            h.push(("⏎", if awaiting { "review" } else { "open" }));
        }
        h.extend([("/", "search"), ("?", "help"), ("q", "leave")]);
        match status {
            Some(Status::NeedsMe) if awaiting => h.push(("a", "open agent")),
            Some(Status::NeedsMe) => h.extend([("a", "open agent"), ("r", "reply")]),
            Some(Status::Running) => h.push(("a", "open agent")),
            Some(Status::Ready | Status::Backlog | Status::Idea)
                if !task.is_some_and(|t| t.tracker) =>
            {
                h.push(("d", "dispatch"));
            }
            _ => {}
        }
        h.push(("n", "new"));
        if status.is_some_and(|s| !matches!(s, Status::Done | Status::Dropped)) {
            h.push(("X", "drop"));
        }
        let pinned = task.is_some_and(|t| self.task_pinned(&t.id));
        h.push(("*", if pinned { "unpin" } else { "pin" }));
        if !self.pins.is_empty() {
            h.push(("g", "shelf"));
        }
        h.extend([
            (".", "menu"),
            ("H/L", "move"),
            ("A", "artifacts"),
            ("V", "gallery"),
            ("Y", "history"),
            ("P", "PM"),
            ("O", "autopilot"),
            ("i", "ideas"),
            ("D", "done"),
            (",", "settings"),
        ]);
        h
    }

    fn refresh(&mut self, ctx: &Ctx) {
        if let Ok(tasks) = ctx.store.list() {
            if let Ok(finishes) = corral_ops::finishes(&ctx.store, &tasks) {
                self.finishes = finishes;
            }
            self.tasks = tasks;
        }
        (self.resources, self.waits) =
            crate::resources::ResourceDisplay::sample_board(ctx, &self.tasks);
        self.load_pins(ctx);
        self.load_reviews(ctx);
        self.refresh_routes(ctx);
        self.done_window = ctx.store.config.ui.done_window();
        self.done_max = ctx.store.config.ui.done_max;
        if self.visible_columns != ctx.store.config.ui.columns {
            self.visible_columns = ctx.store.config.ui.columns.clone();
            self.show_done = self.visible_columns.contains(&Status::Done);
        }
        self.restore_selection();
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        self.mouse(ev, m)
    }

    fn captures_text(&self) -> bool {
        self.overlay.is_some() || self.reply.is_some() || self.menu.is_some()
    }

    fn board_shortcuts(&self) -> bool {
        // The shelf consumes canonical board keys too; focus must not restore old bindings.
        !self.captures_text() && !self.dragging()
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
