//! Stores of the actions-launch action budget (plan T-253 section 4, "Action budget" and the
//! budget fixture rules). Every variant of a kind shares the acted task's current inputs, its
//! named relations' heads and the selected action. Grown adds 10x earlier history on the acted
//! task and its relations, nested growth inside their current containers, 10x terminal tasks
//! and 10x history on every other task. Emitted adds 10x owner comments, non-approve findings
//! with notes and, on worker launch, the source run's obligations and a larger newest payload.
//! Ordinal families start at 9 or 90 earlier rows so new ids keep one width, filler events keep
//! event sequence numbers at one width, the clock is pinned, and the probe pins machine
//! readings over the real resource walk.
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use corral_core::{ObligationSource, OperatingActionKind, ResourceSnapshot, Status, Store};
use corral_herdr::Herdr;
use serde_json::json;

use super::action_budget_growth::{emit, grow, grown_by, land_grown};
use super::fixture::{self, Fx};
use super::{Path, aggregate};
use crate::resources::{PinnedReadingsProbe, ResourceProbe};

pub(super) const KINDS: [Kind; 10] = [
    Kind::Worker,
    Kind::Planner,
    Kind::PlanReviewPane,
    Kind::PlanReviewPm,
    Kind::AcceptPlan,
    Kind::AcceptPlanScopes,
    Kind::RepairLegacy,
    Kind::RepairInbox,
    Kind::AggregateCode,
    Kind::AggregateNoCode,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Worker,
    Planner,
    PlanReviewPane,
    PlanReviewPm,
    /// A plan of one scope, projected onto its own task.
    AcceptPlan,
    /// A two-scope plan decomposed into a tracker and children.
    AcceptPlanScopes,
    RepairLegacy,
    RepairInbox,
    AggregateCode,
    AggregateNoCode,
}

impl Kind {
    pub(super) fn name(self) -> String {
        format!("{self:?}")
    }

    pub(super) fn parse(name: &str) -> Kind {
        KINDS.into_iter().find(|k| k.name() == name).unwrap()
    }

    /// Whether a sync tick takes the action. Repair is measured through its real entry here
    /// and through the autopilot tick in T-333 (PM decision 2026-10-28); automatic aggregate
    /// acceptance has no tick entry, only `corral plan accept`'s automatic path.
    pub(super) fn ticked(self) -> bool {
        !matches!(
            self,
            Kind::RepairLegacy | Kind::RepairInbox | Kind::AggregateCode | Kind::AggregateNoCode
        )
    }

    pub(super) fn action(self) -> OperatingActionKind {
        match self {
            Kind::Worker => OperatingActionKind::Dispatch,
            Kind::Planner => OperatingActionKind::Plan,
            Kind::PlanReviewPane | Kind::PlanReviewPm => OperatingActionKind::PlanReview,
            Kind::AcceptPlan | Kind::AcceptPlanScopes => OperatingActionKind::AcceptPlan,
            Kind::RepairLegacy | Kind::RepairInbox => OperatingActionKind::Repair,
            Kind::AggregateCode | Kind::AggregateNoCode => OperatingActionKind::AcceptAggregate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Variant {
    Base,
    Grown,
    Emitted,
}

impl Variant {
    pub(super) fn name(self) -> &'static str {
        match self {
            Variant::Base => "base",
            Variant::Grown => "grown",
            Variant::Emitted => "emitted",
        }
    }

    /// Earlier rows per ordinal family: 9 and 90 keep new ids at one width.
    fn scale(self) -> usize {
        match self {
            Variant::Grown => 90,
            _ => 9,
        }
    }

    fn terminal(self) -> usize {
        match self {
            Variant::Grown => 100,
            _ => 10,
        }
    }
}

/// The pinned admitting probe, keeping every snapshot it returns.
#[derive(Debug)]
pub(super) struct RecordingProbe {
    inner: PinnedReadingsProbe,
    seen: Mutex<Vec<(ResourceSnapshot, super::action_budget_checks::Walk)>>,
    /// Samples taken on a live store: each opens one view, a pass snapshot's open none.
    live: std::sync::atomic::AtomicU64,
}

impl RecordingProbe {
    fn arc() -> Arc<RecordingProbe> {
        Arc::new(RecordingProbe {
            inner: PinnedReadingsProbe::admitting(),
            seen: Mutex::default(),
            live: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Each sample since the last call, with the walk the store expected of it at that time.
    pub(super) fn take(&self) -> Vec<(ResourceSnapshot, super::action_budget_checks::Walk)> {
        std::mem::take(&mut *self.seen.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The samples taken on live stores since the last call.
    pub(super) fn take_live(&self) -> u64 {
        self.live.swap(0, std::sync::atomic::Ordering::Relaxed)
    }
}

impl ResourceProbe for RecordingProbe {
    fn sample(&self, store: &Store) -> anyhow::Result<ResourceSnapshot> {
        // The approved sample path, PSI included; the fixture pins PSI at its source.
        let snapshot = self.inner.sample(store)?;
        if !store.is_snapshot() {
            self.live.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        // The expected walk of this sample, on the store as it stands now: counted on another
        // thread, so outside the measured pass's ledger.
        let at = corral_core::clock::now();
        let walk = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    corral_core::clock::pin_thread(Some(at));
                    super::action_budget_checks::store_walk(store).0
                })
                .join()
                .unwrap()
        });
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((snapshot.clone(), walk));
        Ok(snapshot)
    }
}

pub(super) struct Bench {
    pub(super) _temp: Option<tempfile::TempDir>,
    pub(super) root: PathBuf,
    pub(super) ctx: crate::Ctx,
    pub(super) probe: Arc<RecordingProbe>,
    pub(super) id: String,
    /// The aggregate attempt an aggregate acceptance accepts.
    pub(super) attempt: String,
}

const SCOPED_PLAN: &str = "---\ngoal: Bounded regression\nnon_goals: []\ndecisions: []\naffected_paths: [candidate.txt]\ncontracts: []\ndependencies:\n  - {task: a, write_scope: [a/], depends_on: [], acceptance: [{scenario: a, expected: verified}]}\n  - {task: b, write_scope: [b/], depends_on: [a], acceptance: [{scenario: b, expected: verified}]}\nacceptance: [{scenario: bounded, expected: verified}]\nverification: [check]\nowner_decisions: []\n---\n";

/// The PSI `(cpu, memory)` every fixture sample reads, pinned at its source.
pub(super) const PSI: (Option<f64>, Option<f64>) = (Some(0.25), Some(1.5));

/// The kind's acted task and its current inputs, before any growth.
fn setup(kind: Kind) -> (Fx, String, String) {
    match kind {
        Kind::AggregateCode | Kind::AggregateNoCode => {
            let (fx, id, attempt, _) = aggregate::scenario(kind == Kind::AggregateCode, true);
            (fx, id, attempt)
        }
        Kind::AcceptPlanScopes => {
            let (mut fx, id) = Path::AcceptPlan.setup();
            let source = fx.temp.path().join("scoped-plan.md");
            std::fs::write(&source, SCOPED_PLAN).unwrap();
            // The dependency is part of the plan contract: it exists before the plan is reviewed.
            dependency(&fx.ctx.store, &id);
            // A revised two-scope plan, reviewed and approved like the single-scope one.
            fx.ctx
                .store
                .update(&id, |t| {
                    t.status = Status::Running;
                    let revision = corral_core::plan_contract_revision(t)?;
                    let run = t.plan_runs.last_mut().unwrap();
                    run.spec_revision = revision;
                    run.state = corral_core::PlanRunState::Changes;
                    run.ended = None;
                    Ok(())
                })
                .unwrap();
            crate::plan_submit(&fx.ctx, &id, &source).unwrap();
            let plan = fx
                .ctx
                .store
                .get(&id)
                .unwrap()
                .plans
                .last()
                .unwrap()
                .id
                .clone();
            let review = crate::plan_review(&fx.ctx, &id, Some("rev")).unwrap();
            crate::plan_verdict(
                &fx.ctx,
                &id,
                &crate::PlanVerdict {
                    plan: &plan,
                    attempt: Some(&review.id),
                    verdict: corral_core::Verdict::Approve,
                    notes: "independently verified",
                },
            )
            .unwrap();
            fx.ctx
                .store
                .update(&id, |t| {
                    t.status = Status::Review;
                    Ok(())
                })
                .unwrap();
            let _ = &mut fx;
            (fx, id, plan)
        }
        _ => {
            let path = match kind {
                Kind::Worker => Path::Worker,
                Kind::Planner => Path::Planner,
                Kind::PlanReviewPane => Path::PlanReviewPane,
                Kind::PlanReviewPm => Path::PlanReviewPm,
                Kind::AcceptPlan => Path::AcceptPlan,
                Kind::RepairLegacy => Path::RepairLegacy,
                _ => Path::RepairInbox,
            };
            let (mut fx, id) = path.setup();
            if kind == Kind::PlanReviewPm {
                fx.ctx.store.config.review.reviewer = Some("pm".into());
                fx.ctx.store.save_config().unwrap();
            }
            (fx, id, String::new())
        }
    }
}

/// The grant with the scoped plan's exact implementation scopes.
fn grant_scopes(fx: &mut Fx, id: &str, plan: &str) {
    let _ = plan;
    let selection = corral_core::OperatingSelection {
        tasks: [id.to_owned()].into(),
        plan_scopes: [(id.to_owned(), ["a".into(), "b".into()].into())].into(),
        authority: corral_core::OperatingAuthority {
            dispatch: true,
            accept: true,
            plan_accept: true,
            aggregate_accept: false,
        },
        ..Default::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", fixture::now())
        .unwrap();
}

/// A worker launch's source: a launched run R1 with a checkpoint, then superseded; emitted
/// stores give it waits, a decision and a larger frozen payload.
fn source_run(fx: &Fx, id: &str, emitted: bool) {
    fx.start(id);
    if emitted {
        super::action_heads::waits(fx, id, 10);
        crate::register_decision(
            &fx.ctx,
            id,
            corral_core::DecisionRequest {
                version: 1,
                id: "release".into(),
                run: "R1".into(),
                question: "Which release branch? ".repeat(40),
                requested_action: "Choose a branch".into(),
                owner: "owner".into(),
                source: ObligationSource::Worker,
                deadline: None,
            },
            fixture::now(),
        )
        .unwrap();
        // Final obligations: a resolved wait and an answered decision are no current input.
        let receipt = |n: String, outcome, answer: Option<&str>| corral_core::ResumeReceipt {
            version: 1,
            id: n,
            run: "R1".into(),
            outcome,
            delivery: corral_core::ResumeDelivery::Acknowledged,
            observed_at: fixture::now(),
            answer: answer.map(str::to_owned),
            data: json!({}),
        };
        for i in 0..10 {
            crate::resume_run(
                &fx.ctx,
                id,
                crate::ResumeTarget::Wait(&format!("build-{i:03}")),
                receipt(
                    format!("wait-{i:03}"),
                    corral_core::ReceiptOutcome::Ready,
                    None,
                ),
                fixture::now(),
            )
            .unwrap();
        }
        crate::resume_run(
            &fx.ctx,
            id,
            crate::ResumeTarget::Decision("release"),
            receipt(
                "decision".into(),
                corral_core::ReceiptOutcome::Answered,
                Some("main"),
            ),
            fixture::now(),
        )
        .unwrap();
    }
    super::action_heads::checkpoint(fx, id);
    fx.ctx
        .store
        .update(id, |t| {
            let run = t.current_run_mut().unwrap();
            run.outcome = Some(corral_core::RunOutcome::Superseded);
            run.ended = Some(fixture::now());
            t.status = Status::Backlog;
            Ok(())
        })
        .unwrap();
}

/// A Done prerequisite of the acted task, whose head every launch check reads.
fn dependency(store: &Store, id: &str) -> String {
    let dep = store
        .create("Prerequisite quartz", Some("app"), Status::Done, "")
        .unwrap()
        .id;
    store
        .update(id, |t| {
            t.depends_on.push(dep.clone());
            Ok(())
        })
        .unwrap();
    dep
}

/// The named relations of the acted task: its dependencies, parent, tracker and members.
fn relations(store: &Store, id: &str) -> Vec<String> {
    let task = store.get(id).unwrap();
    let mut ids: Vec<String> = task.depends_on.clone();
    ids.extend(task.parent.clone());
    ids.extend(task.children.clone());
    // The execution plan's tracker, and the plan targets its backlinks and runs name.
    ids.extend(
        store
            .execution_plan(&task)
            .unwrap()
            .map(|tracker| tracker.id),
    );
    let backlinks = task.plan.iter().chain(
        task.runs
            .iter()
            .filter_map(|run| run.plan_target.as_ref().map(|t| &t.plan)),
    );
    for backlink in backlinks {
        if let Some((target, _)) = backlink.split_once('@') {
            ids.push(target.to_owned());
        }
    }
    ids.retain(|other| other != id);
    ids.sort();
    ids.dedup();
    ids
}

/// Task-less events nothing reads, padding every variant's event log to one sequence width.
fn filler(store: &Store) {
    let count = store.events().unwrap().len();
    let target = 10_000;
    assert!(count < target, "the fixture outgrew the filler width");
    let event = corral_core::Event {
        seq: 0,
        ts: fixture::now(),
        task: None,
        kind: "budget_filler".into(),
        actor: None,
        via: None,
        detail: json!({}),
    };
    let line = serde_json::to_string(&event).unwrap() + "\n";
    store
        .fixture_append_events(&line.repeat(target - count))
        .unwrap();
}

pub(super) fn build(kind: Kind, variant: Variant) -> Bench {
    corral_core::clock::pin_thread(Some(fixture::now()));
    crate::resources::pin_psi(Some(PSI));
    let (fx, id, attempt) = setup(kind);
    let store = fx.ctx.store.clone();
    let root = fx.temp.path().to_path_buf();
    if kind == Kind::Worker {
        source_run(&fx, &id, variant == Variant::Emitted);
    }
    if kind.ticked() && kind != Kind::AcceptPlanScopes {
        let before = corral_core::plan_contract_revision(&store.get(&id).unwrap()).unwrap();
        dependency(&store, &id);
        // A plan written against the task as it now stands: the dependency is in its contract.
        store
            .update(&id, |t| {
                let revision = corral_core::plan_contract_revision(t)?;
                for plan in t.plans.iter_mut().filter(|p| p.spec_revision == before) {
                    plan.spec_revision = revision.clone();
                }
                for run in t.plan_runs.iter_mut().filter(|r| r.spec_revision == before) {
                    run.spec_revision = revision.clone();
                }
                Ok(())
            })
            .unwrap();
    }
    if matches!(kind, Kind::Worker | Kind::Planner) {
        // Automatic admission takes open Backlog work: the tick admits and launches it.
        store
            .update(&id, |t| {
                t.status = Status::Backlog;
                Ok(())
            })
            .unwrap();
    }
    let scale = variant.scale();
    grow(&store, &root, &id, scale, true);
    let related = relations(&store, &id);
    for relation in &related {
        grow(&store, &root, relation, scale, false);
    }
    for (task, acted) in std::iter::once((&id, true)).chain(related.iter().map(|r| (r, false))) {
        grown_by(&store, task, scale, acted);
    }
    for relation in &related {
        land_grown(&store, relation);
    }
    if variant == Variant::Emitted {
        emit(&store, &id, scale, kind);
        if matches!(kind, Kind::RepairLegacy | Kind::RepairInbox) {
            // The emitted repair store really holds the repaired run's added obligations.
            let head = store.head(&id).unwrap();
            let run = head.current_run.as_ref().unwrap().id.clone();
            let obligations = head.emitted_obligations(&run).unwrap();
            assert!(obligations.waits.len() >= 10 && !obligations.decisions.is_empty());
        }
    }
    for n in 0..variant.terminal() {
        let task = store
            .create(
                &format!("Zirconium basalt {n:03}"),
                Some("app"),
                Status::Done,
                "",
            )
            .unwrap();
        grow(
            &store,
            &root,
            &task.id,
            if variant == Variant::Grown { 10 } else { 1 },
            false,
        );
    }
    // The grant binds the acted task's contract: it is confirmed once every input is final.
    let mut fx = fx;
    let attempt = match kind {
        // The grown specs need a fresh verification, as after any scope amendment.
        Kind::AggregateCode | Kind::AggregateNoCode => {
            aggregate::verified_attempt(&mut fx, &id, true)
        }
        Kind::AcceptPlanScopes => {
            grant_scopes(&mut fx, &id, &attempt);
            String::new()
        }
        _ => {
            fx.grant(&[&id]);
            attempt
        }
    };
    filler(&store);
    let probe = RecordingProbe::arc();
    let Fx { temp, mut ctx, .. } = fx;
    ctx.resource_probe = probe.clone();
    Bench {
        _temp: Some(temp),
        root,
        ctx,
        probe,
        id,
        attempt,
    }
}

/// Ticks with automation paused, so first sightings and observations are written before the
/// measured tick and the selected action is not taken; then automation resumes.
pub(super) fn settle(bench: &Bench) {
    if !bench.id.is_empty() && bench_ticks(bench) {
        bench.ctx.store.pause_operating_policy(true).unwrap();
        for _ in 0..2 {
            crate::sync_at(&bench.ctx, fixture::now()).unwrap();
        }
        bench.ctx.store.pause_operating_policy(false).unwrap();
    }
    bench.probe.take();
}

fn bench_ticks(bench: &Bench) -> bool {
    bench.attempt.is_empty()
}

/// The store `root` another process built and settled, opened afresh.
pub(super) fn reopen(root: &FsPath, id: &str, attempt: &str) -> Bench {
    corral_core::clock::pin_thread(Some(fixture::now()));
    let store = Store::open(root).unwrap();
    let herdr = Herdr::with_bin(
        root.join("engine").join("herdr"),
        Some("corral-test".into()),
    );
    let probe = RecordingProbe::arc();
    Bench {
        _temp: None,
        root: root.to_path_buf(),
        ctx: crate::Ctx {
            store,
            herdr,
            resource_probe: probe.clone(),
        },
        probe,
        id: id.to_owned(),
        attempt: attempt.to_owned(),
    }
}
