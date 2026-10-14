use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_core::Status;
use corral_ops::{Change, Ctx};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::artifacts::ArtifactsView;
use crate::board::BoardView;
use crate::detail::DetailView;
use crate::overlays::{self, CaptureView, truncate};
use crate::review::ReviewView;
use crate::theme;
use crate::{Action, Mode, Toast, Tone, View};

#[cfg(test)]
mod tests;

const SYNC_EVERY: Duration = Duration::from_secs(2);
const HERDR_CHECK_EVERY: Duration = Duration::from_secs(10);
const TOAST_FOR: Duration = Duration::from_secs(4);
const MAX_TOASTS: usize = 3;

/// Runs the TUI until quit. Board mode: view stack starting with BoardView, periodic `corral_ops::sync` +
/// refresh every 2 s, toasts, header bar and footer hints. Capture/Artifacts modes: single view, exit on Back.
pub fn run(ctx: Ctx, mode: Mode) -> Result<()> {
    let mut app = App::new(ctx, mode)?;
    let worker = (app.mode == Mode::Board).then(|| spawn_worker(app.ctx.clone()));
    if let Some((tx, _)) = &worker {
        app.trigger = Some(tx.clone());
    }
    let mut terminal = ratatui::try_init().context("initializing the terminal")?;
    let result = app.event_loop(&mut terminal, worker.as_ref().map(|(_, rx)| rx));
    ratatui::restore();
    result
}

enum WorkerMsg {
    Synced(std::result::Result<Vec<Change>, String>),
    Herdr(bool),
}

/// Sync runs off the UI thread so slow herdr calls never freeze input; a `()` on the returned sender triggers an
/// immediate sync. The thread ends when the app drops its channel ends.
fn spawn_worker(ctx: Ctx) -> (Sender<()>, Receiver<WorkerMsg>) {
    let (trigger_tx, trigger_rx) = mpsc::channel::<()>();
    let (msg_tx, msg_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut herdr_checked: Option<Instant> = None;
        loop {
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
}

impl App {
    pub(crate) fn new(ctx: Ctx, mode: Mode) -> Result<App> {
        let root: Box<dyn View> = match &mode {
            Mode::Board => Box::new(BoardView::new(&ctx)?),
            Mode::Capture(cwd) => Box::new(CaptureView::new(&ctx, cwd.clone())),
            Mode::Artifacts => Box::new(ArtifactsView::new(&ctx, None)?),
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
            if event::poll(Duration::from_millis(200)).context("polling terminal events")?
                && let Event::Key(key) = event::read().context("reading terminal events")?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key);
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
                    if let Some(t) = change_toast(c) {
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
        };
    }

    fn reload(&mut self) {
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
            Action::OpenReview(id) => {
                let v = ReviewView::new(&self.ctx, &id).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, &format!("review {id}"));
            }
            Action::OpenArtifacts => {
                let v = ArtifactsView::new(&self.ctx, None).map(|v| Box::new(v) as Box<dyn View>);
                self.push(v, "artifacts");
            }
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
            self.draw_header(f, header);
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
            view.draw(f, body, &self.ctx);
        }
        let hints = self.stack.last().map(|v| v.hints()).unwrap_or_default();
        let mut spans = vec![Span::styled(" corral  ", theme::dim())];
        spans.extend(overlays::hint_spans(&hints));
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::BG2)),
            footer,
        );
        self.draw_toasts(f, area);
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
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
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::BG2)),
            area,
        );
    }

    fn draw_toasts(&self, f: &mut Frame, area: Rect) {
        let w = area.width.min(48);
        if w < 12 {
            return;
        }
        let mut y = area.y + 2;
        for (t, _) in self.toasts.iter().rev() {
            if y + 4 > area.bottom() {
                break;
            }
            let rect = Rect::new(area.right().saturating_sub(w + 1), y, w, 4);
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

fn change_toast(c: &Change) -> Option<Toast> {
    match c.to {
        Status::NeedsMe => Some(Toast::new(
            Tone::Attention,
            format!("▲ {} needs you", c.task),
            c.note.clone(),
        )),
        Status::Review => Some(Toast::new(
            Tone::Good,
            format!("◆ {} ready for review", c.task),
            c.note.clone(),
        )),
        Status::Running if !c.from.holds_slot() => Some(Toast::new(
            Tone::Info,
            format!("⇢ {} dispatched", c.task),
            c.note.clone(),
        )),
        _ => None,
    }
}
