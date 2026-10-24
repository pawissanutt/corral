//! "Run plan review" through the App's own key and mouse path, against the private plugin-free herdr server.
//! Each test skips (with a printed note) when `herdr --session corral-test` is not running.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use corral_core::{AgentCmd, Status};
use corral_ops::plan_submit;

use super::App;
use super::mouse_tests::{click, find_any, render};
use crate::Action;
use crate::board::tests::{Fixture, key, run, seeded};
use crate::plan::tests::{PLAN, planning};
use crate::stages::Stage;

#[derive(Clone, Copy, Debug)]
enum Surface {
    DetailPlanTab,
    DetailOtherTab,
    StageMonitor,
}

fn herdr_ready() -> bool {
    Command::new("herdr")
        .args(["--session", "corral-test", "workspace", "list"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// A submitted plan, no review yet, whose reviewer is a harmless `true` agent in the private herdr.
fn live() -> Option<(Fixture, String)> {
    if !herdr_ready() {
        println!("skipped: no private corral-test herdr server (run under scripts/test-herdr.sh)");
        return None;
    }
    let mut fx = seeded();
    let id = planning(&mut fx);
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("projects.corral.path", "."))
        .unwrap();
    fx.ctx.store.reload_config_if_changed().unwrap();
    let config = &mut fx.ctx.store.config;
    config.profiles.remove("reviewer");
    config.agents.insert(
        "reviewer".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    config.review.reviewer = Some("reviewer".into());
    fx.ctx.store.save_config().unwrap();
    let bin = fx.ctx.store.root().join("live-herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../corral-ops/tests/support/live-herdr.sh"),
        &bin,
    )
    .unwrap();
    fx.ctx.herdr = corral_herdr::Herdr::with_bin(PathBuf::from(bin), Some("corral-test".into()));
    fx.ctx.resource_probe = corral_ops::resources::FixedResourceProbe::arc();
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    plan_submit(&fx.ctx, &id, &path).unwrap();
    Some((fx, id))
}

fn open(fx: &Fixture, id: &str, surface: Surface) -> App {
    let mut app = fx.app(crate::Mode::Board);
    render(&mut app);
    match surface {
        Surface::DetailPlanTab => app.apply(Action::OpenPlan(id.into())),
        Surface::DetailOtherTab => {
            app.apply(Action::OpenPlan(id.into()));
            app.handle_key(key('1'));
        }
        Surface::StageMonitor => app.apply(Action::OpenStage(id.into(), Stage::Review)),
    }
    let text = render(&mut app);
    assert!(text.contains("R run review"), "{surface:?}\n{text}");
    app
}

fn press_r(app: &mut App) {
    app.handle_key(key('R'));
}

fn click_label(app: &mut App, label: &str) {
    let text = render(app);
    let (x, y) = find_any(&text, label);
    click(app, x, y);
}

fn assert_started(fx: &Fixture, id: &str, app: &mut App, how: &str) {
    let task = fx.ctx.store.get(id).unwrap();
    let reviews = &task.plans[0].reviews;
    assert_eq!(
        reviews.len(),
        1,
        "{how}: exactly one review attempt\n{:#?}",
        app.toasts
            .iter()
            .map(|(t, _)| format!("{}: {}", t.title, t.body))
            .collect::<Vec<_>>()
    );
    let review = &reviews[0];
    assert_eq!(review.id, "PR1", "{how}");
    assert_eq!(review.reviewer, "reviewer", "{how}: routed reviewer");
    let pane = review
        .herdr
        .as_ref()
        .unwrap_or_else(|| panic!("{how}: no pane recorded"))
        .pane_id
        .clone();
    let text = render(app);
    assert!(text.contains("plan review PR1 started"), "{how}\n{text}");
    assert!(text.contains("reviewer reviewer"), "{how}\n{text}");
    assert!(
        text.contains(&pane),
        "{how}: pane {pane} not in toast\n{text}"
    );
    assert!(
        !text.contains("R run review"),
        "{how}: the action must go away once a review is running\n{text}"
    );
}

#[test]
fn keyboard_alone_starts_the_routed_reviewer_from_every_surface() {
    for surface in [
        Surface::DetailPlanTab,
        Surface::DetailOtherTab,
        Surface::StageMonitor,
    ] {
        let Some((fx, id)) = live() else { return };
        let mut app = open(&fx, &id, surface);
        press_r(&mut app);
        assert_started(&fx, &id, &mut app, &format!("key on {surface:?}"));
    }
}

#[test]
fn mouse_alone_starts_the_routed_reviewer_from_the_panel_button_and_each_stage_cell() {
    for (surface, label) in [
        (Surface::DetailPlanTab, "Run plan review"),
        (Surface::DetailPlanTab, "R run review"),
        (Surface::StageMonitor, "R run review"),
    ] {
        let Some((fx, id)) = live() else { return };
        let mut app = open(&fx, &id, surface);
        click_label(&mut app, label);
        assert_started(
            &fx,
            &id,
            &mut app,
            &format!("click {label:?} on {surface:?}"),
        );
    }
}

#[test]
fn transient_refusals_leave_a_retry_that_succeeds_once_they_clear() {
    let Some((mut fx, id)) = live() else { return };
    fx.ctx.store.config.limits.count_reviews = true;
    fx.ctx.store.config.limits.running = 3;
    fx.ctx.store.save_config().unwrap();
    let mut app = open(&fx, &id, Surface::DetailPlanTab);
    let reviews = |fx: &Fixture| fx.ctx.store.get(&id).unwrap().plans[0].reviews.len();

    let held = fx.ctx.store.try_lock("session-ownership").unwrap().unwrap();
    press_r(&mut app);
    let text = render(&mut app);
    assert!(text.contains("plan review not started"), "{text}");
    assert!(text.contains("pane inventory"), "{text}");
    assert!(
        text.contains("R run review"),
        "retry stays on offer\n{text}"
    );
    assert_eq!(reviews(&fx), 0);
    drop(held);

    let filler = fx
        .ctx
        .store
        .create("Filler", Some("corral"), Status::Ready, "Hold a slot.\n")
        .unwrap()
        .id;
    fx.ctx
        .store
        .update(&filler, |t| {
            t.status = Status::Running;
            t.runs.push(run("R1", "omo"));
            Ok(())
        })
        .unwrap();
    press_r(&mut app);
    let text = render(&mut app);
    assert!(text.contains("plan review not started"), "{text}");
    assert!(text.contains("waiting for a slot"), "{text}");
    assert_eq!(reviews(&fx), 0);

    fx.ctx
        .store
        .update(&filler, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    click_label(&mut app, "Run plan review");
    assert_started(&fx, &id, &mut app, "retry by click");
}
