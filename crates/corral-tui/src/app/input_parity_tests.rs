//! The same real service flow with strictly separate mouse and keyboard scripts.
//! Agent completion is an external event, not a shortcut through the user-input path.

use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use corral_core::{Accept, AutoStage, CheckpointUpdate, Harness, Profile, RunMode, Status, Store};
use corral_herdr::Herdr;
use corral_ops::{Ctx, HarnessInfo, HarnessReport, ModelSource};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use unicode_width::UnicodeWidthStr;

use super::mouse_tests::FakeClock;
use super::mouse_tests::{mouse, render};
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
    let out = crate::board::tests::fixture_command("git", path)
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
        store.config.pm.agent = "fake".into();
        store.config.agents.clear();
        store.config.harnesses.clear();
        store.config.profiles.clear();
        store.config.harnesses.insert(
            "fake".into(),
            Harness {
                command: "sh -c 'printf \"parity change\\n\" > parity.txt' parity {model_args}"
                    .into(),
                flags: None,
                model_flag: Some("--model".into()),
                effort_flag: Some("--effort".into()),
                efforts: vec!["low".into(), "high".into()],
                prompt: None,
            },
        );
        for (name, model, effort) in [("fake", "model-one", "low"), ("spare", "model-two", "high")]
        {
            store.config.profiles.insert(
                name.into(),
                Profile {
                    harness: "fake".into(),
                    model: Some(model.into()),
                    vendor: None,
                    escalate_to: None,
                    effort: Some(effort.into()),
                    args: Vec::new(),
                },
            );
        }
        store.config.review.accept = Some(Accept::Auto);
        store.save_config().unwrap();
        // Start with the persisted stamp so returning from detail cannot reload
        // config and replace the explicitly injected transport.
        let store = Store::open(root.path()).unwrap();
        let id = store
            .create("Input parity", Some("app"), Status::Ready, "Parity spec\n")
            .unwrap()
            .id;
        store
            .update(&id, |task| {
                task.review = Some(Accept::Owner);
                Ok(())
            })
            .unwrap();
        let report = HarnessReport {
            detected_at: "2026-10-15T12:00:00Z".parse().unwrap(),
            harnesses: vec![HarnessInfo {
                key: "fake".into(),
                program: Some("sh".into()),
                path: Some("/bin/sh".into()),
                version: Some("fixture".into()),
                models: vec!["model-one".into(), "model-two".into()],
                model_source: ModelSource::Listed,
                efforts: vec!["low".into(), "high".into()],
                takes_effort: true,
                notes: Vec::new(),
            }],
        };
        std::fs::write(
            store.state_dir().join(corral_ops::HARNESS_CACHE_FILE),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
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

    fn agent_finishes(&self, app: &mut App, profile: &str, model: &str, effort: &str) {
        let task = self.ctx.store.get(&self.id).unwrap();
        assert_eq!(task.status, Status::Running);
        let run = task.current_run().unwrap();
        assert_eq!(
            (
                run.agent.as_str(),
                run.model.as_deref(),
                run.effort.as_deref(),
                run.mode
            ),
            (profile, Some(model), Some(effort), RunMode::Interactive)
        );
        // The fake executes the command sent through pane_run in the real dispatched worktree.
        assert_eq!(
            std::fs::read_to_string(run.worktree.join("parity.txt")).unwrap(),
            "parity change\n"
        );
        git(&run.worktree, &["add", "parity.txt"]);
        git(&run.worktree, &["commit", "-qm", "Record fixture progress"]);
        let state = corral_ops::progress_readback(&self.ctx, &self.id).unwrap();
        corral_ops::save_checkpoint(
            &self.ctx,
            &self.id,
            &CheckpointUpdate {
                task: self.id.clone(),
                run: state.run,
                spec_revision: state.spec_revision,
                expected_revision: state
                    .checkpoints
                    .last()
                    .map_or(0, |c| c.checkpoint.revision),
                progress_revision: 0,
                head: state.head,
                base: state.base,
                tree: state.tree,
                criterion_ids: Vec::new(),
                evidence_refs: Vec::new(),
                next_action: "Complete the input parity lifecycle".into(),
                decisions: Vec::new(),
                waits: Vec::new(),
                resources: Vec::new(),
            },
        )
        .unwrap();
        corral_ops::done(&self.ctx, &self.id, "Parity run complete").unwrap();
        app.on_worker(WorkerMsg::Synced(Ok(corral_ops::sync(&self.ctx).unwrap())));
        assert_eq!(self.status(), Status::NeedsMe);
        assert_eq!(
            self.ctx
                .store
                .get(&self.id)
                .unwrap()
                .current_run()
                .unwrap()
                .autopilot
                .as_ref()
                .unwrap()
                .stage,
            AutoStage::Escalated
        );
        assert!(render(app).contains("autopilot:"));
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
        assert_eq!(task.runs.len(), 2);
        assert!(!task.runs[1].worktree.exists());
        assert_eq!(
            std::fs::read_to_string(self.root.path().join("closed")).unwrap(),
            "w1:p1\n"
        );
        let events = self.ctx.store.events().unwrap();
        for (kind, count) in [("dispatched", 2), ("done", 2), ("approved", 1)] {
            assert_eq!(events.iter().filter(|e| e.kind == kind).count(), count);
        }
    }
}

/// Only MouseEvents enter the app. Re-render between presses/releases just like its event loop.
fn click(app: &mut App, label: &str) {
    let text = render(app);
    let (x, y) = text
        .lines()
        .enumerate()
        .find_map(|(y, line)| {
            line.match_indices(label).find_map(|(byte, _)| {
                let x = u16::try_from(line[..byte].width()).unwrap();
                let y = u16::try_from(y).unwrap();
                match app.hits.at(x, y) {
                    Some(Hit::Dismiss | Hit::Inert) => None,
                    _ => Some((x, y)),
                }
            })
        })
        .unwrap_or_else(|| panic!("no active target for {label:?}\n{text}"));
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
    click(&mut app, ", settings");
    for section in ["Profiles", "Harnesses", "Projects", "Artifacts", "General"] {
        click(&mut app, section);
    }
    click(&mut app, "‹ back");
    click(&mut app, "O autopilot");
    let config = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(!config.autopilot_enabled());
    click(&mut app, "O autopilot");
    let config = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(config.autopilot_enabled());
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    click(&mut app, "Input parity");
    assert_eq!(app.stack.len(), 1);
    click(&mut app, "Input parity");
    assert!(render(&mut app).contains("Parity spec"));
    click(&mut app, "‹ back");
    click(&mut app, "d dispatch");
    assert!(render(&mut app).contains("[fake]"));
    assert_eq!(fx.status(), Status::Ready);
    click(&mut app, " spare ");
    click(&mut app, " fake ");
    click(&mut app, " high ");
    click(&mut app, "M model");
    click(&mut app, "model-two");
    click(&mut app, "model-two");
    click(&mut app, "p save as profile");
    assert!(render(&mut app).contains("fake-high"));
    click(&mut app, "⏎ save profile");
    let config = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(config.profiles.contains_key("fake-high"));
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    click(&mut app, "[Dispatch]");
    assert_eq!(fx.status(), Status::Running);
    click(&mut app, "dispatched");
    fx.agent_finishes(&mut app, "fake-high", "model-two", "high");
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    // The autopilot toast opens review, just as the badge/card does.
    assert_eq!(app.stack.len(), 2);
    click(&mut app, "×");
    click(&mut app, "[Retry]");
    click(&mut app, "esc cancel");
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().runs.len(), 1);
    click(&mut app, "[Retry]");
    click(&mut app, " spare ");
    click(&mut app, " low ");
    click(&mut app, "M model");
    click(&mut app, "model-one");
    click(&mut app, "model-one");
    click(&mut app, "⏎ retry");
    assert_eq!(fx.status(), Status::Running, "{}", render(&mut app));
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    fx.agent_finishes(&mut app, "spare", "model-one", "low");
    while !app.toasts.is_empty() {
        click(&mut app, " F8 ");
    }
    assert_eq!(app.stack.len(), 2);
    click(&mut app, "×");
    click(&mut app, "[Approve]");
    assert_eq!(fx.status(), Status::NeedsMe);
    click(&mut app, "[Cancel]");
    assert_eq!(fx.status(), Status::NeedsMe);
    click(&mut app, "[Approve]");
    click(&mut app, "fast-forward main");
    click(&mut app, "[Approve]");

    // Then the card is visibly in Done, its change landed, and the real services logged it once.
    fx.assert_landed(&mut app);
    click(&mut app, "q leave");
    assert!(!app.quit, "q first explains that agents keep running");
    click(&mut app, "[Close board]");
    assert!(app.quit);
}

#[test]
fn input_parity_keyboard_only_ready_dispatch_review_approve_done() {
    // Given the same starting state, including deterministic discovery and owner review policy.
    let fx = Fixture::new();
    let mut app = fx.app();

    // When the whole user journey is performed with key events only.
    press(&mut app, KeyCode::Char(','));
    for _ in 0..5 {
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('O'));
    let config = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(!config.autopilot_enabled());
    press(&mut app, KeyCode::Char('O'));
    let config = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(config.autopilot_enabled());
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Enter);
    assert!(render(&mut app).contains("Parity spec"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('d'));
    assert!(render(&mut app).contains("[fake]"));
    assert_eq!(fx.status(), Status::Ready);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Char('e'));
    press(&mut app, KeyCode::Char('M'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('p'));
    assert!(render(&mut app).contains("fake-high"));
    press(&mut app, KeyCode::Enter);
    let config = corral_core::Config::load(&fx.ctx.store.config_path()).unwrap();
    assert!(config.profiles.contains_key("fake-high"));
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(fx.status(), Status::Running);
    press(&mut app, KeyCode::F(8));
    fx.agent_finishes(&mut app, "fake-high", "model-two", "high");
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    assert_eq!(app.stack.len(), 2);
    press(&mut app, KeyCode::Char('R'));
    press(&mut app, KeyCode::Esc);
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().runs.len(), 1);
    press(&mut app, KeyCode::Char('R'));
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Char('e'));
    press(&mut app, KeyCode::Char('M'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    assert_eq!(fx.status(), Status::Running, "{}", render(&mut app));
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    fx.agent_finishes(&mut app, "spare", "model-one", "low");
    while !app.toasts.is_empty() {
        press(&mut app, KeyCode::F(8));
    }
    assert_eq!(app.stack.len(), 2);
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(fx.status(), Status::NeedsMe);
    press(&mut app, KeyCode::Esc);
    assert_eq!(fx.status(), Status::NeedsMe);
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Enter);

    // Then the same persisted and rendered outcome holds.
    fx.assert_landed(&mut app);
    press(&mut app, KeyCode::Char('q'));
    assert!(!app.quit, "q first explains that agents keep running");
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}
