use std::io::Write;

use corral_core::{
    Accept, ArtifactKind, AutoStage, AutopilotState, GateRecord, GateResult, ReviewRecord, Status,
    StuckProcess, Verdict,
};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use ratatui::Terminal;

use super::{DetailView, Z_ACC};
use crate::artifacts::panels_fixture::{fixture, key, render};

use crate::artifacts::panels_fixture::Fixture;
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
    v.on_key(key(KeyCode::Char('f')), &fx.ctx);
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
                    identity: None,
                    commands: vec![],
                    log_sha256: None,
                    result: GateResult::Pass,
                    failed_cmd: None,
                    at,
                    sha: sha.clone(),
                    pane: None,
                });
                r.review = Some(ReviewRecord {
                    identity: None,
                    evidence_digest: None,
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
    use crate::app::mouse_tests::{click, find, render as app_render};

    use crate::app::App;

    let fx = fixture();
    for by_mouse in [false, true] {
        let mut app = App::new(fx.ctx.clone(), crate::Mode::Board).unwrap();
        app.apply(Action::OpenReview(fx.id.clone()));
        app.handle_key(crate::board::tests::code(KeyCode::Esc));
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

#[test]
fn usage_rows_render_totals_and_keep_mouse_targets_after_scrolling() {
    use corral_core::{Status, Store};
    use serde_json::json;

    // Given a priced run with reviewer usage, an unknown run, and an unpriced run.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.pricing.insert(
        "priced".into(),
        serde_json::from_value(json!({"input":10000.0,"output":0.0})).unwrap(),
    );
    let mut task = store.create("Costs", None, Status::Done, "").unwrap();
    let base = json!({
        "id":"R1","agent":"test","model":"priced","worktree":"/unused",
        "branch":"test","base":"main","base_sha":"abc",
        "started":"2026-01-01T00:00:00Z","ended":"2026-01-01T00:01:00Z",
        "usage":{"input":10,"output":0,"cache_read":0,"cache_write":0,"cost_usd":0.1}
    });
    let mut first = base.clone();
    first["review_runs"] = json!([base]);
    let mut unknown = base.clone();
    unknown["id"] = json!("R2");
    unknown["usage"] = serde_json::Value::Null;
    let mut unpriced = base;
    unpriced["id"] = json!("R3");
    unpriced["model"] = serde_json::Value::Null;
    unpriced["usage"] = json!({"input":5,"output":0,"cache_read":0,"cache_write":0});
    task.runs = vec![first, unknown, unpriced]
        .into_iter()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect();
    store.save(&task).unwrap();
    let ctx = corral_ops::Ctx::from_store(store);
    let mut view = DetailView::new(&ctx, &task.id).unwrap();
    view.on_key(key(KeyCode::Char('2')), &ctx);

    // When drawn by TestBackend at compact, normal, and wide terminal sizes.
    for (width, height, offset) in [(120, 24, 0), (200, 32, 0), (120, 24, 2)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut hits = crate::HitMap::default();
        view.scroll.offset = offset;
        terminal
            .draw(|frame| {
                view.draw(frame, frame.area(), &ctx, &mut hits);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let lines: Vec<String> = (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let screen = lines.join("\n");

        // Then costs stay in their own rows and both run rows select the matching run.
        assert!(screen.contains("est. $0.20; 20t"), "{screen}");
        assert!(screen.contains("cost: unknown"), "{screen}");
        assert!(screen.contains("est. -; 5t"), "{screen}");
        assert!(
            screen.contains("Total: reported -; est. $0.20; 25t"),
            "{screen}"
        );
        for (rect, hit) in hits.iter() {
            if let crate::hit::Hit::Item(super::Z_RUN, index) = hit {
                let row = &lines[usize::from(rect.y)];
                let expected = if row.contains("cost:") {
                    match index {
                        0 => "20t",
                        1 => "unknown",
                        2 => "5t",
                        _ => panic!("unexpected run index"),
                    }
                } else {
                    &view.task.runs[*index].id
                };
                assert!(row.contains(expected), "{index}: {row}");
            }
        }
        if width >= 120 {
            assert!(
                screen.contains(&super::cost_summary(view.task.total_usage())),
                "{screen}"
            );
            assert!(
                screen.contains(&super::cost_summary(view.task.runs[0].total_usage())),
                "{screen}"
            );
            if offset == 0 {
                println!("{screen}");
            }
        }
    }
}

#[test]
fn live_tail_scrolls_by_keyboard_without_moving_spec() {
    // Given a tail longer than the Live panel.
    let fx = fixture();
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    v.live = Ok((1..=80).map(|i| format!("live line {i:03}")).collect());
    let bottom = render(&mut v, &fx.ctx);
    assert!(bottom.contains("live line 080"));

    // When the keyboard scrolls older and back to newest.
    v.on_key(key(KeyCode::Char('[')), &fx.ctx);
    let older = render(&mut v, &fx.ctx);
    assert!(!older.contains("live line 080"));
    assert!(older.contains("live line 077"));
    assert_eq!(v.scroll.offset, 0);
    v.on_key(key(KeyCode::Char(']')), &fx.ctx);

    // Then the original tail returns without disturbing the Spec.
    assert_eq!(render(&mut v, &fx.ctx), bottom);
}

#[test]
fn clicking_the_raw_fold_row_expands_it_like_f() {
    let fx = fixture();
    let diff = fx.root.join("x.diff");
    std::fs::write(&diff, "+x\n").unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &diff,
        Some(ArtifactKind::Diff),
        Some("Final diff"),
    )
    .unwrap();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            for a in t.artifacts.iter_mut().filter(|a| a.title == "Final diff") {
                a.tier_override = Some(corral_core::ArtifactTier::Transient);
            }
            Ok(())
        })
        .unwrap();
    let mut v = DetailView::new(&fx.ctx, &fx.id).unwrap();
    v.on_key(key(KeyCode::Char('3')), &fx.ctx);
    assert!(
        !render(&mut v, &fx.ctx).contains("Final diff"),
        "raw items start folded"
    );

    let fold = v
        .list
        .rows(&v.task.artifacts)
        .iter()
        .position(|r| *r == crate::artifacts::list::Row::Fold)
        .expect("a fold row");
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    let m = MouseCtx {
        ctx: &fx.ctx,
        hit: Some(Hit::Item(super::Z_ART, fold)),
        double: false,
    };
    assert_eq!(v.on_mouse(ev, &m), Action::None);
    assert!(
        render(&mut v, &fx.ctx).contains("Final diff"),
        "a click on the fold shows raw items"
    );
}
