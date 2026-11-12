use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_core::{AutoStage, Status};
use corral_ops::{Change, Ctx, Message};
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::artifacts::ArtifactsView;
use crate::board::BoardView;
use crate::detail::DetailView;
use crate::guide::{self, LeaveView, TourView};
use crate::history::HistoryView;
use crate::hit::{ClickTracker, Clock, Hit, HitMap, MouseCtx, SystemClock};
use crate::nav::NavKeys;
use crate::overlays::CaptureView;
use crate::overlays::{self, truncate};
use crate::review::ReviewView;
use crate::search::SearchView;
use crate::settings::SettingsView;
use crate::theme;
use crate::{Action, Mode, Toast, Tone, View};
use footer::Footer;

#[cfg(test)]
mod autopilot_parity_tests;
#[cfg(test)]
mod autopilot_tests;
#[cfg(test)]
mod board_mouse_tests;
#[cfg(test)]
mod comments_tests;
// The scripted agent transport is a per-test Unix executable.
mod draw;
mod footer;
mod input;
mod presentation;
#[cfg(test)]
mod presentation_tests;
mod update;
mod worker;
#[cfg(test)]
pub(crate) use update::change_toast;
#[cfg(test)]
mod guide_tests;
mod header;
#[cfg(all(test, unix))]
mod hot_budget;
#[cfg(all(test, unix))]
mod hot_budget_checks;
#[cfg(all(test, unix))]
mod hot_budget_fixture;
#[cfg(all(test, unix))]
mod hot_budget_rows;
#[cfg(all(test, unix))]
mod input_parity_tests;
#[cfg(test)]
mod latency_tests;
#[cfg(test)]
mod live_open_tests;
mod messages;
#[cfg(test)]
mod messages_tests;
#[cfg(test)]
pub(crate) mod mouse_tests;
pub(crate) mod operating;
#[cfg(test)]
mod parity_tests;
#[cfg(test)]
mod pins_tests;
#[cfg(test)]
mod plan_form_tests;
#[cfg(test)]
mod plan_review_live_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod preferences_tests;
#[cfg(test)]
mod screen_mouse_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod settings_parity_tests;
#[cfg(test)]
mod slot_tests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod sync_budget;
pub(crate) mod sync_step;
#[cfg(test)]
mod tests;
#[cfg(all(test, unix))]
mod work_budget;
#[cfg(test)]
mod work_paths;
#[cfg(test)]
mod work_probe;

const HERDR_CHECK_EVERY: Duration = Duration::from_secs(10);
/// With work waiting, this long without a completed sync pass is a stall the header reports.
const SYNC_STALL_AFTER: Duration = Duration::from_secs(5 * 60);
const TOAST_FOR: Duration = Duration::from_secs(4);
const MAX_TOASTS: usize = 3;

/// Opens the TUI's validated context before terminal or worker startup.
pub fn open(root: &std::path::Path) -> Result<Ctx> {
    let store = corral_core::Store::open_with(root, |store| {
        store.set_attribution(corral_core::Actor::Owner, "tui")
    })?;
    Ok(Ctx::from_store(store))
}

/// Runs the TUI until quit. Board mode: view stack starting with BoardView, periodic `corral_ops::sync` +
/// refresh every 2 s, toasts, header bar and footer hints. Capture/Artifacts modes: single view, exit on Back.
/// `mouse` turns on mouse capture (see [`crate::mouse_wanted`]).
/// `ctx` comes from [`open`], which establishes attribution and validates startup configuration.
pub fn run(ctx: Ctx, mode: Mode, mouse: bool) -> Result<()> {
    presentation::run(ctx, mode, mouse)
}

use worker::WorkerMsg;
#[cfg(test)]
use worker::spawn_worker;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Counts {
    slots: corral_ops::SlotUsage,
    needs: usize,
    used: usize,
    limit: usize,
    wip: usize,
    wip_limit: usize,
    review: usize,
    /// Ready is the dispatch queue: authorized work waiting for admission.
    ready: usize,
    tasks: usize,
    reviews: usize,
    gates: usize,
    reservations: usize,
}

pub(crate) struct App {
    resources: crate::resources::ResourceDisplay,
    ctx: Ctx,
    mode: Mode,
    stack: Vec<Box<dyn View>>,
    toasts: Vec<(Toast, Instant)>,
    counts: Counts,
    operating_header: String,
    inbox_open: Option<usize>,
    herdr_online: bool,
    last_sync_error: Option<String>,
    /// When the last sync pass completed (or the app started), by [`Self::clock`].
    synced_at: Instant,
    messages: messages::MessageLog,
    message_tx: Sender<Message>,
    message_rx: Receiver<Message>,
    trigger: Option<Sender<()>>,
    pub(crate) quit: bool,
    pub(crate) hits: HitMap,
    clicks: ClickTracker,
    pub(crate) clock: Box<dyn Clock>,
    /// Terminal-engine keys named in the footer, tour and leave popup.
    pub(crate) nav: NavKeys,
}

impl App {
    #[cfg(test)]
    pub(crate) fn new(ctx: Ctx, mode: Mode) -> Result<App> {
        Self::with_nav(ctx, mode, NavKeys::default())
    }

    pub(crate) fn with_nav(ctx: Ctx, mode: Mode, nav: NavKeys) -> Result<App> {
        let board_nav = nav.clone();
        let mut app = Self::with_board(ctx, mode, |c| Ok(BoardView::new(c)?.with_nav(board_nav)))?;
        app.nav = nav;
        Ok(app)
    }

    /// Opens the welcome tour on the board unless this workspace has finished or skipped it.
    pub(crate) fn start_tour_if_new(&mut self) {
        if self.mode == Mode::Board && self.stack.len() == 1 && !guide::tour_seen(&self.ctx) {
            self.apply(Action::OpenTour);
        }
    }

    pub(crate) fn with_board(
        ctx: Ctx,
        mode: Mode,
        board: impl FnOnce(&Ctx) -> Result<BoardView>,
    ) -> Result<App> {
        let root: Box<dyn View> = match &mode {
            Mode::Board => Box::new(board(&ctx)?),
            Mode::Capture(cwd) => Box::new(CaptureView::new(&ctx, cwd.clone())),
            Mode::Artifacts => Box::new(ArtifactsView::new(&ctx, None)?),
            Mode::Settings => Box::new(SettingsView::new(&ctx)),
        };
        let (message_tx, message_rx) = mpsc::channel();
        let mut app = App {
            resources: crate::resources::ResourceDisplay::sample(&ctx),
            ctx,
            mode,
            stack: vec![root],
            toasts: Vec::new(),
            counts: Counts::default(),
            operating_header: String::new(),
            inbox_open: None,
            herdr_online: true,
            last_sync_error: None,
            synced_at: Instant::now(),
            messages: messages::MessageLog::default(),
            message_tx,
            message_rx,
            trigger: None,
            quit: false,
            hits: HitMap::default(),
            clicks: ClickTracker::default(),
            clock: Box::new(SystemClock),
            nav: NavKeys::default(),
        };
        app.load_counts();
        Ok(app)
    }
}
