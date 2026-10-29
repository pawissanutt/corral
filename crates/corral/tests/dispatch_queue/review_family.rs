//! T-133: exercise real CLI policy resolution and launch admission, without agents.
use std::process::Output;

use corral_core::{PlanRun, PlanRunState, Profile, ReviewOverride, RunOutcome, Status};
use serde_json::{Value, json};

use crate::queue_support::Fixture;

const PLAN: &str = "---\ngoal: Ship widget\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: spin, expected: clockwise}]\nverification: [check]\nowner_decisions: []\n---\n";

const BEDROCK_GPT: &str = "amazon-bedrock/openai.gpt-oss-120b-1:0";
const BEDROCK_CLAUDE: &str = "amazon-bedrock/us.anthropic.claude-sonnet-4-5-20250929-v1:0";

fn fixture() -> Fixture {
    let mut f = crate::queue_fixture();
    let config = &mut f.ctx.store.config;
    config.review = ReviewOverride {
        reviewer: Some("author".into()),
        autopilot: Some(false),
        ..Default::default()
    };
    for project in config.projects.values_mut() {
        project.review = ReviewOverride::default();
    }
    config.defaults.profile = Some("author".into());
    config.pm.agent = "author".into();
    config.profiles.clear();
    config.agents.clear();
    config.routing.reviewer_by_vendor = [
        ("openai".into(), "author".into()),
        ("anthropic".into(), "different-vendor".into()),
    ]
    .into();
    let harness = config.harnesses.get_mut("generic").unwrap();
    harness.command = "echo {model_args}".into();
    harness.model_flag = Some("--model {model}".into());
    for (name, model, vendor) in [
        ("author", "openai/gpt-fixture", "openai"),
        ("same-model", "openai/gpt-fixture", "openai"),
        ("same-vendor", "openai/other-fixture", "openai"),
        ("different-vendor", "anthropic/claude-fixture", "anthropic"),
        ("bedrock-gpt", BEDROCK_GPT, "amazon-bedrock"),
        ("bedrock-claude", BEDROCK_CLAUDE, "amazon-bedrock"),
    ] {
        config.profiles.insert(
            name.into(),
            Profile {
                metadata: None,
                harness: "generic".into(),
                model: Some(model.into()),
                vendor: Some(vendor.into()),
                effort: None,
                args: vec![],
                escalate_to: None,
            },
        );
    }
    f.ctx.store.save_config().unwrap();
    // Exact model discovery is unrelated to family policy. Use the existing
    // harness-cache wire format, so routing never probes an installed agent.
    let cache = json!({
        "detected_at": jiff::Timestamp::now(),
        "harnesses": [{
            "key": "generic", "program": "echo", "path": null, "version": null,
            "models": ["openai/gpt-fixture", "openai/other-fixture", "anthropic/claude-fixture", BEDROCK_GPT, BEDROCK_CLAUDE],
            "model_source": "listed", "efforts": [], "takes_effort": false, "notes": []
        }]
    });
    std::fs::write(
        f.ctx.store.state_dir().join(corral_ops::HARNESS_CACHE_FILE),
        serde_json::to_vec(&cache).unwrap(),
    )
    .unwrap();
    f
}

fn cli(f: &Fixture, args: &[&str]) -> Output {
    let mut command = f.command(args);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    let home = f.temp.path().join("private-home");
    command
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_RUNTIME_DIR", f.temp.path())
        .env("CORRAL_ROOT", f.temp.path())
        .env("HERDR_BIN_PATH", &f.bin)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .output()
        .unwrap()
}

fn ok(f: &Fixture, args: &[&str]) -> Output {
    let output = cli(f, args);
    assert!(
        output.status.success(),
        "{args:?}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn set(f: &Fixture, key: &str, value: &str) {
    ok(f, &["config", "set", key, value, "--workspace"]);
}

fn route(f: &Fixture, id: &str) -> Value {
    serde_json::from_slice(&ok(f, &["route", id, "--json"]).stdout).unwrap()
}

#[test]
fn config_boolean_roundtrips_at_global_and_project_scopes() {
    // Given a private configuration with the default policy.
    let f = fixture();
    let default = ok(&f, &["config", "get", "review.require_cross_family"]);
    assert_eq!(String::from_utf8_lossy(&default.stdout).trim(), "false");
    for key in [
        "review.require_cross_family",
        "projects.app.review.require_cross_family",
    ] {
        for value in ["true", "false"] {
            // When config set writes a boolean through the actual CLI.
            set(&f, key, value);
            // Then config get and check recognize its type and persisted value.
            let output = ok(&f, &["config", "get", key]);
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), value);
            ok(&f, &["config", "check"]);
        }
        let before = std::fs::read(f.ctx.store.config_path()).unwrap();
        let output = cli(&f, &["config", "set", key, "not-a-boolean"]);
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(std::fs::read(f.ctx.store.config_path()).unwrap(), before);
    }
}

#[test]
fn config_check_rejects_string_instead_of_review_boolean() {
    for section in ["review", "projects.app.review"] {
        // Given syntactically valid TOML with the wrong policy value type.
        let f = fixture();
        std::fs::write(
            f.ctx.store.config_path(),
            format!("[{section}]\nrequire_cross_family = 'true'\n"),
        )
        .unwrap();
        // When config check reads the actual workspace file.
        let output = cli(&f, &["config", "check"]);
        // Then the CLI rejects the schema rather than treating a string as true.
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("require_cross_family"),
            "{output:?}"
        );
    }
}

#[test]
fn route_json_reports_default_inheritance_project_override_and_selector() {
    // Given one project inheriting policy and another overriding it.
    let f = fixture();
    let app = f.task("app", Status::Ready);
    let other = f.task("other", Status::Ready);
    assert_eq!(route(&f, &app.id)["require_cross_family"], false);
    assert_eq!(
        route(&f, &app.id)["review_policy_source"],
        "built-in default"
    );
    // When global and per-project values are changed through config set.
    set(&f, "review.require_cross_family", "true");
    set(&f, "projects.app.review.require_cross_family", "false");
    // Then false is an override, not an absent value, and sources explain both.
    let app_route = route(&f, &app.id);
    let other_route = route(&f, &other.id);
    assert_eq!(app_route["require_cross_family"], false);
    assert_eq!(app_route["review_policy_source"], "[projects.app.review]");
    assert_eq!(other_route["require_cross_family"], true);
    assert_eq!(other_route["review_policy_source"], "[review]");

    set(&f, "projects.app.review.reviewer", "cross-vendor");
    let selected = route(&f, &app.id);
    assert_eq!(selected["require_cross_family"], true);
    assert_eq!(selected["review_policy_source"], "[projects.app.review]");
    set(&f, "projects.app.review.reviewer", "author");
    ok(&f, &["route", &app.id, "--set-reviewer", "cross-vendor"]);
    let selected = route(&f, &app.id);
    assert_eq!(selected["require_cross_family"], true);
    assert_eq!(selected["review_policy_source"], "task reviewer:");
    assert_eq!(
        selected["review"]["reviewer"]["profile"],
        "different-vendor"
    );
}

#[derive(Clone, Copy, Debug)]
enum Target {
    Plan,
    Implementation,
}

fn prepared(f: &Fixture, target: Target) -> String {
    let task = f.task("app", Status::Ready);
    match target {
        Target::Plan => {
            // Seed only the historical planner identity, like plan_ops.rs.
            // No planner, bubblewrap dependency, or owner pane is needed.
            f.ctx
                .store
                .update(&task.id, |task| {
                    task.plan_runs.push(PlanRun {
                        agent_host: None,
                        execution: None,
                        route: None,
                        delivery: corral_core::inbox::DeliveryMode::Legacy,
                        inbox: Default::default(),
                        id: "D1".into(),
                        profile: "author".into(),
                        harness: "generic".into(),
                        model: Some("openai/gpt-fixture".into()),
                        vendor: Some("openai".into()),
                        effort: None,
                        spec_revision: corral_ops::plan_spec_revision(task)?,
                        started: jiff::Timestamp::now(),
                        ended: None,
                        herdr: None,
                        output_dir: f.temp.path().join("plan-output"),
                        submitted: None,
                        rounds: 0,
                        state: PlanRunState::Planning,
                        pending_reply: None,
                    });
                    Ok(())
                })
                .unwrap();
            let path = f.temp.path().join("plan.md");
            std::fs::write(&path, PLAN).unwrap();
            ok(f, &["plan", "submit", &task.id, path.to_str().unwrap()]);
        }
        Target::Implementation => {
            ok(f, &["dispatch", &task.id, "--profile", "author"]);
            // Candidate creation and ownership use actual CLI dispatch. Only
            // finishing is seeded: checkpoint/done policy is outside this test.
            f.ctx
                .store
                .update(&task.id, |task| {
                    let run = task.current_run_mut().unwrap();
                    run.outcome = Some(RunOutcome::Done);
                    run.ended = Some(jiff::Timestamp::now());
                    task.status = Status::Review;
                    Ok(())
                })
                .unwrap();
        }
    }
    task.id
}

fn launch(f: &Fixture, target: Target, id: &str, reviewer: Option<&str>) -> Output {
    let mut args = match target {
        Target::Plan => vec!["plan", "review", id],
        Target::Implementation => vec!["review-run", id],
    };
    if let Some(reviewer) = reviewer {
        args.extend(["--reviewer", reviewer]);
    }
    cli(f, &args)
}

fn assert_launch(
    f: &Fixture,
    target: Target,
    id: &str,
    reviewer: Option<&str>,
    expected: &str,
    accepted: bool,
) {
    let before = f.ctx.store.get(id).unwrap();
    // Inventory reads initialize the protocol file; rejection is allowed to
    // inspect inventory but must not create panes or submit launch commands.
    f.ctx.herdr.pane_list().unwrap();
    let state = f.temp.path().join("protocol.json");
    let protocol_before = std::fs::read(&state).ok();
    // When review is requested in a separate actual CLI process.
    let output = launch(f, target, id, reviewer);
    assert_eq!(
        output.status.success(),
        accepted,
        "{target:?}, {reviewer:?}: {output:?}"
    );
    let after = f.ctx.store.get(id).unwrap();
    // Then acceptance creates a dedicated reviewer pane; rejection creates none.
    if accepted {
        let pane = match target {
            Target::Plan => {
                let review = after.plans.last().unwrap().reviews.last().unwrap();
                assert_eq!(review.reviewer, expected);
                assert!(review.verdict.is_none());
                assert!(after.runs.is_empty());
                &review.herdr.as_ref().unwrap().pane_id
            }
            Target::Implementation => {
                let run = after.current_run().unwrap();
                let review = run.review.as_ref().unwrap();
                assert_eq!(review.reviewer, expected);
                let child = run.review_runs.last().unwrap();
                assert_eq!(child.agent, expected);
                assert_ne!(
                    child.herdr.as_ref().unwrap().pane_id,
                    run.herdr.as_ref().unwrap().pane_id
                );
                review.pane.as_ref().unwrap()
            }
        };
        let state = f.state();
        assert!(
            state["panes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["pane_id"] == *pane)
        );
        let before_submits = protocol_before
            .as_ref()
            .map(|bytes| {
                serde_json::from_slice::<Value>(bytes).unwrap()["submits"]
                    .as_u64()
                    .unwrap()
            })
            .unwrap_or(0);
        assert_eq!(state["submits"], before_submits + 1);
    } else {
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cross-family review is required"),
            "{output:?}"
        );
        assert_eq!(after.plans, before.plans);
        assert_eq!(after.runs, before.runs);
        assert_eq!(std::fs::read(&state).ok(), protocol_before);
    }
}

#[test]
fn default_and_off_allow_same_profile_and_effective_model_for_both_reviews() {
    for target in [Target::Plan, Target::Implementation] {
        for policy in [None, Some("false")] {
            for reviewer in [None, Some("author"), Some("same-model")] {
                // Given default/off policy and a same-profile or same-model reviewer.
                let f = fixture();
                if let Some(policy) = policy {
                    set(&f, "review.require_cross_family", policy);
                }
                let id = prepared(&f, target);
                assert_launch(
                    &f,
                    target,
                    &id,
                    reviewer,
                    reviewer.unwrap_or("author"),
                    true,
                );
            }
        }
    }
}

#[test]
fn required_policy_checks_vendor_for_configured_and_explicit_reviewers() {
    for target in [Target::Plan, Target::Implementation] {
        for explicit in [false, true] {
            for reviewer in ["author", "same-model", "same-vendor", "different-vendor"] {
                // Given global opt-in, including different models of the same vendor.
                let f = fixture();
                set(&f, "review.require_cross_family", "true");
                if !explicit {
                    set(&f, "review.reviewer", reviewer);
                }
                let id = prepared(&f, target);
                assert_launch(
                    &f,
                    target,
                    &id,
                    explicit.then_some(reviewer),
                    reviewer,
                    reviewer == "different-vendor",
                );
            }
        }
    }
}

#[test]
fn task_reviewer_override_obeys_family_policy_in_both_launch_paths() {
    for target in [Target::Plan, Target::Implementation] {
        for required in ["false", "true"] {
            for reviewer in ["author", "different-vendor"] {
                // Given a task reviewer distinct from the configured fallback.
                let f = fixture();
                set(&f, "review.require_cross_family", required);
                set(&f, "review.reviewer", "same-vendor");
                let id = prepared(&f, target);
                ok(&f, &["route", &id, "--set-reviewer", reviewer]);
                // When previewing and launching without a CLI override.
                let preview = route(&f, &id);
                assert_eq!(preview["review"]["reviewer"]["selection"], reviewer);
                assert_eq!(preview["review"]["reviewer"]["source"], "task_override");
                assert_eq!(preview["require_cross_family"], required == "true");
                assert_launch(
                    &f,
                    target,
                    &id,
                    None,
                    reviewer,
                    required == "false" || reviewer == "different-vendor",
                );
            }
        }
    }
}

#[test]
fn project_boolean_overrides_global_policy_in_both_review_launch_paths() {
    for target in [Target::Plan, Target::Implementation] {
        for (global, project, accepted) in [("true", "false", true), ("false", "true", false)] {
            // Given opposite values at global and project scope.
            let f = fixture();
            set(&f, "review.require_cross_family", global);
            set(&f, "projects.app.review.require_cross_family", project);
            let id = prepared(&f, target);
            assert_launch(&f, target, &id, Some("author"), "author", accepted);
        }
    }
}

#[test]
fn cross_vendor_selector_stays_opted_in_when_boolean_is_false() {
    for target in [Target::Plan, Target::Implementation] {
        for explicit in [false, true] {
            // Given explicit false and a cross-vendor selector.
            let f = fixture();
            set(&f, "review.require_cross_family", "false");
            set(&f, "review.reviewer", "cross-vendor");
            let id = prepared(&f, target);
            // A named override must not bypass the configured opt-in.
            assert_launch(
                &f,
                target,
                &id,
                explicit.then_some("author"),
                "different-vendor",
                !explicit,
            );
        }
    }
}

#[test]
fn required_policy_judges_bedrock_reviewers_by_model_family() {
    for target in [Target::Plan, Target::Implementation] {
        for explicit in [false, true] {
            // Given an OpenAI author: Bedrock gpt-oss shares its family, Bedrock Claude does not.
            for reviewer in ["bedrock-gpt", "bedrock-claude"] {
                let f = fixture();
                set(&f, "review.require_cross_family", "true");
                if !explicit {
                    set(&f, "review.reviewer", reviewer);
                }
                let id = prepared(&f, target);
                assert_launch(
                    &f,
                    target,
                    &id,
                    explicit.then_some(reviewer),
                    reviewer,
                    reviewer == "bedrock-claude",
                );
            }
        }
    }
}
