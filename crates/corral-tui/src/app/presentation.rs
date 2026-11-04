//! Terminal driver: only owned cells, input and bounded messages live on this thread.
//! The model owns every View/Store operation, including synchronous key handlers. A slow
//! commit cannot hold the terminal, its last frame, or the emergency quit path hostage.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Position};

use super::*;

const INPUT_CAPACITY: usize = 32;
const INPUT_POLL: Duration = Duration::from_millis(25);
const MODEL_TICK: Duration = Duration::from_secs(1);

struct Picture {
    cells: Buffer,
    cursor: Option<Position>,
    board_root: bool,
    quit: bool,
}

type Output = Arc<Mutex<Option<Result<Picture, String>>>>;

pub(super) struct Session {
    input: mpsc::SyncSender<Event>,
    output: Output,
    pending: Arc<AtomicUsize>,
    syncing: Arc<AtomicBool>,
    picture: Option<Picture>,
    leave: bool,
    full: bool,
    repaint: bool,
    started: Instant,
}

impl Session {
    pub(super) fn spawn(
        size: Rect,
        init: impl FnOnce() -> Result<App> + Send + 'static,
    ) -> Result<Self> {
        let (input, events) = mpsc::sync_channel(INPUT_CAPACITY);
        let output = Output::default();
        let pending = Arc::new(AtomicUsize::new(1)); // Startup is also off the terminal thread.
        let syncing = Arc::new(AtomicBool::new(false));
        let (out, work, sync) = (output.clone(), pending.clone(), syncing.clone());
        std::thread::Builder::new()
            .name("corral-board-model".into())
            .spawn(move || {
                let result = init().and_then(|app| model(app, size, events, &out, &work, sync));
                if let Err(error) = result {
                    *out.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(Err(format!("{error:#}")));
                }
            })
            .context("start board model worker")?;
        Ok(Self {
            input,
            output,
            pending,
            syncing,
            picture: None,
            leave: false,
            full: false,
            repaint: true,
            started: Instant::now(),
        })
    }

    fn busy(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0 || self.syncing.load(Ordering::Acquire)
    }

    /// Returns true for quit without consulting the model, a database, or a worker lock.
    pub(super) fn input(&mut self, event: Event) -> bool {
        self.repaint = true;
        if let Event::Key(key) = &event {
            if key.kind != KeyEventKind::Press {
                return false;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return true;
            }
            if self.leave {
                match key.code {
                    KeyCode::Enter | KeyCode::Char('q') => return true,
                    KeyCode::Esc => self.leave = false,
                    _ => {}
                }
                return false;
            }
            if self.busy()
                && key.code == KeyCode::Char('q')
                && self.picture.as_ref().is_some_and(|p| p.board_root)
            {
                self.leave = true;
                return false;
            }
        }
        if matches!(
            event,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                ..
            })
        ) {
            return false;
        }
        self.pending.fetch_add(1, Ordering::AcqRel);
        match self.input.try_send(event) {
            Ok(()) => self.full = false,
            Err(error) => {
                self.pending.fetch_sub(1, Ordering::AcqRel);
                self.full = true;
                return matches!(error, mpsc::TrySendError::Disconnected(_));
            }
        }
        false
    }

    pub(super) fn receive(&mut self) -> Result<bool> {
        // Only a frame swap happens under this lock; never wait even for that swap.
        if let Ok(mut output) = self.output.try_lock()
            && let Some(output) = output.take()
        {
            self.picture = Some(output.map_err(anyhow::Error::msg)?);
            self.repaint = true;
        }
        Ok(self.picture.as_ref().is_some_and(|p| p.quit))
    }

    pub(super) fn paint(&self, f: &mut Frame) {
        let area = f.area();
        if let Some(picture) = &self.picture {
            let visible = area.intersection(picture.cells.area);
            for y in visible.y..visible.bottom() {
                for x in visible.x..visible.right() {
                    f.buffer_mut()[(x, y)] = picture.cells[(x, y)].clone();
                }
            }
            if let Some(cursor) = picture.cursor.filter(|p| area.contains(*p)) {
                f.set_cursor_position(cursor);
            }
        }
        let notice = if self.leave {
            "Leave board? Enter quits immediately (pending saves may not finish); Esc stays"
                .to_owned()
        } else if self.full {
            "Saving/syncing · input queue full; key not queued · Ctrl-C quits".to_owned()
        } else if self.busy() {
            let spinner =
                ['◐', '◓', '◑', '◒'][(self.started.elapsed().as_millis() / 100 % 4) as usize];
            format!("{spinner} Saving/syncing · Ctrl-C quits")
        } else {
            return;
        };
        let row = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
        f.render_widget(
            Paragraph::new(notice).style(Style::new().fg(theme::YELLOW).bg(theme::BG2)),
            row,
        );
    }
}

pub(super) fn run(ctx: Ctx, mode: Mode, mouse: bool) -> Result<()> {
    let nav = NavKeys::load();
    let capture = matches!(mode, Mode::Capture(_));
    let mut terminal = ratatui::try_init().context("initializing the terminal")?;
    // This terminal capability query must precede both the input loop and model startup.
    if !capture {
        crate::artifacts::image::init_picker();
    }
    let result = (|| {
        let _guard = mouse.then(crate::term::MouseGuard::enable).transpose()?;
        let size = terminal.size().context("read terminal size")?;
        let mut session = Session::spawn(Rect::new(0, 0, size.width, size.height), move || {
            let mut app = App::with_nav(ctx, mode, nav)?;
            app.start_tour_if_new();
            Ok(app)
        })?;
        let mut drawn = Instant::now();
        loop {
            if session.receive()? {
                return Ok(());
            }
            let interval = if session.busy() {
                Duration::from_millis(50)
            } else {
                MODEL_TICK
            };
            if session.repaint || drawn.elapsed() >= interval {
                terminal
                    .draw(|f| session.paint(f))
                    .context("drawing terminal")?;
                session.repaint = false;
                drawn = Instant::now();
            }
            if event::poll(INPUT_POLL).context("polling terminal events")?
                && session.input(event::read().context("reading terminal event")?)
            {
                return Ok(());
            }
        }
    })();
    ratatui::restore();
    result
}

fn model(
    mut app: App,
    size: Rect,
    events: Receiver<Event>,
    output: &Output,
    pending: &AtomicUsize,
    syncing: Arc<AtomicBool>,
) -> Result<()> {
    let _messages = corral_ops::capture_messages(app.message_tx.clone());
    let worker = (app.mode == Mode::Board).then(|| {
        worker::spawn_worker_with_status(app.ctx.clone(), app.message_tx.clone(), syncing)
    });
    if let Some((tx, _)) = &worker {
        app.trigger = Some(tx.clone());
    }
    let mut terminal = Terminal::new(TestBackend::new(size.width, size.height))?;
    let mut dirty = true;
    let mut last_draw = Instant::now();
    let mut handled = 1;
    loop {
        if let Some((_, rx)) = &worker {
            while let Ok(message) = rx.try_recv() {
                app.on_worker(message);
                dirty = true;
            }
        }
        app.drain_messages();
        app.toasts.retain(|(_, at)| at.elapsed() < TOAST_FOR);
        if dirty || last_draw.elapsed() >= MODEL_TICK {
            let cells = terminal.draw(|f| app.draw(f))?.buffer.clone();
            let backend = terminal.backend();
            let picture = Picture {
                cells,
                cursor: backend.cursor_visible().then(|| backend.cursor_position()),
                board_root: app.mode == Mode::Board
                    && app.stack.len() == 1
                    && !app.stack.last().is_some_and(|v| v.captures_text()),
                quit: app.quit,
            };
            *output.lock().unwrap_or_else(PoisonError::into_inner) = Some(Ok(picture));
            pending.fetch_sub(handled, Ordering::AcqRel);
            handled = 0;
            dirty = false;
            last_draw = Instant::now();
        }
        if app.quit {
            break;
        }
        match events.recv_timeout(INPUT_POLL) {
            Ok(event) => {
                match event {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                    Event::Mouse(mouse) => app.handle_mouse(mouse),
                    Event::Resize(width, height) => {
                        terminal.backend_mut().resize(width, height);
                        terminal.resize(Rect::new(0, 0, width, height))?;
                    }
                    _ => {}
                }
                handled += 1;
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    // No cleanup I/O on the terminal thread, even after quit. Do not join a pending write.
    if corral_ops::artifact_server_used() {
        corral_ops::stop_server(&app.ctx)?;
    }
    Ok(())
}
