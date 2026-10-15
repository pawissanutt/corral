use std::io::Write;

use corral_core::{
    Accept, ArtifactKind, AutoStage, AutopilotState, GateRecord, GateResult, ReviewRecord, Status,
    StuckProcess, Verdict,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::{DetailView, Z_ACC};
use crate::artifacts::panels_fixture::{Fixture, fixture, key, render};
use crate::{Action, Hit, HitMap, MouseCtx, Tone, View};

/// No live process has this pid (above Linux's pid_max), so ops must refuse to kill it.
const GONE_PID: u32 = u32::MAX;

fn set_stuck(fx: &Fixture, stuck: Option<StuckProcess>) {
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.status = Status::Running;
            if let Some(r) = t.current_run_mut() {
                r.outcome = None;
                r.ended = None;
                r.idle_no_report = true;
                r.stuck = stuck;
            }
            Ok(())
        })
        .unwrap();
}

fn stuck(pid: u32, argv: &[&str]) -> StuckProcess {
    StuckProcess {
        pid,
        argv: argv.iter().map(|a| a.to_string()).collect(),
        started: "2026-01-01T11:58:00Z".into(),
    }
}

#[test]
fn detail_unstick_confirms_the_captured_process() {
    let fx = fixture();
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    let none = v.on_key(key(KeyCode::Char('u')), &fx.ctx);
    assert!(
        matches!(&none, Action::Toast(t) if t.tone == Tone::Info),
        "{none:?}"
    );

    let shown = stuck(GONE_PID, &["sleep", "1000"]);
    set_stuck(&fx, Some(shown.clone()));
    v.refresh(&fx.ctx);
    v.on_key(key(KeyCode::Char('2')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("⚠ stuck: sleep 1000"), "{screen}");
    assert!(
        screen.contains(&format!("pid {GONE_PID}")) && !screen.contains("idle, no report"),
        "stuck replaces the idle warning\n{screen}"
    );
    assert!(v.hints().contains(&("u", "unstick")));

    v.on_key(key(KeyCode::Char('u')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(
        screen.contains(&format!("Unstick run R1? Kill pid {GONE_PID}:"))
            && screen.contains("[Unstick]"),
        "{screen}"
    );
    assert_eq!(v.on_key(key(KeyCode::Esc), &fx.ctx), Action::None);
    assert!(!render(&mut v, &fx.ctx).contains("Unstick run R1?"));

    v.on_key(key(KeyCode::Char('u')), &fx.ctx);
    set_stuck(&fx, Some(stuck(1, &["other"])));
    v.refresh(&fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(
        screen.contains(&format!("Kill pid {GONE_PID}:")) && screen.contains("   sleep 1000"),
        "the popup keeps the process it opened with\n{screen}"
    );
    let out = v.on_key(key(KeyCode::Enter), &fx.ctx);
    assert!(
        matches!(&out, Action::Toast(t) if t.tone == Tone::Bad),
        "a changed process is refused, never swapped in\n{out:?}"
    );
    let run = fx.ctx.store.get(&fx.id).unwrap().runs.remove(0);
    assert_eq!(run.stuck, Some(stuck(1, &["other"])));
    assert!(!render(&mut v, &fx.ctx).contains("Unstick run R1?"));
}

#[test]
fn panels_detail_renders_and_switches_tabs() {
    let fx = fixture();
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("Witness shard cache"), "{screen}");
    assert!(
        screen.contains("1 Spec") && screen.contains("2 Runs") && screen.contains("4 Diff"),
        "{screen}"
    );
    assert!(
        screen.contains("Cache shard reads per character."),
        "{screen}"
    );
    assert!(
        screen.contains("Live") && screen.contains("No live run."),
        "{screen}"
    );

    v.on_key(key(KeyCode::Char('2')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(
        screen.contains("outcome")
            && screen.contains("harness·model·effort")
            && screen.contains(&format!("corral/{}", fx.id)),
        "{screen}"
    );
    assert!(
        !screen.contains("Cache shard reads per character."),
        "{screen}"
    );

    v.on_key(key(KeyCode::Char('3')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(screen.contains("QA checklist"), "{screen}");

    v.on_key(key(KeyCode::Char('4')), &fx.ctx);
    let screen = render(&mut v, &fx.ctx);
    assert!(
        screen.contains("shard_cache.rs") && screen.contains("+pub struct ShardCache;"),
        "{screen}"
    );

    assert_eq!(
        v.on_key(key(KeyCode::Char('a')), &fx.ctx),
        Action::Jump(fx.id.clone())
    );
    assert_eq!(v.on_key(key(KeyCode::Esc), &fx.ctx), Action::Back);
}

#[test]
fn done_task_detail_reads_snapshots_after_the_worktree_is_removed() {
    // Given a reviewed task whose final diff was snapshotted, approved with its worktree removed.
    let fx = fixture();
    let run = fx.ctx.store.get(&fx.id).unwrap().runs[0].clone();
    let diff = corral_git::diff_text(&run.worktree, &run.base_sha).unwrap();
    let diff_file = fx.root.join("final.diff");
    std::fs::write(&diff_file, diff).unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &diff_file,
        Some(corral_core::ArtifactKind::Diff),
        Some("Final diff +3 −0"),
    )
    .unwrap();
    corral_ops::approve(
        &fx.ctx,
        &fx.id,
        corral_ops::ApproveStrategy::FastForward,
        true,
        false,
    )
    .unwrap();
    assert!(!run.worktree.exists(), "worktree removed");

    // When the Done task's detail opens on its Diff tab, then on Artifacts.
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    v.on_key(key(KeyCode::Char('4')), &fx.ctx);
    let diff_tab = render(&mut v, &fx.ctx);
    v.on_key(key(KeyCode::Char('3')), &fx.ctx);
    let artifacts_tab = render(&mut v, &fx.ctx);

    // Then the diff and its stat come from the snapshot, and the artifacts are still listed.
    assert!(
        diff_tab.contains("Done · P2")
            && diff_tab.contains("1 files  +3 −0")
            && diff_tab.contains("+pub struct ShardCache;"),
        "{diff_tab}"
    );
    assert!(
        artifacts_tab.contains("QA checklist") && artifacts_tab.contains("Final diff +3 −0"),
        "{artifacts_tab}"
    );
    v.on_key(key(KeyCode::Char('2')), &fx.ctx);
    let runs_tab = render(&mut v, &fx.ctx);
    assert!(
        runs_tab.contains("(removed)") && runs_tab.contains("Added the shard cache."),
        "{runs_tab}"
    );
}

#[test]
fn acceptance_tab_shows_policy_sources_results_records_and_history() {
    let fx = fixture();
    let mut ctx = fx.ctx.clone();
    ctx.store.config.review.gate = Some(vec!["cargo test".into()]);
    ctx.store.config.review.reviewer = Some("pm".into());
    let app = ctx.store.config.projects.get_mut("app").unwrap();
    app.review.accept = Some(Accept::Auto);
    let at = "2026-01-01T12:05:00Z".parse().unwrap();
    let sha = "abcdef1234567890".to_string();
    ctx.store
        .update(&fx.id, |t| {
            t.rounds = 1;
            if let Some(r) = t.current_run_mut() {
                r.gate = Some(GateRecord {
                    result: GateResult::Pass,
                    failed_cmd: None,
                    at,
                    sha: sha.clone(),
                    pane: None,
                });
                r.review = Some(ReviewRecord {
                    verdict: Some(Verdict::Approve),
                    reviewer: "pm".into(),
                    notes: Some("Looks right.\nKeep the cache bounded.".into()),
                    sha: sha.clone(),
                    at,
                    pane: None,
                });
                r.autopilot = Some(AutopilotState {
                    stage: AutoStage::Reviewed,
                    reason: None,
                    sha: Some(sha.clone()),
                    at,
                });
            }
            Ok(())
        })
        .unwrap();
    let log = fx.root.join("gate.log");
    std::fs::write(&log, "$ cargo test\nok\n").unwrap();
    corral_ops::add_artifact(
        &ctx,
        &fx.id,
        &log,
        Some(ArtifactKind::Log),
        Some("Gate: pass"),
    )
    .unwrap();
    let mut events = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(ctx.store.dir().join("events.jsonl"))
        .unwrap();
    writeln!(
        events,
        r#"{{"ts":"2026-01-01T12:06:00Z","task":"{}","kind":"autopilot","detail":{{"run":"R1","from":"reviewing","to":"reviewed","note":"gate and review passed"}}}}"#,
        fx.id
    )
    .unwrap();

    let mut v = DetailView::new(&ctx, &fx.id).unwrap();
    assert!(render(&mut v, &ctx).contains("5 Acceptance"), "tab label");
    v.on_key(key(KeyCode::Char('5')), &ctx);
    let screen = render(&mut v, &ctx);
    let line = |needle: &str| {
        screen
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} missing\n{screen}"))
            .to_string()
    };
    assert!(line("Policy in effect").contains("Policy in effect"));
    let gate = line("cargo test");
    assert!(gate.contains("gate") && gate.contains("[review]"), "{gate}");
    let accept = line("accept ");
    assert!(
        accept.contains("auto") && accept.contains("[projects.app.review]"),
        "{accept}"
    );
    assert!(line("max rounds").contains("built-in default"));
    assert!(line("set by").contains("value"), "column header");
    assert!(line("stage").contains("reviewed since 2026-01-01 12:05"));
    assert!(line("change rounds used").contains("1 of 2"));
    assert!(line("landing").contains("not landed"));
    let run = line("gate pass");
    assert!(
        run.contains("R1") && run.contains("@abcdef1") && run.contains("review approve by pm"),
        "{run}"
    );
    assert!(line("Keep the cache bounded.").contains("Keep the cache"));
    assert!(line("Gate: pass").contains("Gate: pass"), "gate log record");
    assert!(
        !screen.contains("QA checklist"),
        "ordinary artifacts are not acceptance records\n{screen}"
    );
    assert!(line("reviewing → reviewed").contains("gate and review passed"));
    assert!(v.hints().contains(&("⏎", "preview")));

    v.on_key(key(KeyCode::Enter), &ctx);
    assert!(v.preview.is_some(), "⏎ previews the selected record");
    let screen = render(&mut v, &ctx);
    assert!(screen.contains("$ cargo test"), "{screen}");
    v.on_key(key(KeyCode::Esc), &ctx);
    assert!(v.preview.is_none());

    // The same record by mouse: a click on the selected record row previews it like ⏎.
    let mut hits = HitMap::default();
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    term.draw(|f| v.draw(f, f.area(), &ctx, &mut hits)).unwrap();
    let (rect, hit) = hits
        .iter()
        .find(|(_, h)| *h == Hit::Item(Z_ACC, 0))
        .cloned()
        .expect("record row is clickable");
    let tab = hits
        .iter()
        .any(|(_, h)| *h == Hit::Key(key(KeyCode::Char('5'))));
    assert!(tab, "the Acceptance tab label presses 5");
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    let m = MouseCtx {
        ctx: &ctx,
        hit: Some(hit),
        double: false,
    };
    assert_eq!(v.on_mouse(ev, &m), Action::None);
    assert!(v.preview.is_some(), "click on the selected record = ⏎");
}

#[test]
fn review_screen_opens_the_acceptance_tab_by_key_and_by_footer_click() {
    use crate::app::App;
    use crate::app::mouse_tests::{click, find, render as app_render};

    let fx = fixture();
    for by_mouse in [false, true] {
        let mut app = App::new(fx.ctx.clone(), crate::Mode::Board).unwrap();
        app.apply(Action::OpenReview(fx.id.clone()));
        let text = app_render(&mut app);
        let footer = text.lines().count() - 1;
        let hints = text.lines().nth(footer).unwrap();
        assert!(
            hints.contains("a approve") && hints.contains("i acceptance"),
            "{hints}"
        );
        if by_mouse {
            let (x, y) = find(&text, footer, "i acceptance");
            click(&mut app, x, y);
        } else {
            app.handle_key(key(KeyCode::Char('i')));
        }
        let text = app_render(&mut app);
        assert!(
            text.contains("Policy in effect") && text.contains("not picked up by the autopilot"),
            "opens on the Acceptance tab\n{text}"
        );
    }
}
