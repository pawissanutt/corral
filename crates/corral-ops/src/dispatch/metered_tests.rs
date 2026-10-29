use std::sync::mpsc;

use corral_core::Status;

use crate::agent::hooks::metered_tests::{
    assert_notice_before_pane_run, assert_one_notice, fixture, isolated,
};

#[test]
fn worker_dispatch_emits_one_metered_notice_despite_repeated_readiness() {
    isolated("dispatch::metered_tests::worker_dispatch_child");
}

#[test]
#[ignore = "driven with private HOME/XDG by the parent test"]
fn worker_dispatch_child() {
    let (mut fx, resolved) = fixture();
    fx.ctx.store.config.dispatch.paused = false;
    fx.ctx.store.config.defaults.profile = Some("bedrock".into());
    fx.ctx.store.save_config().unwrap();
    let task = fx
        .ctx
        .store
        .create("Metered worker", Some("corral"), Status::Ready, "Fixture")
        .unwrap();
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    // Given repeated route readiness, which must have no presentation side effects.
    for _ in 0..2 {
        crate::agent::hooks::preflight::check(
            &fx.ctx,
            corral_core::agent::AgentRole::Worker,
            &resolved,
        )
        .unwrap();
    }
    assert!(receive.try_recv().is_err());
    assert!(fx.events("metered_notice").is_empty());
    assert_notice_before_pane_run(&fx, "worker");
    // When dispatch performs its own repeated preflight and submits the one host command.
    let outcome = crate::dispatch(&fx.ctx, &task.id, &crate::DispatchOpts::default()).unwrap();
    assert!(matches!(outcome, crate::DispatchOutcome::Started { .. }));
    // Then the launch, rather than any readiness probe, presented exactly one notice.
    assert_one_notice(&fx, "worker", &receive);
    assert_eq!(fx.calls("pane run").len(), 1);
}

#[test]
fn planner_launch_emits_one_metered_notice_before_submission() {
    isolated("dispatch::metered_tests::planner_launch_child");
}

#[test]
#[ignore = "driven with private HOME/XDG by the parent test"]
fn planner_launch_child() {
    let (mut fx, _) = fixture();
    fx.ctx.store.config.dispatch.paused = false;
    fx.ctx.store.config.defaults.profile = Some("bedrock".into());
    fx.ctx.store.save_config().unwrap();
    let engine = fx.root().join("herdr");
    let source = std::fs::read_to_string(&engine).unwrap();
    crate::agent::hooks::metered_tests::write(
        &engine,
        &source.replace(
            r#""foreground_processes":[]"#,
            r#""foreground_processes":[{"pid":11,"argv":["fixture-planner"]}]"#,
        ),
    );
    let task = fx
        .ctx
        .store
        .create("Metered planner", Some("corral"), Status::Ready, "Fixture")
        .unwrap();
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    assert_notice_before_pane_run(&fx, "planner");
    // When a plan dispatch reaches its once-per-launch presentation point.
    let outcome = crate::dispatch(
        &fx.ctx,
        &task.id,
        &crate::DispatchOpts {
            plan: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(outcome, crate::DispatchOutcome::Started { .. }));
    // Then the notice was durable before the fake engine accepted the command.
    assert_one_notice(&fx, "planner", &receive);
    assert_eq!(fx.calls("pane run").len(), 1);
}
