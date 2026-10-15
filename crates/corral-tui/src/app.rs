use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_core::{AutoStage, Status};
use corral_ops::{Change, Ctx};
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::artifacts::ArtifactsView;
use crate::board::BoardView;
use crate::detail::DetailView;
use crate::history::HistoryView;
use crate::hit::{self, ClickTracker, Clock, Hit, HitMap, MouseCtx, SystemClock};
use crate::overlays::CaptureView;
use crate::overlays::{self, truncate};
use crate::review::ReviewView;
use crate::settings::SettingsView;
use crate::theme;
use crate::{Action, Mode, Toast, Tone, View};

#[cfg(test)]
mod autopilot_tests;
#[cfg(test)]
mod board_mouse_tests;
#[cfg(test)]
pub(crate) mod mouse_tests;
#[cfg(test)]
mod screen_mouse_tests;
#[cfg(test)]
mod tests;

const SYNC_EVERY: Duration = Duration::from_secs(2);
const HERDR_CHECK_EVERY: Duration = Duration::from_secs(10);
const TOAST_FOR: Duration = Duration::from_secs(4);
const MAX_TOASTS: usize = 3;
const FOOTER_BRAND: &str = " corral  ";

/// Runs the TUI until quit. Board mode: view stack starting with BoardView, periodic `corral_ops::sync` +
/// refresh every 2 s, toasts, header bar and footer hints. Capture/Artifacts modes: single view, exit on Back.
/// `mouse` turns on mouse capture (see [`crate::mouse_wanted`]).
pub fn run(ctx: Ctx, mode: Mode, mouse: bool) -> Result<()> {
    let mut app = App::new(ctx, mode)?;
    let worker = (app.mode == Mode::Board).then(|| spawn_worker(app.ctx.clone()));
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
    result
}

enum WorkerMsg {
    Synced(std::result::Result<Vec<Change>, String>),
    Herdr(bool),
}

/// Sync runs off the UI thread so slow herdr calls never freeze input; a `()` on the returned sender triggers an
/// immediate sync. The thread ends when the app drops its channel ends.
fn spawn_worker(mut ctx: Ctx) -> (Sender<()>, Receiver<WorkerMsg>) {
    let (trigger_tx, trigger_rx) = mpsc::channel::<()>();
    let (msg_tx, msg_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut herdr_checked: Option<Instant> = None;
        loop {
            // A config that fails to load keeps the previous one; the UI thread toasts the error.
            if let Ok(true) = ctx.store.reload_config_if_changed() {
                ctx = Ctx::from_store(ctx.store.clone());
            }
            if herdr_checked.is_none_or(|t| t.elapsed() >= HERDR_CHECK_EVERY) {
                herdr_checked = Some(Instant::now());
                if msg_tx
                    .send(WorkerMsg::Herdr(ctx.herdr.available()))
                    .is_err()
                {
                    return;
                }
            }
            let synced = corral_ops::sync(&ctx).map_err(|e| format!("{e:#}"));
            if msg_tx.send(WorkerMsg::Synced(synced)).is_err() {
                return;
            }
            match trigger_rx.recv_timeout(SYNC_EVERY) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    (trigger_tx, msg_rx)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    needs: usize,
    used: usize,
    limit: usize,
    review: usize,
    queued: usize,
    /// `[review] autopilot = false` (re-read from corral.toml on every reload).
    autopilot_off: bool,
}

pub(crate) struct App {
    ctx: Ctx,
    mode: Mode,
    stack: Vec<Box<dyn View>>,
    toasts: Vec<(Toast, Instant)>,
    counts: Counts,
    herdr_online: bool,
    last_sync_error: Option<String>,
    trigger: Option<Sender<()>>,
    pub(crate) quit: bool,
    pub(crate) hits: HitMap,
    clicks: ClickTracker,
    pub(crate) clock: Box<dyn Clock>,
}

impl App {
    pub(crate) fn new(ctx: Ctx, mode: Mode) -> Result<App> {
        Self::with_board(ctx, mode, BoardView::new)
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
        let mut app = App {
            ctx,
            mode,
            stack: vec![root],
            toasts: Vec::new(),
            counts: Counts::default(),
            herdr_online: true,
            last_sync_error: None,
            trigger: None,
            quit: false,
            hits: HitMap::default(),
            clicks: ClickTracker::default(),
            clock: Box::new(SystemClock),
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
            self.toasts.retain(|(_, at)| at.elapsed() < TOAST_FOR);
        }
        Ok(())
    }

    fn on_worker(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::Herdr(online) => self.herdr_online = online,
            WorkerMsg::Synced(Ok(changes)) => {
                self.last_sync_error = None;
                for c in &changes {
                    if let Some(t) = change_toast(c, self.stage_reason(c)) {
                        self.toast(t);
                    }
                }
                self.reload();
            }
            WorkerMsg::Synced(Err(e)) => {
                if self.last_sync_error.as_deref() != Some(e.as_str()) {
                    self.toast(Toast::new(Tone::Bad, "Sync failed", e.clone()));
                    self.last_sync_error = Some(e);
                }
            }
        }
    }

    /// The reason stored with the autopilot stage a change moved to (blocked / escalated), if it is still current.
    fn stage_reason(&self, c: &Change) -> Option<String> {
        let stage = c.stage?;
        let task = self.ctx.store.get(&c.task).ok()?;
        let state = task.current_run()?.autopilot.clone()?;
        (state.stage == stage).then_some(state.reason).flatten()
    }

    /// Flips `[review] autopilot` in corral.toml (comment-preserving) and syncs so the board shows the effect.
    fn toggle_autopilot(&mut self) {
        let on = !self.ctx.store.config.autopilot_enabled();
        if let Err(e) = self.ctx.store.set_autopilot(on) {
            self.toast(Toast::new(
                Tone::Bad,
                "Autopilot not changed",
                format!("{e:#}"),
            ));
            return;
        }
        self.ctx = Ctx::from_store(self.ctx.store.clone());
        self.toast(if on {
            Toast::new(
                Tone::Good,
                "Autopilot on",
                "Review tasks advance on the next sync",
            )
        } else {
            Toast::new(
                Tone::Warn,
                "Autopilot off",
                "nothing auto-advances; manual commands still work",
            )
        });
        self.apply(Action::Reload);
    }

    pub(crate) fn toast(&mut self, t: Toast) {
        self.toasts.push((t, Instant::now()));
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    fn load_counts(&mut self) {
        let Ok(tasks) = self.ctx.store.list() else {
            return;
        };
        let count = |s: Status| tasks.iter().filter(|t| t.status == s).count();
        self.counts = Counts {
            needs: count(Status::NeedsMe),
            used: tasks.iter().filter(|t| t.status.holds_slot()).count(),
            limit: self.ctx.store.config.limits.running,
            review: count(Status::Review),
            queued: tasks.iter().filter(|t| t.queued).count(),
            autopilot_off: !self.ctx.store.config.autopilot_enabled(),
        };
    }

    fn reload_config(&mut self) {
        match self.ctx.store.reload_config_if_changed() {
            Ok(false) => {}
            Ok(true) => {
                self.ctx = Ctx::from_store(self.ctx.store.clone());
                self.toast(Toast::new(
                    Tone::Info,
                    "Config reloaded",
                    self.ctx.store.config_path().display().to_string(),
                ));
            }
            Err(e) => self.toast(Toast::new(
                Tone::Bad,
                "Config not reloaded",
                format!("{e:#}"),
            )),
        }
    }

    fn reload(&mut self) {
        self.reload_config();
        for v in &mut self.stack {
            v.refresh(&self.ctx);
        }
        self.load_counts();
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        let at_board_root = self.mode == Mode::Board && self.stack.len() == 1;
        let Some(top) = self.stack.last_mut() else {
            self.quit = true;
            return;
        };
        if at_board_root && !top.captures_text() && key.code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        let action = top.on_key(key, &self.ctx);
        self.apply(action);
    }

    /// Resolves the hit under the pointer from the last frame. Key hits go through `handle_key`, so a click and
    /// its key are one code path; everything else goes to the topmost view (overlays are modal).
    pub(crate) fn handle_mouse(&mut self, ev: MouseEvent) {
        if ev.kind == MouseEventKind::Moved {
            return;
        }
        let hit = self.hits.at(ev.column, ev.row).cloned();
        let left = ev.kind == MouseEventKind::Down(MouseButton::Left);
        let double = left && self.clicks.press(self.clock.now(), ev.column, ev.row);
        match (&hit, left) {
            (Some(Hit::Key(k)), true) => return self.handle_key(*k),
            (Some(Hit::Behind(k)), true) => {
                self.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return self.handle_key(*k);
            }
            (Some(Hit::Toast(i)), true) => return self.click_toast(*i),
            (Some(Hit::Dismiss), true) => {
                if !self.stack.last().is_some_and(|v| v.has_unsaved_text()) {
                    self.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                }
                return;
            }
            (Some(Hit::Dismiss), false) | (Some(Hit::Inert), true) => return,
            _ => {}
        }
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        let m = MouseCtx {
            ctx: &self.ctx,
            hit,
            double,
        };
        let action = top.on_mouse(ev, &m);
        self.apply(action);
    }

    /// A click on a toast dismisses it; a toast about a task also opens that task (review for a Review card).
    fn click_toast(&mut self, i: usize) {
        if i >= self.toasts.len() {
            return;
        }
        let (toast, _) = self.toasts.remove(i);
        if let Some(id) = toast.task {
            let review = self
                .ctx
                .store
                .get(&id)
                .is_ok_and(|t| t.status == Status::Review);
            self.apply(if review {
                Action::OpenReview(id)
            } else {
                Action::OpenDetail(id)
            });
        }
    }

    fn push(&mut self, view: Result<Box<dyn View>>, what: &str) {
        match view {
            Ok(v) => self.stack.push(v),
            Err(e) => self.toast(Toast::new(
                Tone::Bad,
                format!("Could not open {what}"),
                format!("{e:#}"),
            )),
        }
    }

    fn pop(&mut self) {
        self.stack.pop();
        if self.stack.is_empty() {
            self.quit = true;
        } else {
            self.reload();
        }
    }

    pub(crate) fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Quit => self.quit = true,
            Action::Back => self.pop(),
            Action::BackWithToast(t) => {
                self.toast(t);
                self.pop();
            }
            Action::Toast(t) => {
                self.toast(t);
                self.reload();
            }
            Action::Reload => {
                if let Some(tx) = &self.trigger {
                    let _ = tx.send(());
                }
                self.reload();
            }
            Action::OpenDetail(id) => {
                let v = DetailView::new(&self.ctx, &id).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &id);
            }
            Action::OpenAcceptance(id) => {
                let v =
                    DetailView::acceptance(&self.ctx, &id).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &format!("acceptance of {id}"));
            }
            Action::OpenReview(id) => {
                let v = ReviewView::new(&self.ctx, &id).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &format!("review {id}"));
            }
            Action::OpenArtifacts => {
                let v = ArtifactsView::new(&self.ctx, None).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, "artifacts");
            }
            Action::OpenHistory => {
                let v = HistoryView::new(&self.ctx).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, "history");
            }
            Action::OpenSettings => self.stack.push(Box::new(SettingsView::new(&self.ctx))),
            Action::ToggleAutopilot => self.toggle_autopilot(),
            Action::Jump(id) => {
                if let Err(e) = corral_ops::jump(&self.ctx, &id) {
                    self.toast(Toast::new(
                        Tone::Bad,
                        format!("Jump to {id} failed"),
                        format!("{e:#}"),
                    ));
                }
            }
        }
    }

    pub(crate) fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.hits.clear();
        f.render_widget(
            Block::new().style(Style::new().bg(theme::BG).fg(theme::FG)),
            area,
        );
        let with_header = self.mode == Mode::Board;
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(u16::from(with_header)),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        if with_header {
            let counters = self.draw_header(f, header);
            if self.stack.len() == 1 {
                for (rect, hit) in counters {
                    self.hits.push(rect, hit);
                }
            }
        }
        let base = self
            .stack
            .iter()
            .rposition(|v| !v.is_overlay())
            .unwrap_or(0);
        for (i, view) in self.stack.iter_mut().enumerate().skip(base) {
            if i > base || view.is_overlay() {
                overlays::dim_backdrop(f.buffer_mut(), body);
            }
            view.draw(f, body, &self.ctx, &mut self.hits);
        }
        let hints = self.stack.last().map(|v| v.hints()).unwrap_or_default();
        let brand = unicode_width::UnicodeWidthStr::width(FOOTER_BRAND) as u16;
        hit::hint_hits(
            &mut self.hits,
            &hints,
            footer.x + brand,
            footer.y,
            footer.right(),
        );
        let mut spans = vec![Span::styled(FOOTER_BRAND, theme::dim())];
        spans.extend(overlays::hint_spans(&hints));
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::BG2)),
            footer,
        );
        draw_toasts(&self.toasts, f, area, &mut self.hits);
    }

    /// Draws the header and returns its click targets: each counter focuses its board column, the autopilot
    /// switch presses `O`.
    fn draw_header(&self, f: &mut Frame, area: Rect) -> Vec<(Rect, Hit)> {
        let c = self.counts;
        let root = self.ctx.store.root();
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let gap = || Span::raw("   ");
        let slots_color = if c.used >= c.limit {
            theme::ORANGE
        } else {
            theme::BLUE
        };
        let mut spans = vec![
            Span::styled(
                " corral",
                Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", theme::dim()),
            Span::styled(name, Style::new().fg(theme::FG)),
            gap(),
            Span::styled(
                format!("▲ {} needs you", c.needs),
                Style::new().fg(theme::YELLOW),
            ),
            gap(),
            Span::styled(
                format!("⟳ {}/{} slots", c.used, c.limit),
                Style::new().fg(slots_color),
            ),
            gap(),
            Span::styled(
                format!("◆ {} review", c.review),
                Style::new().fg(theme::GREEN),
            ),
            gap(),
            Span::styled(format!("⏳ {} queued", c.queued), theme::dim()),
        ];
        if !self.herdr_online {
            spans.push(gap());
            spans.push(Span::styled(
                "herdr offline",
                Style::new().fg(theme::RED).add_modifier(Modifier::BOLD),
            ));
        }
        spans.push(gap());
        let switch = spans.len();
        spans.push(if c.autopilot_off {
            Span::styled("⏸ autopilot off", Style::new().fg(theme::YELLOW))
        } else {
            Span::styled("● autopilot on", Style::new().fg(theme::GREEN))
        });
        let mut targets = Vec::new();
        let mut x = area.x;
        for (i, s) in spans.iter().enumerate() {
            let w = s.width() as u16;
            let hit = match i {
                4 => Some(Hit::Column(Status::NeedsMe)),
                6 => Some(Hit::Column(Status::Running)),
                8 => Some(Hit::Column(Status::Review)),
                i if i == switch => Some(Hit::Key(hit::press(KeyCode::Char('O')))),
                _ => None,
            };
            if let Some(hit) = hit {
                let w = w.min(area.right().saturating_sub(x));
                targets.push((Rect::new(x, area.y, w, 1), hit));
            }
            x = x.saturating_add(w);
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::BG2)),
            area,
        );
        targets
    }
}

/// Draws the toasts newest first and makes each one clickable ([`Hit::Toast`], on top of everything).
fn draw_toasts(toasts: &[(Toast, Instant)], f: &mut Frame, area: Rect, hits: &mut HitMap) {
    let w = area.width.min(48);
    if w < 12 {
        return;
    }
    let mut y = area.y + 2;
    for (i, (t, _)) in toasts.iter().enumerate().rev() {
        if y + 4 > area.bottom() {
            break;
        }
        let rect = Rect::new(area.right().saturating_sub(w + 1), y, w, 4);
        hits.push(rect, Hit::Toast(i));
        let color = tone_color(t.tone);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color))
            .style(Style::new().bg(theme::BG2));
        let inner = block.inner(rect);
        let width = inner.width as usize;
        f.render_widget(Clear, rect);
        f.render_widget(block, rect);
        let lines = vec![
            Line::styled(
                truncate(&t.title, width),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            Line::styled(truncate(&t.body, width), theme::dim()),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        y += 4;
    }
}

fn tone_color(tone: Tone) -> ratatui::style::Color {
    match tone {
        Tone::Info => theme::CYAN,
        Tone::Good => theme::GREEN,
        Tone::Warn => theme::ORANGE,
        Tone::Bad => theme::RED,
        Tone::Attention => theme::YELLOW,
    }
}

/// The toast for one sync change. Autopilot toasts are about their task (a click opens it); blocked and escalated
/// ones carry the stored `reason` of the stage.
pub(crate) fn change_toast(c: &Change, reason: Option<String>) -> Option<Toast> {
    if let Some(stage) = c.stage {
        let with_reason = |r: Option<String>| r.filter(|r| !r.trim().is_empty());
        let (tone, title, body) = match stage {
            AutoStage::Changes => (Tone::Info, format!("↺ {} sent back", c.task), None),
            AutoStage::Blocked => (
                Tone::Warn,
                format!("⛔ {} blocked", c.task),
                with_reason(reason),
            ),
            AutoStage::Reviewed => (Tone::Good, format!("✓ {} reviewed", c.task), None),
            AutoStage::Done => (
                Tone::Good,
                format!("✓ {} landed automatically", c.task),
                None,
            ),
            AutoStage::Escalated => (
                Tone::Attention,
                format!("▲ {} escalated to you", c.task),
                with_reason(reason),
            ),
            AutoStage::Gating | AutoStage::Reviewing | AutoStage::Landing => return None,
        };
        let body = body.unwrap_or_else(|| c.note.clone());
        return Some(Toast::new(tone, title, body).about(&c.task));
    }
    match c.to {
        Status::NeedsMe => Some(
            Toast::new(
                Tone::Attention,
                format!("▲ {} needs you", c.task),
                c.note.clone(),
            )
            .about(&c.task),
        ),
        Status::Review => Some(
            Toast::new(
                Tone::Good,
                format!("◆ {} ready for review", c.task),
                c.note.clone(),
            )
            .about(&c.task),
        ),
        Status::Running if !c.from.holds_slot() => Some(Toast::new(
            Tone::Info,
            format!("⇢ {} dispatched", c.task),
            c.note.clone(),
        )),
        _ => None,
    }
}
