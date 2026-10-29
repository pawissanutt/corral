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
pub(crate) use cards::CardExtra;

pub(crate) use cards::card_lines;
use slots::slot_badge;
pub use slots::slot_summary;

mod autopilot;
#[cfg(test)]
pub(crate) mod blocked_tests;
#[cfg(test)]
pub(crate) mod card_tests;
pub(crate) mod cards;
mod columns;
mod compact;
mod drop;
#[cfg(test)]
mod drop_tests;
mod empty;
mod index;
mod input;
mod menu;
mod mouse;
mod overlay;
mod view;
pub(crate) use overlay::Overlay;
mod pins;
#[cfg(test)]
mod pins_tests;
mod slots;
mod snapshot;
#[cfg(test)]
mod snapshot_tests;
pub use snapshot::BoardSnapshot;
#[cfg(test)]
mod slots_tests;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tracker_tests;
#[cfg(test)]
mod usage_refresh_tests;

const MOVE_RULE: &str = "H/L moves cards between Backlog and Ready. Dispatch, herdr state and `corral done` move tasks into \
Running, Blocked and Review; approve moves Review to Done.";

/// The kanban board (see mock "Board" scene).
pub struct BoardView {
    attention: crate::attention::Snapshot,
    routes: crate::routes::RouteCache,
    resources: crate::resources::ResourceDisplay,
    now: Box<dyn Fn() -> Timestamp>,
    tasks: Vec<Task>,
    /// Column order and Done history derived from `tasks`, `finishes` and `pinned`.
    index: index::BoardIndex,
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
    /// Submitted plans whose review automation will not start, with the refusal.
    plan_review_waits: HashMap<String, String>,
    plan_progress: HashMap<String, crate::plan::tracker::Progress>,
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
        let snapshot = BoardSnapshot::load_at(ctx, now())?;
        let mut board = BoardView {
            attention: Ok(Default::default()),
            routes: crate::routes::RouteCache::default(),
            resources: snapshot.resources.clone(),
            waits: HashMap::new(),
            automation_blocks: HashMap::new(),
            plan_review_waits: HashMap::new(),
            plan_progress: HashMap::new(),
            now,
            finishes: HashMap::new(),
            tasks: Vec::new(),
            index: index::BoardIndex::default(),
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
        board.apply_snapshot(snapshot);
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
        let cutoff = (self.now)()
            .checked_sub(self.done_window)
            .unwrap_or(Timestamp::MIN);
        self.index.done_cards(&self.tasks, cutoff, self.done_max)
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
        self.index.column(&self.tasks, col, self.ideas_only)
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
}
