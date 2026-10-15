use super::*;

const RUNTIME_KEYS: &[&str] = &[
    "autopilot",
    "ui",
    "herdr.keys",
    "herdr.session",
    "limits",
    "pm.push",
    "sync",
    "artifacts",
    "viewers",
    "defaults",
    "routing",
    "project",
];

fn reviewing() -> Fx {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.remove_worktree = Some(false);
    });
    let mut store = Store::open(fx.ctx.store.root()).unwrap();
    store.config.profiles.get_mut("omo").unwrap().vendor = Some("openai".into());
    store.save_config().unwrap();
    fx.change("widget.txt", "spins\n");
    fx.done();
    fx
}

fn edit_config(fx: &Fx, key: &str) {
    let mut store = Store::open(fx.ctx.store.root()).unwrap();
    let config = &mut store.config;
    match key {
        "autopilot" => config.review.autopilot = Some(false),
        "ui" => config.ui.mouse = false,
        "herdr.keys" => config.herdr.keys.board = "prefix+b".into(),
        "herdr.session" => config.herdr.session = Some("corral-test".into()),
        "limits" => config.limits.running = 8,
        "pm.push" => {
            config.pm.push_events = true;
            config.pm.push_kinds = vec!["done".into()];
        }
        "sync" => config.sync.idle_after_secs = 900,
        "artifacts" => config.artifacts.max_total = "1GB".into(),
        "viewers" => {
            config.viewers.insert("md".into(), "system".into());
        }
        "defaults" => config.defaults.profile = Some("fake".into()),
        "routing" => config.routing.escalate_after = 5,
        "project" => config.projects.get_mut("app").unwrap().base = Some("other".into()),
        "gate" => config.review.gate = gate(&["false"]),
        "project.gate" => config.projects.get_mut("app").unwrap().review.gate = gate(&["false"]),
        "reviewer" => config.review.reviewer = Some("fake".into()),
        "accept" => config.review.accept = Some(Accept::Auto),
        "max_rounds" => config.review.max_rounds = Some(8),
        "agent" => config.agents.get_mut("rev").unwrap().command = "false".into(),
        "profile" => {
            let mut profile = config.profiles["omo"].clone();
            profile.args = vec!["--changed".into()];
            config.profiles.insert("rev".into(), profile);
        }
        "harness" => config.harnesses.get_mut("omo").unwrap().command = "false {model_args}".into(),
        "reviewer_mapping" => {
            config
                .routing
                .reviewer_by_vendor
                .insert("openai".into(), "omo".into());
        }
        "pm.agent" => config.pm.agent = "rev".into(),
        _ => panic!("unknown config edit {key}"),
    }
    store.save_config().unwrap();
}

#[test]
fn runtime_config_edits_preserve_inflight_gate_receipts() {
    for key in RUNTIME_KEYS {
        // Given an executed gate whose result has not been collected yet.
        let fx = reviewing();
        let started = corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
        // When an operational setting changes through a separate store.
        edit_config(&fx, key);
        // Then collection accepts the original identity, without rerunning.
        let collected = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
        assert_eq!(collected.result, GateResult::Pass, "{key}: {collected:?}");
        assert_eq!(collected.identity, started.identity, "{key}");
        assert_eq!(fx.events("gate_started"), 1, "{key}");
        println!("INFLIGHT_FRESH {key}");
    }
}

#[test]
fn runtime_config_edits_preserve_passed_gates_and_approved_verdicts() {
    for key in RUNTIME_KEYS {
        // Given a passed gate and an approved semantic review.
        let fx = reviewing();
        fx.sync();
        fx.verdict(Verdict::Approve, "Reviewed before runtime config edit.");
        let original = fx.run();
        // When a runtime setting changes after both receipts were recorded.
        edit_config(&fx, key);
        // Then the existing gate and verdict still authorize landing.
        let collected = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
        assert_eq!(collected.result, GateResult::Pass, "{key}: {collected:?}");
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true)
            .unwrap_or_else(|error| panic!("{key}: {error:#}"));
        assert_eq!(fx.task().status, Status::Done, "{key}");
        assert_eq!(fx.run().gate, original.gate, "{key}");
        assert_eq!(fx.run().review, original.review, "{key}");
        assert_eq!(fx.events("gate_started"), 1, "{key}");
        assert_eq!(fx.events("review_started"), 1, "{key}");
        assert!(fx.app.join("widget.txt").is_file(), "{key}");
        println!("GATE_AND_VERDICT_FRESH {key}");
    }
}

#[test]
fn verification_config_edits_invalidate_gates_and_verdicts() {
    for key in [
        "gate",
        "project.gate",
        "reviewer",
        "accept",
        "max_rounds",
        "agent",
        "profile",
        "harness",
        "reviewer_mapping",
        "pm.agent",
    ] {
        // Given a passed gate and a pending review on an unchanged candidate.
        let fx = reviewing();
        fx.sync();
        let original = fx.run();
        // When verification policy or reviewer execution settings change.
        edit_config(&fx, key);
        // Then neither the old verdict request nor its gate can be accepted.
        let error = corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Approve, "Old config.")
            .unwrap_err()
            .to_string();
        assert!(error.contains("stale review"), "{key}: {error}");
        let collected = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
        assert_eq!(collected.result, GateResult::Fail, "{key}");
        assert_eq!(collected.sha, original.gate.unwrap().sha, "{key}");
        assert!(collected.failed_cmd.unwrap().starts_with("stale_result:"));
        assert_eq!(fx.run().review.unwrap().verdict, None, "{key}");
        assert!(!fx.app.join("widget.txt").exists(), "{key}");
        println!("GATE_AND_VERDICT_STALE {key}");
    }
}
