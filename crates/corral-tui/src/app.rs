use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_core::{AutoStage, Status};
use corral_ops::{Change, Ctx, Message};
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};

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
mod update;
mod worker;
#[cfg(test)]
pub(crate) use update::change_toast;
#[cfg(test)]
mod guide_tests;
mod header;
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
pub(crate) mod sync_step;
#[cfg(test)]
mod tests;
#[cfg(all(test, unix))]
mod work_budget;
#[cfg(test)]
mod work_paths;
#[cfg(test)]
mod work_probe;

const SYNC_EVERY: Duration = Duration::from_secs(2);
const HERDR_CHECK_EVERY: Duration = Duration::from_secs(10);
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
    let mut app = App::with_nav(ctx, mode, NavKeys::load())?;
    app.start_tour_if_new();
    let _messages = corral_ops::capture_messages(app.message_tx.clone());
    let worker =
        (app.mode == Mode::Board).then(|| spawn_worker(app.ctx.clone(), app.message_tx.clone()));
    if let Some((tx, _)) = &worker {
        app.trigger = Some(tx.clone());
    }
    let mut terminal = ratatui::try_init().context("initializing the terminal")?;
    if !matches!(app.mode, Mode::Capture(_)) {
        crate::artifacts::image::init_picker();
    }
    let guard = match mouse.then(crate::term::MouseGuard::enable).transpose() {
        Ok(g) => g,
        Err(e) => {
            ratatui::restore();
            return Err(e);
        }
    };
    let result = app.event_loop(&mut terminal, worker.as_ref().map(|(_, rx)| rx));
    drop(guard);
    ratatui::restore();
    if corral_ops::artifact_server_used()
        && let Err(e) = corral_ops::stop_server(&app.ctx)
    {
        eprintln!("corral: could not stop the artifact server: {e:#}");
    }
    result
}

use worker::{WorkerMsg, spawn_worker};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    needs: usize,
    used: usize,
    limit: usize,
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
    herdr_online: bool,
    last_sync_error: Option<String>,
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
            herdr_online: true,
            last_sync_error: None,
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

    fn event_loop(
        &mut self,
        terminal: &mut DefaultTerminal,
        worker: Option<&Receiver<WorkerMsg>>,
    ) -> Result<()> {
        while !self.quit {
            terminal
                .draw(|f| self.draw(f))
                .context("drawing the terminal")?;
            if event::poll(Duration::from_millis(200)).context("polling terminal events")? {
                match event::read().context("reading terminal events")? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
                    Event::Mouse(ev) => self.handle_mouse(ev),
                    _ => {}
                }
            }
            if let Some(rx) = worker {
                while let Ok(msg) = rx.try_recv() {
                    self.on_worker(msg);
                }
            }
            self.drain_messages();
            self.toasts.retain(|(_, at)| at.elapsed() < TOAST_FOR);
        }
        Ok(())
    }
}
