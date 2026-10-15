use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use corral_core::{Artifact, ArtifactKind, Run, RunMode, Status, Store};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use jiff::Timestamp;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{BoardView, Overlay};
use crate::{Action, Mode, View};

static SEQ: AtomicUsize = AtomicUsize::new(0);

const START: Timestamp = Timestamp::constant(1_767_268_800, 0); // 2026-01-01T12:00:00Z

pub(crate) struct Fixture {
    pub ctx: Ctx,
    pub now: Rc<Cell<Timestamp>>,
    root: PathBuf,
}

impl Fixture {
    pub fn board(&self) -> BoardView {
        let now = self.now.clone();
        BoardView::with_clock(&self.ctx, Box::new(move || now.get())).unwrap()
    }

    pub fn app(&self, mode: Mode) -> crate::app::App {
        crate::app::App::with_board(self.ctx.clone(), mode, |_| Ok(self.board())).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn run(id: &str, agent: &str) -> Run {
    Run {
        id: id.into(),
        agent: agent.into(),
        harness: None,
        model: None,
        effort: None,
        mode: RunMode::Interactive,
        worktree: PathBuf::from("/tmp/wt"),
        branch: "corral/x".into(),
        base: "main".into(),
        base_sha: "0".repeat(40),
        herdr: None,
        started: START,
        ended: None,
        outcome: None,
        summary: None,
        blocked_excerpt: None,
        agent_state: Some("working".into()),
        seen_working: true,
        gate: None,
        idle_since: None,
        idle_no_report: false,
        review: None,
        usage: None,
        usage_note: None,
        autopilot: None,
    }
}

fn artifact(kind: ArtifactKind, title: &str) -> Artifact {
    Artifact {
        run: "R1".into(),
        kind,
        title: title.into(),
        path: "artifacts/x".into(),
        source: "/tmp/x".into(),
        sha256: "0".repeat(64),
        bytes: 1,
        added: START,
    }
}

/// T-001 idea, T-002 backlog, T-003 ready, T-004 running, T-005 needs me, T-006 review, T-007 done.
pub(crate) fn seeded() -> Fixture {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("corral-tui-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let store = Store::init(&root).unwrap();
    let mk = |title: &str, project: &str, status: Status| {
        store
            .create(title, Some(project), status, "## Goal\nDo it.\n")
            .unwrap()
            .id
    };
    mk("Arrival copy pass", "mitiplay", Status::Idea);
    mk("Artifact finder fuzzy ranking", "corral", Status::Backlog);
    let ready = mk("Bench seed cache", "mitigraph", Status::Ready);
    store
        .update(&ready, |t| {
            t.profile = Some("omo".into());
            Ok(())
        })
        .unwrap();
    let running = mk("Witness shard cache", "mitiplay", Status::Ready);
    store
        .update(&running, |t| {
            t.status = Status::Running;
            t.runs.push(run("R1", "omo"));
            Ok(())
        })
        .unwrap();
    let needs = mk("OCR consent flow", "mitimind-lite", Status::Ready);
    store
        .update(&needs, |t| {
            t.status = Status::NeedsMe;
            let mut r = run("R1", "omp");
            r.agent_state = Some("blocked".into());
            r.blocked_excerpt = Some(
                "● Read credentials.rs\n\nKeep the 0600 fallback file, or the OS keyring?\n".into(),
            );
            t.runs.push(r);
            Ok(())
        })
        .unwrap();
    let review = mk("Temporal recall fallback", "mitigraph", Status::Ready);
    store
        .update(&review, |t| {
            t.status = Status::Review;
            t.runs.push(run("R1", "claude"));
            t.artifacts
                .push(artifact(ArtifactKind::Diff, "Final diff +412 −88"));
            t.artifacts.push(artifact(ArtifactKind::Summary, "Summary"));
            Ok(())
        })
        .unwrap();
    let done = mk("Event-log seed guard", "mitigraph", Status::Ready);
    store
        .update(&done, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    // Store writes stamp `updated` with wall time. Serialize the final fixture directly.
    for mut task in store.list().unwrap() {
        task.created = START;
        task.updated = START;
        std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();
    }
    let ctx = Ctx {
        store,
        herdr: Herdr::new(Some("corral-test".into())),
    };
    Fixture {
        ctx,
        now: Rc::new(Cell::new(START + jiff::SignedDuration::from_secs(310))),
        root,
    }
}

pub(crate) fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

pub(crate) fn code(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(crate) fn buffer_text(buf: &Buffer) -> String {
    let mut s = String::new();
    for y in 0..buf.area.height {
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
            s.push_str(sym);
        }
        s.push('\n');
    }
    s
}

fn render(board: &mut BoardView, ctx: &Ctx) -> String {
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    term.draw(|f| board.draw(f, f.area(), ctx, &mut crate::HitMap::default()))
        .unwrap();
    buffer_text(term.backend().buffer())
}

#[test]
fn board_ages_follow_the_injected_clock() {
    // Given a board whose run and waiting ages are still displayed in seconds.
    let fx = seeded();
    fx.now.set(START + jiff::SignedDuration::from_secs(10));
    let mut board = fx.board();
    let before = render(&mut board, &fx.ctx);
    assert!(before.contains("⟳ 10s"), "{before}");
    assert!(before.contains("waiting 10s"), "{before}");

    // When one second passes on the injected clock.
    fx.now
        .set(fx.now.get() + jiff::SignedDuration::from_secs(1));
    let after = render(&mut board, &fx.ctx);

    // Then both age fields advance without consulting the wall clock.
    assert!(after.contains("⟳ 11s"), "{after}");
    assert!(after.contains("waiting 11s"), "{after}");
    assert_eq!(
        before.replace("10s", "11s"),
        after,
        "only the displayed ages change"
    );
}

#[test]
fn board_renders_columns_cards_and_meta() {
    let fx = seeded();
    let mut b = fx.board();
    let text = render(&mut b, &fx.ctx);
    assert!(text.contains("╭ Backlog ─"), "focused column title\n{text}");
    assert!(text.contains("1/3 ╮"), "running slot count\n{text}");
    for title in ["Backlog", "Ready", "Running", "Needs me", "Review"] {
        assert!(text.contains(title), "missing column {title}\n{text}");
    }
    for id in ["T-001", "T-002", "T-003", "T-004", "T-005", "T-006"] {
        assert!(text.contains(id), "missing card {id}\n{text}");
    }
    assert!(!text.contains("T-007"), "done hidden by default");
    assert!(text.contains("◌ idea"));
    assert!(text.contains("⇢ omo"));
    assert!(
        text.contains("\"Keep the 0600") && text.contains("keyring?\""),
        "needs-me question wraps\n{text}"
    );
    assert!(
        text.contains("+412 −88") && text.contains("◆ 2 art"),
        "review meta\n{text}"
    );
    b.on_key(key('D'), &fx.ctx);
    let text = render(&mut b, &fx.ctx);
    assert!(text.contains("Done") && text.contains("T-007") && text.contains('✓'));
}

#[test]
fn board_shows_idle_no_report_badge_on_running_card() {
    let fx = seeded();
    let mut b = fx.board();
    assert!(!render(&mut b, &fx.ctx).contains("idle, no report"));
    fx.ctx
        .store
        .update("T-004", |t| {
            if let Some(r) = t.current_run_mut() {
                r.agent_state = Some("idle".into());
                r.idle_no_report = true;
            }
            Ok(())
        })
        .unwrap();
    let mut b = fx.board();
    let text = render(&mut b, &fx.ctx);
    let line = text
        .lines()
        .find(|l| l.contains("⚠ idle, no report"))
        .unwrap_or_else(|| panic!("badge missing\n{text}"));
    assert!(
        line.contains("omo"),
        "badge on the running omo card\n{text}"
    );
}

#[test]
fn board_cards_show_harness_model_effort_of_the_run() {
    let fx = seeded();
    fx.ctx
        .store
        .update("T-006", |t| {
            if let Some(r) = t.current_run_mut() {
                r.harness = Some("claude".into());
                r.model = Some("opus".into());
                r.effort = Some("high".into());
            }
            Ok(())
        })
        .unwrap();
    let mut b = BoardView::new(&fx.ctx).unwrap();
    let text = render(&mut b, &fx.ctx);
    assert!(text.contains("claude·opus·high"), "{text}");
}

#[test]
fn board_navigation_and_open_actions() {
    let fx = seeded();
    let ctx = &fx.ctx;
    let mut b = fx.board();
    assert_eq!(b.selected_task().map(|t| t.id.as_str()), Some("T-001"));
    b.on_key(key('j'), ctx);
    assert_eq!(b.selected_task().map(|t| t.id.as_str()), Some("T-002"));
    b.on_key(key('l'), ctx);
    assert_eq!(
        b.on_key(code(KeyCode::Enter), ctx),
        Action::OpenDetail("T-003".into())
    );
    b.on_key(key('l'), ctx);
    assert_eq!(b.on_key(key('a'), ctx), Action::Jump("T-004".into()));
    assert!(
        matches!(b.on_key(key('L'), ctx), Action::Toast(_)),
        "running is not a manual move"
    );
    b.on_key(code(KeyCode::Right), ctx);
    b.on_key(key('l'), ctx);
    assert_eq!(
        b.on_key(code(KeyCode::Enter), ctx),
        Action::OpenReview("T-006".into())
    );
    assert_eq!(b.on_key(key('A'), ctx), Action::OpenArtifacts);
    b.on_key(key('i'), ctx);
    b.on_key(key('h'), ctx);
    b.on_key(key('h'), ctx);
    b.on_key(key('h'), ctx);
    b.on_key(key('h'), ctx);
    b.on_key(key('j'), ctx);
    assert_eq!(
        b.selected_task().map(|t| t.id.as_str()),
        Some("T-001"),
        "ideas-only filter hides T-002"
    );
}

fn autopilot(stage: corral_core::AutoStage, reason: Option<&str>) -> corral_core::AutopilotState {
    corral_core::AutopilotState {
        stage,
        reason: reason.map(str::to_string),
        sha: None,
        at: Timestamp::now(),
    }
}

#[test]
fn board_shows_autopilot_state_and_reviews_escalated_cards() {
    use corral_core::{AutoStage, RunOutcome};
    let fx = seeded();
    let ctx = &fx.ctx;
    ctx.store
        .update("T-006", |t| {
            if let Some(r) = t.current_run_mut() {
                r.outcome = Some(RunOutcome::Done);
                r.autopilot = Some(autopilot(AutoStage::Reviewed, None));
            }
            Ok(())
        })
        .unwrap();
    ctx.store
        .update("T-005", |t| {
            if let Some(r) = t.current_run_mut() {
                r.outcome = Some(RunOutcome::Done);
                r.autopilot = Some(autopilot(
                    AutoStage::Escalated,
                    Some("the change touches owner_paths: key.rs"),
                ));
            }
            Ok(())
        })
        .unwrap();
    let mut b = BoardView::new(ctx).unwrap();
    let text = render(&mut b, ctx);
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.contains("reviewed ✓"))
        .unwrap_or_else(|| panic!("reviewed badge missing\n{text}"));
    assert!(
        lines[at - 1].contains("claude · +412 −88 · ◆ 2 art"),
        "badge under the review card's meta line\n{text}"
    );
    assert!(
        text.contains("\"autopilot: the change") && !text.contains("Keep the 0600"),
        "escalation reason replaces the pane excerpt\n{text}"
    );
    for _ in 0..3 {
        b.on_key(key('l'), ctx);
    }
    assert_eq!(b.hints()[0], ("⏎", "review"));
    assert_eq!(
        b.on_key(code(KeyCode::Enter), ctx),
        Action::OpenReview("T-005".into())
    );
}

#[test]
fn board_reply_line_opens_for_needs_me() {
    let fx = seeded();
    let ctx = &fx.ctx;
    let mut b = fx.board();
    assert!(matches!(b.on_key(key('r'), ctx), Action::Toast(_)));
    for _ in 0..3 {
        b.on_key(key('l'), ctx);
    }
    b.on_key(key('r'), ctx);
    assert!(b.reply.is_some() && b.captures_text());
    for c in "keyring".chars() {
        b.on_key(key(c), ctx);
    }
    let text = render(&mut b, ctx);
    assert!(
        text.contains("Reply → T-005") && text.contains("reply › keyring"),
        "{text}"
    );
    assert_eq!(b.hints()[0], ("⏎", "send"));
    b.on_key(code(KeyCode::Esc), ctx);
    assert!(b.reply.is_none());
}

#[test]
fn board_dispatch_sheet_and_help_overlays() {
    let fx = seeded();
    let ctx = &fx.ctx;
    let mut b = fx.board();
    b.on_key(key('l'), ctx);
    b.on_key(key('d'), ctx);
    let Some(Overlay::Dispatch(sheet)) = &b.overlay else {
        panic!("dispatch sheet expected")
    };
    assert_eq!(sheet.task_id, "T-003");
    assert_eq!(Some(sheet.choice.profile()), Some("omo"));
    let text = render(&mut b, ctx);
    assert!(
        text.contains("dispatch T-003")
            && text.contains("Do it.")
            && text.contains("[interactive]"),
        "{text}"
    );
    b.on_key(key('m'), ctx);
    b.on_key(key('l'), ctx);
    let Some(Overlay::Dispatch(sheet)) = &b.overlay else {
        panic!("dispatch sheet expected")
    };
    assert_eq!(sheet.mode, RunMode::Headless);
    assert_ne!(Some(sheet.choice.profile()), Some("omo"));
    assert_eq!(b.on_key(code(KeyCode::Esc), ctx), Action::None);
    assert!(b.overlay.is_none());

    b.on_key(key('?'), ctx);
    assert!(matches!(b.overlay, Some(Overlay::Help(_))));
    let text = render(&mut b, ctx);
    assert!(
        text.contains("keys") && text.contains("move selection"),
        "{text}"
    );
    b.on_key(key('x'), ctx);
    assert!(b.overlay.is_none());
}

#[test]
fn board_capture_saves_idea_and_move_follows_card() {
    let fx = seeded();
    let ctx = &fx.ctx;
    let mut b = fx.board();
    b.on_key(key('n'), ctx);
    assert!(matches!(b.overlay, Some(Overlay::Capture(_))));
    for c in "quick thought".chars() {
        b.on_key(key(c), ctx);
    }
    assert!(render(&mut b, ctx).contains("› quick thought"));
    let action = b.on_key(code(KeyCode::Enter), ctx);
    assert!(
        matches!(&action, Action::Toast(t) if t.title == "T-008 captured"),
        "{action:?}"
    );
    assert!(b.overlay.is_none());
    assert_eq!(ctx.store.get("T-008").unwrap().status, Status::Idea);
    assert!(render(&mut b, ctx).contains("T-008"));

    b.on_key(key('l'), ctx);
    assert_eq!(b.on_key(key('H'), ctx), Action::None);
    assert_eq!(ctx.store.get("T-003").unwrap().status, Status::Backlog);
    assert_eq!(
        b.selected_task().map(|t| t.id.as_str()),
        Some("T-003"),
        "selection follows the moved card"
    );
    assert_eq!(b.on_key(key('L'), ctx), Action::None);
    assert_eq!(ctx.store.get("T-003").unwrap().status, Status::Ready);
}
