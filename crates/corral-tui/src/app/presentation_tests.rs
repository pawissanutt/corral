//! A blocked write seam, not load or a race/torture fixture.
use super::*;
use ratatui::{Terminal, backend::TestBackend};

struct PendingWrite {
    started: Sender<()>,
    release: Receiver<()>,
}

impl View for PendingWrite {
    fn draw(&mut self, f: &mut Frame, area: Rect, _: &Ctx, _: &mut HitMap) {
        f.render_widget(Paragraph::new("board frame"), area);
    }
    fn on_key(&mut self, _: KeyEvent, _: &Ctx) -> Action {
        self.started.send(()).unwrap();
        self.release.recv().unwrap(); // Artificial fsync delay, released by the terminal check.
        Action::None
    }
    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }
}

#[test]
fn blocked_commit_does_not_block_terminal() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(corral_core::Store::init(root.path()).unwrap());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (painted_tx, painted_rx) = mpsc::channel();
    let (busy_tx, busy_rx) = mpsc::channel();
    let release = std::thread::spawn(move || {
        started_rx.recv().unwrap();
        busy_tx.send(()).unwrap();
        let responsive = painted_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        release_tx.send(()).unwrap();
        responsive
    });
    let mut session = presentation::Session::spawn(Rect::new(0, 0, 80, 24), move || {
        let mut app = App::new(ctx, Mode::Settings)?;
        app.stack = vec![Box::new(PendingWrite {
            started: started_tx,
            release: release_rx,
        })];
        Ok(app)
    })
    .unwrap();
    assert!(!session.input(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE
    ))));
    // The barrier thread starts the 100ms observation only after the model entered the write.
    // A second message lets this thread wait on the exact same event, without polling/sleeps.
    busy_rx.recv().unwrap();
    let start = Instant::now();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    session.receive().unwrap();
    terminal.draw(|f| session.paint(f)).unwrap();
    assert!(session.input(Event::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL
    ))));
    terminal.draw(|f| session.paint(f)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(text.contains("board frame"));
    assert!(text.contains("Saving/syncing"));
    println!(
        "blocked-write terminal redraw + quit: {:?}",
        start.elapsed()
    );
    painted_tx.send(()).ok();
    assert!(
        release.join().unwrap(),
        "no redraw/quit response within 100 ms while a write is blocked"
    );
}
