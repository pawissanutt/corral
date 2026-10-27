use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{OperatingAuthority, OperatingSelection, Status, Store};
use corral_ops::resources::ResourceProbe;
use corral_ops::{Ctx, PmWorkset};
use serde_json::{Value, json};

mod support;

#[derive(Debug)]
struct PinnedReadings;

impl ResourceProbe for PinnedReadings {
    fn sample(&self, store: &Store) -> anyhow::Result<corral_core::ResourceSnapshot> {
        // Exercise the real walk; pin only machine readings to admitting values.
        let mut snapshot = corral_ops::resources::SystemResourceProbe.sample(store)?;
        snapshot.load_one = 0.1;
        snapshot.load_per_core = 0.1;
        snapshot.mem_available = u64::MAX;
        snapshot.cpu_psi = Some(0.0);
        snapshot.mem_psi = Some(0.0);
        for disk in &mut snapshot.disks {
            disk.free_bytes = u64::MAX;
        }
        Ok(snapshot)
    }
}

fn fixture() -> (tempfile::TempDir, Ctx) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    ] {
        assert!(
            support::command("git", root.path())
                .arg("-C")
                .arg(&repo)
                .args(args)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let mut store = Store::init(root.path()).unwrap();
    store.config.agents.insert(
        "fake".into(),
        corral_core::AgentCmd {
            command: "true".into(),
        },
    );
    store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .default_profile = Some("fake".into());
    store.config.routing.rules.clear();
    store.config.limits.running = 8;
    store.config.limits.review = 2;
    store.save_config().unwrap();
    (root, corral_ops::resources::FixedResourceProbe::ctx(store))
}

fn grant(ctx: &mut Ctx, ids: &[&str]) {
    let selection = OperatingSelection {
        tasks: ids.iter().map(|id| (*id).to_owned()).collect(),
        authority: OperatingAuthority {
            dispatch: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let preview = ctx.store.preview_operating_policy(&selection).unwrap();
    ctx.store.config = ctx
        .store
        .confirm_operating_policy(
            &preview,
            "fixture owner",
            "2026-10-27T00:00:00Z".parse().unwrap(),
        )
        .unwrap();
}

#[test]
fn pm_option_identity_is_stable_until_a_target_write_commits() {
    // Given one executable delegated option.
    let (_root, mut ctx) = fixture();
    let task = ctx
        .store
        .create("Delegated", Some("app"), Status::Ready, "Finite contract")
        .unwrap();
    grant(&mut ctx, &[&task.id]);
    let identity = |ctx: &Ctx| {
        corral_ops::pm_workset(ctx)
            .unwrap()
            .actions
            .into_iter()
            .find(|action| action.action.task.as_deref() == Some(&task.id))
            .unwrap()
            .id
    };
    let before = identity(&ctx);
    assert_eq!(before, identity(&ctx));
    // When an otherwise irrelevant telemetry write commits on its target.
    ctx.store
        .update(&task.id, |task| {
            task.priority = "P1".into();
            Ok(())
        })
        .unwrap();
    // Then the old token is stale, while repeated unchanged reads stay stable.
    let after = identity(&ctx);
    assert_ne!(before, after);
    assert_eq!(after, identity(&ctx));
}

fn evaluate(ctx: &Ctx, automation: bool) -> Value {
    let workset = corral_ops::pm_workset(ctx).unwrap();
    if automation {
        json!({
            "workset":workset,
            "effective":ctx.store.effective_operating_policy().unwrap(),
            "summary":corral_ops::automation_summary(ctx).unwrap(),
            "dispatch":corral_ops::automatic_dispatch_status(ctx).unwrap(),
        })
    } else {
        serde_json::to_value(workset).unwrap()
    }
}

fn frozen(ctx: &Ctx) -> Ctx {
    Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    }
}

#[test]
fn pm_next_heads_match_the_full_task_hold_observation() {
    // Given a held task whose old observation is computed from the full record.
    let (_root, ctx) = fixture();
    let task = ctx.store.create("Held", None, Status::Ready, "").unwrap();
    let full = ctx
        .store
        .update(&task.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let bin = ctx.store.root().display().to_string();
    let actual = corral_ops::pm_next(&ctx).unwrap();
    let command_prefix = actual.options[0].command.split(" show ").next().unwrap();
    assert!(command_prefix.contains(&bin));
    let mut expected = corral_ops::pm_card(
        &corral_ops::PmObservation::Owner {
            task: full.id.clone(),
            reason: "Task has an explicit dispatch hold; owner clearance is required.".into(),
        },
        command_prefix,
    );
    expected
        .evidence
        .push(format!("task {}: {}", full.id, full.status.as_str()));
    for event in ctx.store.events_for(&full.id).unwrap().iter().rev().take(3) {
        expected.evidence.push(format!(
            "event {} {}: {}",
            event.ts, event.kind, event.detail
        ));
    }
    // When the bounded evaluation runs, its complete public card stays unchanged.
    // Then both the typed card and serialized output equal the full-task oracle.
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}

fn writer_between_reads(automation: bool) {
    // Given a dependency, a counted terminal task and a frozen pre-commit oracle.
    let (root, mut ctx) = fixture();
    let dependency = ctx
        .store
        .create("Dependency", Some("app"), Status::Done, "")
        .unwrap();
    let candidate = ctx
        .store
        .create("Candidate", Some("app"), Status::Ready, "")
        .unwrap();
    ctx.store
        .update(&candidate.id, |task| {
            task.depends_on.push(dependency.id.clone());
            Ok(())
        })
        .unwrap();
    grant(&mut ctx, &[&candidate.id, &dependency.id]);
    let oracle = frozen(&ctx);
    let expected = evaluate(&oracle, automation);
    let snapshot = frozen(&ctx);
    let writer = Store::open_unobserved(root.path()).unwrap();
    let dependency_id = dependency.id.clone();
    let socket = root.path().join("writer.sock");
    let writer_socket = socket.clone();
    // When another handle commits all four changes after the open-head read.
    let actual = PmWorkset::with_open_heads_hook(
        move || {
            writer
                .update(&dependency_id, |task| {
                    task.status = Status::Backlog;
                    Ok(())
                })
                .unwrap();
            writer
                .create("Count change", None, Status::Done, "")
                .unwrap();
            writer
                .save_record("operating-claims", "writer-barrier", &"after")
                .unwrap();
            writer
                .bind_herdr_session(&writer_socket, "writer-between-reads fixture")
                .unwrap();
        },
        || evaluate(&snapshot, automation),
    );
    // Then the result and each later component read stay on the old committed state.
    assert_eq!(actual, expected);
    assert_eq!(
        snapshot.store.head(&dependency.id).unwrap().status,
        Status::Done
    );
    assert_eq!(
        snapshot.store.board(None).unwrap().status_counts,
        oracle.store.board(None).unwrap().status_counts
    );
    assert_eq!(
        snapshot
            .store
            .record::<String>("operating-claims", "writer-barrier")
            .unwrap(),
        None
    );
    assert!(snapshot.store.herdr_session_binding().unwrap().is_none());
    let next = frozen(&ctx);
    assert_eq!(
        next.store.head(&dependency.id).unwrap().status,
        Status::Backlog
    );
    assert_ne!(
        next.store.board(None).unwrap().status_counts,
        oracle.store.board(None).unwrap().status_counts
    );
    assert_eq!(
        next.store
            .record::<String>("operating-claims", "writer-barrier")
            .unwrap(),
        Some("after".into())
    );
    assert_eq!(
        next.store.herdr_session_binding().unwrap().unwrap().socket,
        socket
    );
    assert_ne!(evaluate(&next, automation), expected);
}

#[test]
fn pm_next_keeps_one_view_when_a_writer_commits_between_components() {
    writer_between_reads(false);
}

#[test]
fn automation_status_keeps_one_view_when_a_writer_commits_between_components() {
    writer_between_reads(true);
}

fn measure(ctx: &Ctx, automation: bool) -> WorkSnapshot {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    evaluate(&frozen(ctx), automation);
    work::snapshot().since(&before)
}

fn history_budget(automation: bool) {
    let mut measurements = Vec::new();
    for multiplier in [1, 10] {
        // Given identical current inputs with ten times the terminal tasks and history.
        let (_root, mut ctx) = fixture();
        ctx.resource_probe = std::sync::Arc::new(PinnedReadings);
        let active = ctx
            .store
            .create("Active", None, Status::NeedsMe, "")
            .unwrap();
        ctx.store.update(&active.id, |task| {
            task.dispatch_hold = true;
            for n in 0..multiplier {
                let mut run: corral_core::Run = serde_json::from_value(json!({
                    "id":format!("R{n}"), "agent":"generic", "worktree":ctx.store.root().join("app"),
                    "branch":"main", "base":"main", "base_sha":"fixture",
                    "started":"2026-10-18T00:00:00Z", "ended":"2026-10-18T01:00:00Z", "outcome":"done"
                })).unwrap();
                run.summary = Some("Historical run".into());
                task.runs.push(run);
                task.plan_runs.push(serde_json::from_value(json!({
                    "id":format!("P{n}"), "profile":"generic", "harness":"generic",
                    "model":null, "vendor":null, "effort":null, "spec_revision":"fixture",
                    "started":"2026-10-18T00:00:00Z", "ended":"2026-10-18T01:00:00Z",
                    "herdr":null, "output_dir":ctx.store.root(), "submitted":null,
                    "rounds":0, "state":"approved", "pending_reply":null
                })).unwrap());
                task.comments.push(serde_json::from_value(json!({
                    "author":"owner", "at":"2026-10-18T00:00:00Z", "text":"Historical comment"
                })).unwrap());
                task.artifacts.push(serde_json::from_value(json!({
                    "run":format!("R{n}"), "kind":"evidence", "title":"Historical artifact",
                    "path":"artifacts/history.txt", "source":"history.txt",
                    "sha256":"fixture", "bytes":0, "added":"2026-10-18T00:00:00Z"
                })).unwrap());
            }
            let mut plan = task.plan_runs.last().unwrap().clone();
            plan.id = "current-plan".into();
            task.plan_runs.push(plan);
            let mut current = task.runs.last().unwrap().clone();
            current.id = "current".into();
            current.ended = None;
            current.outcome = None;
            task.runs.push(current);
            Ok(())
        }).unwrap();
        for n in 0..multiplier {
            ctx.store
                .create(&format!("Terminal {n}"), None, Status::Done, "")
                .unwrap();
            ctx.store
                .log_event(Some(&active.id), "historical", json!({"n":n}))
                .unwrap();
        }
        for _ in 0..3 {
            ctx.store
                .log_event(Some(&active.id), "current", json!({"anchor":"current"}))
                .unwrap();
        }
        // When measured warm, twice in the same process.
        measure(&ctx, automation);
        let measured = measure(&ctx, automation);
        // Then no owned consumer may hydrate a task or open a second view.
        measured.check_identity().unwrap();
        assert_eq!(
            measured.unattributed_hydrations(),
            0,
            "{:?}",
            measured.sites
        );
        assert_eq!(measured.get(Counter::ViewsOpened), 1);
        measurements.push(measured);
    }
    for counter in Counter::ALL {
        if counter != Counter::DecodedDocBytes {
            assert_eq!(
                measurements[0].get(counter),
                measurements[1].get(counter),
                "{counter:?}"
            );
        }
    }
    assert_eq!(
        measurements[0].unattributed_byte_remainder(),
        measurements[1].unattributed_byte_remainder()
    );
}

#[test]
fn pm_next_heads_history_budget() {
    history_budget(false);
}

#[test]
fn automation_status_heads_history_budget() {
    history_budget(true);
}
