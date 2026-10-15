//! The same real service flow with strictly separate mouse and keyboard scripts.
//! Agent completion is an external event, not a shortcut through the user-input path.

use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::time::Instant;

use corral_core::{AgentCmd, RunMode, Status, Store};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::mouse_tests::FakeClock;
use super::mouse_tests::{find_any, mouse, render};
use super::{App, WorkerMsg};

use crate::board::tests::code;

use crate::board::BoardView;
use crate::{Hit, Mode};

struct Fixture {
    root: tempfile::TempDir,
    ctx: Ctx,
    id: String,
}

fn git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "corral parity test"]);
        git(&repo, &["config", "user.email", "parity@example.invalid"]);
        std::fs::write(repo.join("README.md"), "initial\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-qm", "init"]);
        let mut store = Store::init(root.path()).unwrap();
        store.config.herdr.session = Some("corral-test".into());
        store.config.defaults.profile = Some("fake".into());
        store.config.agents.insert(
            "fake".into(),
            AgentCmd {
                command: "printf 'parity change\\n' > parity.txt".into(),
            },
        );
        store.save_config().unwrap();
        // Start with the persisted stamp so returning from detail cannot reload
        // config and replace the explicitly injected transport.
        let store = Store::open(root.path()).unwrap();
        let id = store
            .create("Input parity", Some("app"), Status::Ready, "Parity spec\n")
            .unwrap()
            .id;
        let bin = root.path().join("herdr");
        std::fs::write(&bin, include_str!("../../testdata/parity-herdr.sh")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        Self { root, ctx, id }
    }

    fn app(&self) -> App {
        let now = "2026-10-15T12:00:00Z".parse().unwrap();
        let mut app = App::with_board(self.ctx.clone(), Mode::Board, |ctx| {
            BoardView::with_clock(ctx, Box::new(move || now))
        })
        .unwrap();
        app.clock = Box::new(FakeClock(Rc::new(Cell::new(Instant::now()))));
        app
    }

    fn status(&self) -> Status {
        self.ctx.store.get(&self.id).unwrap().status
    }

    fn agent_finishes(&self, app: &mut App) {
        let task = self.ctx.store.get(&self.id).unwrap();
        assert_eq!(task.status, Status::Running);
        let run = task.current_run().unwrap();
        assert_eq!(
            (run.agent.as_str(), run.mode),
            ("fake", RunMode::Interactive)
        );
        // The fake executes the command sent through pane_run in the real dispatched worktree.
        assert_eq!(
            std::fs::read_to_string(run.worktree.join("parity.txt")).unwrap(),
            "parity change\n"
        );
        corral_ops::done(&self.ctx, &self.id, "Parity run complete").unwrap();
        app.on_worker(WorkerMsg::Synced(Ok(corral_ops::sync(&self.ctx).unwrap())));
        assert_eq!(self.status(), Status::Review);
    }

    fn assert_landed(&self, app: &mut App) {
        assert_eq!(self.status(), Status::Done);
        let text = render(app);
        let card = app
            .hits
            .iter()
            .find(|(_, hit)| *hit == Hit::Card(self.id.clone()))
            .unwrap()
            .0;
        let done = app
            .hits
            .iter()
            .find(|(_, hit)| *hit == Hit::Column(Status::Done))
            .unwrap()
            .0;
        assert!(done.contains((card.x, card.y).into()), "{text}");
        assert_eq!(
            std::fs::read_to_string(self.root.path().join("app/parity.txt")).unwrap(),
            "parity change\n"
        );
        let task = self.ctx.store.get(&self.id).unwrap();
        assert!(!task.runs[0].worktree.exists());
        assert_eq!(
            std::fs::read_to_string(self.root.path().join("closed")).unwrap(),
            "w1:p1\n"
        );
        let events = self.ctx.store.events().unwrap();
        for kind in ["dispatched", "done", "approved"] {
            assert_eq!(events.iter().filter(|e| e.kind == kind).count(), 1);
        }
    }
}

/// Only MouseEvents enter the app. Re-render between presses/releases just like its event loop.
fn click(app: &mut App, label: &str) {
    let text = render(app);
    let (x, y) = find_any(&text, label);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    render(app);
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    render(app);
}

/// Only KeyEvents enter the app.
fn press(app: &mut App, key: KeyCode) {
    app.handle_key(code(key));
    render(app);
}

#[test]
fn input_parity_mouse_only_ready_dispatch_review_approve_done() {
    // Given a real store/repo and an isolated, synchronous fake agent transport.
    let fx = Fixture::new();
    let mut app = fx.app();

    // When the whole user journey is performed with mouse events only.
    click(&mut app, "Input parity");
    assert_eq!(app.stack.len(), 1);
    click(&mut app, "Input parity");
    assert!(render(&mut app).contains("Parity spec"));
    click(&mut app, "‹ back");
    click(&mut app, "d dispatch");
    assert!(render(&mut app).contains("[fake]"));
    assert_eq!(fx.status(), Status::Ready);
    click(&mut app, "[Dispatch]");
    assert_eq!(fx.status(), Status::Running);
    click(&mut app, "dispatched");
    fx.agent_finishes(&mut app);
    click(&mut app, "Input parity");
    assert_eq!(app.stack.len(), 2);
    click(&mut app, "[Approve]");
    assert_eq!(fx.status(), Status::Review);
    click(&mut app, "[Cancel]");
    assert_eq!(fx.status(), Status::Review);
    click(&mut app, "[Approve]");
    click(&mut app, "fast-forward main");
    click(&mut app, "[Approve]");
    click(&mut app, "D done");

    // Then the card is visibly in Done, its change landed, and the real services logged it once.
    fx.assert_landed(&mut app);
    click(&mut app, "q quit");
    assert!(app.quit);
}

#[test]
fn input_parity_keyboard_only_ready_dispatch_review_approve_done() {
    // Given the same starting state. No settings screen exists on this branch.
    let fx = Fixture::new();
    let mut app = fx.app();

    // When the whole user journey is performed with key events only.
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Enter);
    assert!(render(&mut app).contains("Parity spec"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('d'));
    assert!(render(&mut app).contains("[fake]"));
    assert_eq!(fx.status(), Status::Ready);
    press(&mut app, KeyCode::Enter);
    assert_eq!(fx.status(), Status::Running);
    press(&mut app, KeyCode::F(8));
    fx.agent_finishes(&mut app);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.stack.len(), 2);
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(fx.status(), Status::Review);
    press(&mut app, KeyCode::Esc);
    assert_eq!(fx.status(), Status::Review);
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('D'));

    // Then the same persisted and rendered outcome holds.
    fx.assert_landed(&mut app);
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}
