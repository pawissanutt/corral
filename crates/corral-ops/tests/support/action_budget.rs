//! Action budget of the actions-launch kinds (plan T-253 section 4, "Action budget"): each
//! automatic action runs through its real entry on a base store, a grown store (10x history on
//! the acted task and 10x terminal tasks) and an emitted store (base plus 10x owner comments),
//! and the work it counts must not follow that history.
use super::*;
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{CommentAuthor, ProfileSwitch, Run, Task, TaskComment};
use serde_json::json;

const SCALE: usize = 10;

/// Bytes per decoded document a written timestamp's fractional width may vary by.
const TIMESTAMP_NOISE: u64 = 2;

/// Entries whose new identifiers count earlier rows (`R<n>`, `D<n>`, `PR<n>`): their decoded
/// bytes differ by those digits, so only their decode counts are compared.
fn ordinal_ids(path: Path) -> bool {
    matches!(
        path,
        Path::Worker | Path::Planner | Path::PlanReviewPane | Path::PlanReviewPm
    )
}

fn historic_run(index: usize, worktree: &std::path::Path) -> Run {
    serde_json::from_value(json!({
        "id": format!("H{index:03}"), "agent": "historic", "harness": "historic",
        "worktree": worktree.join(format!("history-{index:03}")), "branch": "corral/history",
        "base": "main", "base_sha": "0".repeat(40), "started": fixture::now(),
        "ended": fixture::now(), "outcome": "superseded",
        "review": {"reviewer": "historic", "sha": "0".repeat(40), "verdict": "approve",
                   "at": fixture::now()},
    }))
    .unwrap()
}

/// 10x earlier history on `id` that no current input reads: earlier coding runs, planning runs
/// and plans before the current ones, earlier aggregate attempts, nested growth in the current
/// coding run (earlier approved review children, profile switches) and in the current plan
/// (earlier approved reviews by an unpolicied reviewer), events of every run-scoped kind on the
/// earlier runs, and 10x terminal tasks.
fn grow(fx: &Fx, id: &str, scale: usize) {
    let store = &fx.ctx.store;
    let root = fx.temp.path().to_path_buf();
    store
        .update(id, |t| {
            // Earlier coding runs go before the current one, which stays.
            if !t.runs.is_empty() {
                let earlier: Vec<Run> = (0..scale).map(|i| historic_run(i, &root)).collect();
                t.runs.splice(0..0, earlier);
            }
            if let Some(current) = t.plan_runs.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut run = current.clone();
                    run.id = format!("HD{i:03}");
                    run.state = corral_core::PlanRunState::Cancelled;
                    run.ended = Some(fixture::now());
                    run.herdr = None;
                    run.inbox = Default::default();
                    run
                });
                let at = t.plan_runs.len() - 1;
                t.plan_runs.splice(at..at, earlier.collect::<Vec<_>>());
            }
            if let Some(current) = t.plans.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut plan = current.clone();
                    plan.id = format!("HP{i:03}");
                    plan.reviews.clear();
                    plan.approval = None;
                    plan
                });
                let at = t.plans.len() - 1;
                t.plans.splice(at..at, earlier.collect::<Vec<_>>());
                let plan = t.plans.last_mut().unwrap();
                if let Some(template) = plan.reviews.last().cloned() {
                    let reviews = (0..scale).map(|i| {
                        let mut review = template.clone();
                        review.id = format!("HPR{i:03}");
                        review.reviewer = "historic".into();
                        review.verdict = Some(corral_core::Verdict::Approve);
                        review.notes = None;
                        review.herdr = None;
                        review.pending_reply = None;
                        review.inbox = Default::default();
                        review
                    });
                    plan.reviews.splice(0..0, reviews.collect::<Vec<_>>());
                }
            }
            if let Some(current) = t.aggregate_attempts.last().cloned() {
                let earlier = (0..scale).map(|i| {
                    let mut attempt = current.clone();
                    attempt.id = format!("HA{i:03}");
                    attempt.acceptance = None;
                    attempt
                });
                let at = t.aggregate_attempts.len() - 1;
                t.aggregate_attempts
                    .splice(at..at, earlier.collect::<Vec<_>>());
            }
            // Nested history goes before the current child and the last switch, which stay.
            if let Some(run) = t
                .runs
                .last_mut()
                .filter(|run| !run.id.starts_with('H') && !run.review_runs.is_empty())
            {
                let children = (0..scale).map(|i| {
                    let mut child = historic_run(i, &root);
                    child.id = format!("{}-review-h{i:03}", run.id);
                    child.outcome = Some(corral_core::RunOutcome::Done);
                    child
                });
                run.review_runs.splice(0..0, children.collect::<Vec<_>>());
            }
            if let Some(run) = t.runs.last_mut().filter(|run| !run.switches.is_empty()) {
                let switches = (0..scale).map(|i| ProfileSwitch {
                    from: format!("historic-{i}"),
                    to: format!("historic-{}", i + 1),
                    reason: "earlier switch".into(),
                });
                run.switches.splice(0..0, switches.collect::<Vec<_>>());
            }
            Ok(())
        })
        .unwrap();
    for i in 0..scale {
        let run = format!("H{i:03}");
        for kind in [
            "fixture_history",
            "dispatch_policy",
            "lessons_selected",
            "lessons_applied",
            "gate_started",
            "review_started",
        ] {
            store
                .log_event(
                    Some(id),
                    kind,
                    json!({"run": run, "action": "dispatch", "lessons": [], "pane": "h"}),
                )
                .unwrap();
        }
        let done = store
            .create(&format!("Zirconium basalt {i}"), None, Status::Done, "")
            .unwrap();
        store
            .update(&done.id, |t| {
                t.runs.push(historic_run(i, &root));
                Ok(())
            })
            .unwrap();
    }
}

/// Base plus 10x owner comments on the acted task: the Owner comments section shows them.
fn emit(fx: &Fx, id: &str) {
    fx.ctx
        .store
        .update(id, |t| {
            for i in 0..SCALE {
                t.comments.push(TaskComment {
                    at: fixture::now(),
                    author: CommentAuthor::Owner,
                    text: format!("Owner note {i}"),
                });
            }
            Ok(())
        })
        .unwrap();
}

/// The action through its real automatic entry, its inputs read before counting starts.
/// A worker launch's previous, superseded run: a current input every variant holds, so the
/// grown store has earlier runs before it.
fn prepare(path: Path, fx: &Fx, id: &str) {
    if matches!(path, Path::Worker) {
        let worktree = fx.temp.path().to_path_buf();
        fx.ctx
            .store
            .update(id, |t| {
                let mut previous = historic_run(SCALE, &worktree);
                previous.id = "R1".into();
                previous.agent = "fake".into();
                previous.harness = Some("fake".into());
                t.runs.push(previous);
                Ok(())
            })
            .unwrap();
    }
}

fn act(path: Path, fx: &mut Fx, id: &str) -> WorkSnapshot {
    if matches!(path, Path::PlanReviewPm) {
        fx.ctx.store.config.review.reviewer = Some("pm".into());
        fx.ctx.store.save_config().unwrap();
    }
    fx.grant(&[id]);
    let repair = matches!(path, Path::RepairLegacy | Path::RepairInbox).then(|| {
        let task: Task = fx.ctx.store.get(id).unwrap();
        let run = task.current_run().unwrap();
        crate::integration::RepairKey::new(&task, run, "regression").unwrap()
    });
    let ctx = fx.ctx.clone();
    measure(|| match &repair {
        Some(key) => {
            crate::dispatch::auto_request_changes(
                &ctx,
                id,
                "repair regression",
                1,
                AutopilotState {
                    stage: AutoStage::Gating,
                    reason: None,
                    sha: None,
                    at: fixture::now(),
                },
                key,
            )
            .unwrap();
        }
        None => path
            .enter(&ctx, id, true)
            .unwrap_or_else(|e| panic!("{path:?}: {e:#}")),
    })
}

fn measure(call: impl FnOnce()) -> WorkSnapshot {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    call();
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    delta
}

fn hydration_sites(delta: &WorkSnapshot) -> Vec<String> {
    delta
        .sites
        .iter()
        .filter(|site| site.followup.is_none())
        .map(|site| format!("{}:{}", site.location.file(), site.location.line()))
        .collect()
}

/// Every unattributed cell the budget compares, decoded bytes excluded.
fn unattributed(delta: &WorkSnapshot) -> Vec<(&'static str, u64)> {
    Counter::DECODES
        .iter()
        .chain(&[Counter::ViewsOpened, Counter::Processes, Counter::FileStats])
        .map(|c| (c.name(), delta.get(*c)))
        .collect()
}

fn compare(label: &str, base: &WorkSnapshot, other: &WorkSnapshot, bytes: bool) -> Vec<String> {
    let mut failures = Vec::new();
    for (name, snapshot) in [("base", base), (label, other)] {
        let sites = hydration_sites(snapshot);
        if !sites.is_empty() || snapshot.unattributed_hydrations() != 0 {
            failures.push(format!("{name}: counted hydrations at {sites:?}"));
        }
    }
    if unattributed(base) != unattributed(other) {
        failures.push(format!(
            "{label}: unattributed cells {:?} != base {:?}",
            unattributed(other),
            unattributed(base)
        ));
    }
    // Timestamps written during the action serialize their fraction without trailing zeros, so
    // each decoded document may differ by a few bytes between runs; decode counts are exact.
    let decodes: u64 = Counter::DECODES.iter().map(|c| base.get(*c)).sum();
    let noise = TIMESTAMP_NOISE * decodes;
    if bytes
        && base
            .unattributed_byte_remainder()
            .abs_diff(other.unattributed_byte_remainder())
            > noise
    {
        failures.push(format!(
            "{label}: unattributed byte remainder {} != base {} (beyond {noise} bytes of timestamp widths)",
            other.unattributed_byte_remainder(),
            base.unattributed_byte_remainder()
        ));
    }
    if (base.own_record_rows(), base.own_record_bytes())
        != (other.own_record_rows(), other.own_record_bytes())
    {
        failures.push(format!("{label}: own-record cells changed"));
    }
    failures
}

fn emits_comments(path: Path) -> bool {
    matches!(
        path,
        Path::Worker | Path::Planner | Path::PlanReviewPane | Path::PlanReviewPm
    )
}

#[test]
fn action_budget_actions_launch_kinds_do_not_follow_history() {
    let mut failures = Vec::new();
    for path in [
        Path::Worker,
        Path::Planner,
        Path::RepairLegacy,
        Path::RepairInbox,
        Path::PlanReviewPane,
        Path::PlanReviewPm,
        Path::AcceptPlan,
    ] {
        let (mut base, id) = path.setup();
        prepare(path, &base, &id);
        grow(&base, &id, 1);
        let base = act(path, &mut base, &id);
        let (mut grown, id) = path.setup();
        prepare(path, &grown, &id);
        grow(&grown, &id, SCALE);
        let grown = act(path, &mut grown, &id);
        let (mut emitted, id) = path.setup();
        prepare(path, &emitted, &id);
        grow(&emitted, &id, 1);
        emit(&emitted, &id);
        let emitted = act(path, &mut emitted, &id);
        let bytes = !ordinal_ids(path);
        failures.extend(
            compare("grown", &base, &grown, bytes)
                .into_iter()
                .map(|f| format!("{path:?} {f}")),
        );
        if (base.emitted_rows(), base.emitted_doc_bytes())
            != (grown.emitted_rows(), grown.emitted_doc_bytes())
        {
            failures.push(format!("{path:?} grown: emitted cells changed"));
        }
        failures.extend(
            compare("emitted", &base, &emitted, bytes)
                .into_iter()
                .map(|f| format!("{path:?} {f}")),
        );
        let added = if emits_comments(path) { SCALE } else { 0 };
        let added = u64::try_from(added).unwrap();
        if emitted.emitted_rows() != base.emitted_rows() + added {
            failures.push(format!(
                "{path:?} emitted: emitted_rows {} != base {} + {added}",
                emitted.emitted_rows(),
                base.emitted_rows()
            ));
        }
    }
    for code in [false, true] {
        let run = |grown: bool| {
            let (fx, id, attempt, _) = aggregate::scenario(code, true);
            grow(&fx, &id, if grown { SCALE } else { 1 });
            measure(|| {
                crate::plan_accept(&fx.ctx, &id, &attempt, true).unwrap();
            })
        };
        let base = run(false);
        let grown = run(true);
        failures.extend(
            compare("grown", &base, &grown, true)
                .into_iter()
                .map(|f| format!("aggregate code={code} {f}")),
        );
    }
    assert!(
        failures.is_empty(),
        "action budget exceeded:\n  {}",
        failures.join("\n  ")
    );
}
