use corral_core::{Profile, RouteSource, Status};
use corral_ops::{HarnessInfo, HarnessReport, ModelSource};

use crate::queue_support::Fixture;

fn fixture() -> Fixture {
    let mut f = crate::queue_fixture();
    let harness = f.ctx.store.config.harnesses.get_mut("generic").unwrap();
    harness.command = "echo {model_args}".into();
    harness.model_flag = Some("--model {model}".into());
    for (name, model) in [("opus", "opus-exact"), ("gpt", "gpt-exact")] {
        f.ctx.store.config.profiles.insert(
            name.into(),
            Profile {
                metadata: None,
                harness: "generic".into(),
                model: Some(model.into()),
                effort: None,
                args: vec![],
                vendor: None,
                escalate_to: None,
            },
        );
    }
    f.ctx.store.config.defaults.profile = Some("gpt".into());
    f.ctx.store.config.routing =
        toml::from_str("[[rules]]\nname = 'UI'\nkind = 'ui'\nprofile = 'opus'\n").unwrap();
    f.ctx.store.save_config().unwrap();
    catalog(&f, &["opus-exact", "gpt-exact"]);
    f
}

fn catalog(f: &Fixture, models: &[&str]) {
    let harnesses = [
        ("claude", "claude"),
        ("generic", "echo"),
        ("omo", "omo"),
        ("omp", "omp"),
    ]
    .into_iter()
    .map(|(key, program)| HarnessInfo {
        key: key.into(),
        program: Some(program.into()),
        path: None,
        version: None,
        models: models.iter().map(|m| (*m).into()).collect(),
        model_source: ModelSource::Listed,
        efforts: vec![],
        takes_effort: false,
        notes: vec![],
    })
    .collect();
    let report = HarnessReport {
        detected_at: jiff::Timestamp::now(),
        harnesses,
    };
    std::fs::write(
        f.ctx.store.state_dir().join(corral_ops::HARNESS_CACHE_FILE),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
}

#[test]
fn ui_routes_to_opus_and_backend_to_gpt_at_admission_and_stays_pinned() {
    let f = fixture();
    let ui = f.task("app", Status::Ready);
    let backend = f.task("app", Status::Ready);
    for (task, kind) in [(&ui, "ui"), (&backend, "backend")] {
        f.ctx
            .store
            .update(&task.id, |t| {
                t.kind = Some(kind.into());
                Ok(())
            })
            .unwrap();
        f.ok(&["queue", "add", &task.id]);
    }
    let ui = f.ctx.store.get(&ui.id).unwrap();
    let backend = f.ctx.store.get(&backend.id).unwrap();
    let ui_plan = ui.dispatch_plan.as_ref().unwrap();
    let backend_plan = backend.dispatch_plan.as_ref().unwrap();
    assert_eq!(ui_plan.route.profile, "opus");
    assert_eq!(ui_plan.route.model.as_deref(), Some("opus-exact"));
    assert!(matches!(
        ui_plan.route.route,
        RouteSource::Rule { index: 1, .. }
    ));
    assert_eq!(backend_plan.route.profile, "gpt");
    assert_eq!(backend_plan.route.model.as_deref(), Some("gpt-exact"));
    assert_eq!(backend_plan.route.route, RouteSource::Defaults);
    f.ctx
        .store
        .update(&ui.id, |t| {
            t.kind = Some("backend".into());
            Ok(())
        })
        .unwrap();
    f.ok(&["sync"]);
    assert_eq!(
        f.ctx
            .store
            .get(&ui.id)
            .unwrap()
            .current_run()
            .unwrap()
            .agent,
        "opus"
    );
}

#[test]
fn disappearing_pinned_model_is_visible_and_never_falls_back_at_claim() {
    let f = fixture();
    let task = f.task("app", Status::Ready);
    f.ok(&["queue", "add", &task.id, "--profile", "opus"]);
    catalog(&f, &["gpt-exact"]);
    f.ok(&["sync"]);
    let task = f.ctx.store.get(&task.id).unwrap();
    assert!(task.runs.is_empty());
    assert!(task.queued);
    assert_eq!(
        task.dispatch_plan.unwrap().route.model.as_deref(),
        Some("opus-exact")
    );
    let out = f.ok(&["queue", "list", "--json"]);
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        entries[0]["reason"]
            .as_str()
            .unwrap()
            .contains("not an exact discovered model")
    );
    assert_eq!(f.state()["creates"], 0);
}
