//! op-receipts: the F1 sweep and its fault-injection points, the two-phase gate settled from
//! attempt evidence with real SIGKILLed executors, wakes, the PM journal and question refusals.
#![cfg(unix)]

use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;

use corral_core::agent::{
    self as core_agent, AgentQuestion, AgentRole, ContainerRef, Executor, ReceiptState,
    RequestBinding, RequestIdentity, RequestOrigin, RequestReceipt, WakeState, paths, pm_state,
};
use corral_core::{
    CriteriaManifest, Criterion, EvidenceObservation, ObservationStatus, Run, RunOutcome, Status,
    Store, SubmissionDisposition, Task, Verdict,
};
use corral_herdr::Herdr;
use serde_json::{Value, json};

use super::effects::{self, Receipted, Report};
use super::{BindTarget, fence_generation, open_generation, tool_request_id};
use crate::Ctx;
use crate::review_budget::{AttemptState, Kind};

const FAKE: &str = r#"#!/usr/bin/env bash
cd "$(dirname "$0")" || exit 1
printf '%s\n' "$*" >> calls.log
if [ "${1:-}" = "--session" ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create"|"tab create")
    n=1
    while [ -f "created-$n" ]; do n=$((n+1)); done
    touch "created-$n" "w1:p$n"
    ok "{\"workspace\":{\"workspace_id\":\"w1\"},\"root_pane\":{\"workspace_id\":\"w1\",\"tab_id\":\"w1:t$n\",\"pane_id\":\"w1:p$n\"}}" ;;
  "workspace list") ok '{"workspaces":[]}' ;;
  "pane list") ok '{"panes":[{"pane_id":"w0:p1"}]}' ;;
  "pane get")
    if [ -f "$3" ]; then ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}"; else ok '{}'; fi ;;
  "pane close") if [ -f fail-close ]; then printf '{"error":{"code":"failed","message":"close failed"}}\n'; exit 1; fi; rm -f "$3"; ok '{}' ;;
  "pane read")
    if [ -f fail-read ]; then printf '{"error":{"code":"failed","message":"fixture transport failure"}}\n'; exit 1; fi
    if [ -f "$3" ]; then ok '{"text":"reviewer screen"}'; else printf '{"error":{"code":"pane_not_found","message":"gone"}}\n'; exit 1; fi ;;
  *) ok '{}' ;;
esac
"#;
const SUMMARY: &str = "Verified: fixture\nNot verified: none\n";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().into()
}

fn ctx_at(root: &Path) -> Ctx {
    Ctx {
        resource_probe: crate::resources::FixedResourceProbe::arc(),
        store: Store::discover(root).unwrap(),
        herdr: Herdr::with_bin(root.join("herdr"), None),
    }
}

fn run(id: &str, app: &Path, pane: &str) -> Run {
    let herdr = Herdr::with_bin(app.parent().unwrap().join("herdr"), None);
    serde_json::from_value(json!({
        "id": id, "agent": "pi", "worktree": app, "branch": "main", "base": "main",
        "base_sha": git(app, &["rev-parse", "HEAD"]), "started": "2026-01-01T00:00:00Z",
        "herdr": {"workspace_id": "w0", "pane_id": pane, "socket": herdr.socket_path().unwrap()},
    }))
    .unwrap()
}

struct Fx {
    tmp: tempfile::TempDir,
    ctx: Ctx,
    id: String,
}

fn fixture(gate: bool) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-qb", "main"]);
    for (key, value) in [("user.name", "t"), ("user.email", "t@example.invalid")] {
        git(&app, &["config", key, value]);
    }
    std::fs::write(app.join("tracked"), "initial\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "initial"]);
    let bin = tmp.path().join("herdr");
    std::fs::write(&bin, FAKE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.limits.review = 4;
    store.config.limits.count_reviews = false;
    store.config.limits.count_gates = false;
    store.config.dispatch.paused = true;
    if gate {
        store.config.review.gate = Some(vec!["true".into()]);
    }
    let project = serde_json::from_value(json!({"path": "app"})).unwrap();
    store.config.projects.insert("app".into(), project);
    store.save_config().unwrap();
    let ctx = ctx_at(tmp.path());
    let task = ctx
        .store
        .create("Pi", Some("app"), Status::Running, "spec")
        .unwrap();
    let r1 = run("R1", &app, "w0:p1");
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(r1);
            Ok(())
        })
        .unwrap();
    Fx {
        tmp,
        ctx,
        id: task.id,
    }
}

fn origin(
    fx: &Fx,
    container: ContainerRef,
    generation: u32,
    call: &str,
    op: &str,
) -> RequestOrigin {
    let (role, task, run, review, callback) = match &container {
        ContainerRef::Worker { task, run } => {
            (AgentRole::Worker, Some(task), Some(run), None, None)
        }
        ContainerRef::Reviewer { task, run, review } => (
            AgentRole::Reviewer,
            Some(task),
            Some(run),
            Some(review),
            Some("g1:c1"),
        ),
        ContainerRef::Pm => (AgentRole::Pm, None, None, None, None),
        _ => panic!("planning and assistant containers do not execute implementation effects"),
    };
    let callback = callback.map(String::from);
    RequestOrigin {
        request_id: tool_request_id("s1", call),
        op: op.into(),
        payload_sha256: paths::sha256_hex(call.as_bytes()),
        identity: RequestIdentity {
            store: paths::store_id(&fx.ctx.store),
            project: "app".into(),
            role,
            task: task.cloned(),
            run: run.cloned(),
            review: review.cloned(),
            callback: callback.clone(),
            assistant: None,
            session_id: "s1".into(),
            generation,
            turn: None,
        },
        binding: RequestBinding {
            container,
            generation,
            callback,
            settles_turn: None,
        },
    }
}

impl Fx {
    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    fn worker(&self) -> ContainerRef {
        ContainerRef::Worker {
            task: self.id.clone(),
            run: "R1".into(),
        }
    }

    fn bind(&self) -> u32 {
        let target = BindTarget::Worker {
            task: self.id.clone(),
            run: "R1".into(),
        };
        open_generation(&self.ctx.store, &target, "w0:p1")
            .unwrap()
            .generation
    }

    fn call(&self, generation: u32, call: &str, op: &str) -> RequestOrigin {
        origin(self, self.worker(), generation, call, op)
    }

    /// Turns the task into a Review with a bound review child and returns its verdict request.
    fn reviewer(&self) -> RequestOrigin {
        let child = run("R1-review1", &self.root().join("app"), "w0:p2");
        self.ctx
            .store
            .update(&self.id, |t| {
                t.status = Status::Review;
                t.runs[0].outcome = Some(RunOutcome::Done);
                t.runs[0].review_runs.push(child);
                Ok(())
            })
            .unwrap();
        let (task, run, review) = (self.id.clone(), "R1".to_owned(), "R1-review1".to_owned());
        let target = BindTarget::Reviewer {
            task: task.clone(),
            run: run.clone(),
            review: review.clone(),
            callback: "g1:c1".into(),
        };
        let n = open_generation(&self.ctx.store, &target, "w0:p2")
            .unwrap()
            .generation;
        origin(
            self,
            ContainerRef::Reviewer { task, run, review },
            n,
            "verdict-1",
            "verdict",
        )
    }

    fn receipt(&self, o: &RequestOrigin) -> Option<RequestReceipt> {
        let task = self.task();
        let state = core_agent::container_state(&task, &o.binding.container)?;
        state.receipt(&o.request_id).cloned()
    }

    fn calls(&self, prefix: &str) -> usize {
        let log = std::fs::read_to_string(self.root().join("calls.log")).unwrap_or_default();
        log.lines().filter(|l| l.starts_with(prefix)).count()
    }

    /// Event lines of `kind` carrying the request ID, and how many distinct requests they name.
    fn events(&self, kind: &str, request: &str) -> (usize, usize) {
        let events = self.ctx.store.events().unwrap();
        let tagged: Vec<_> = events
            .iter()
            .filter(|e| e.kind == kind && e.detail["request_id"] == request)
            .collect();
        let distinct: std::collections::HashSet<_> = tagged
            .iter()
            .map(|e| e.detail["request_id"].to_string())
            .collect();
        (tagged.len(), distinct.len())
    }

    fn ask(&self, container: &ContainerRef) -> String {
        self.ask_in(container, "open")
    }

    fn ask_in(&self, container: &ContainerRef, state: &str) -> String {
        let key = format!("{}/Q1", container.key());
        let question: AgentQuestion = serde_json::from_value(json!({
            "version": 1, "key": key, "revision": 1, "kind": "clarification",
            "asked": {"request_id": "rq-q", "payload_sha256": "00", "session_id": "s1",
                      "generation": 1, "at": "2026-01-02T00:00:00Z"},
            "blocking": false, "text": "Which?", "state": state,
            "wake": {"route": "none", "key": "qwake", "state": "pending"},
        }))
        .unwrap();
        let update = |t: &mut Task| {
            let state = core_agent::container_state_mut(t, container).unwrap();
            state.questions.push(question);
            Ok(())
        };
        self.ctx.store.update(&self.id, update).unwrap();
        key
    }
}

/// Runs `op` with a hook that panics the first time `point` is reached; true when it crashed.
fn crash_at<T>(point: &str, op: impl FnOnce() -> T) -> bool {
    let armed = Rc::new(Cell::new(true));
    let (name, flag) = (point.to_owned(), armed.clone());
    super::receipts::FAILPOINT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |reached: &str| {
            if reached == name && flag.replace(false) {
                panic!("injected crash at {reached}");
            }
        }));
    });
    let crashed = catch_unwind(AssertUnwindSafe(op)).is_err();
    super::receipts::FAILPOINT.with(|hook| *hook.borrow_mut() = None);
    crashed && !armed.get()
}

type SweptOp = dyn Fn(&Ctx, &RequestOrigin) -> anyhow::Result<Receipted>;

/// One canonical transition and one committed receipt after a crash at every point and a
/// replay in a fresh Ctx; owed effects run until their flag clears, and their event lines
/// carry the request ID, so readers count them once.
fn fault_matrix(
    setup: &dyn Fn(&Fx) -> RequestOrigin,
    op: &SweptOp,
    count: &dyn Fn(&Fx, &Task) -> usize,
    event: &str,
) {
    for point in [
        "swept_before_commit",
        "swept_after_commit",
        "swept_after_post",
        "swept_before_ack",
    ] {
        let fx = fixture(false);
        let o = setup(&fx);
        assert!(
            crash_at(point, || op(&fx.ctx, &o)),
            "{event}: {point} was never reached"
        );
        let replay = op(&ctx_at(fx.root()), &o).unwrap();
        assert_eq!(
            replay.replayed,
            point != "swept_before_commit",
            "{event} at {point}"
        );
        let task = fx.task();
        assert_eq!(
            count(&fx, &task),
            1,
            "{event} at {point}: exactly one transition"
        );
        let stored = fx.receipt(&o).expect("one committed receipt");
        assert_eq!(
            (stored.state, stored.post_pending.len()),
            (ReceiptState::Committed, 0)
        );
        assert_eq!(replay.receipt.request_id, stored.request_id);
        let (lines, distinct) = fx.events(event, &o.request_id);
        assert!(
            lines >= 1 && distinct == 1,
            "{event} at {point}: {lines} lines"
        );
    }
}

fn wait_request() -> corral_core::WaitRequest {
    serde_json::from_value(json!({
        "version": 1, "id": "w1", "run": "R1", "condition": "CI green", "owner": "ci",
        "source": "worker", "deadline": null, "handle": {"kind": "channel", "channel_id": "ci"},
    }))
    .unwrap()
}

fn bound(fx: &Fx, call: &str, op: &str) -> RequestOrigin {
    let generation = fx.bind();
    fx.call(generation, call, op)
}

#[test]
fn wait_artifact_checkpoint_done_and_verdict_survive_every_crash_point() {
    let wait: &SweptOp = &|ctx, o| {
        let now = jiff::Timestamp::now();
        crate::obligations::register_wait_request(
            ctx,
            o.identity.task.as_deref().unwrap(),
            wait_request(),
            now,
            o,
        )
    };
    let waits = |_: &Fx, t: &Task| {
        let wakes = &t.runs[0].agent_host.as_ref().unwrap().wakes;
        let armed = wakes.len() == 1 && wakes[0].state == WakeState::Armed;
        t.runs[0].obligations.waits.len() * usize::from(armed)
    };
    fault_matrix(
        &|fx| bound(fx, "wait-1", "wait"),
        wait,
        &waits,
        "wait_registered",
    );

    let artifact: &SweptOp = &|ctx, o| {
        let file = ctx.store.root().join("proof.log");
        std::fs::write(&file, "proof\n").unwrap();
        let id = o.identity.task.as_deref().unwrap();
        crate::artifacts::add_artifact_request(ctx, id, &file, None, Some("Proof"), o)
    };
    let proofs = |fx: &Fx, t: &Task| {
        let entries = t.artifacts.iter().filter(|a| a.title == "Proof").count();
        entries * usize::from(blobs(fx, "-proof.log") == 1)
    };
    fault_matrix(
        &|fx| bound(fx, "artifact-1", "artifact"),
        artifact,
        &proofs,
        "artifact",
    );

    let checkpoint: &SweptOp = &|ctx, o| {
        let id = o.identity.task.as_deref().unwrap();
        let input = checkpoint_input(ctx, id);
        crate::progress::save_checkpoint_request(ctx, id, &input, o)
    };
    let saved = |fx: &Fx, t: &Task| {
        t.runs[0].checkpoints.len() * usize::from(blobs(fx, "-checkpoint-1.json") == 1)
    };
    fault_matrix(
        &|fx| bound(fx, "checkpoint-1", "checkpoint"),
        checkpoint,
        &saved,
        "checkpoint_saved",
    );

    let done: &SweptOp = &|ctx, o| {
        let report = Report {
            origin: o,
            gate_ref: None,
            contract_sha256: Some("c0".into()),
        };
        crate::review::done_request(
            ctx,
            o.identity.task.as_deref().unwrap(),
            SUMMARY,
            None,
            &report,
        )
    };
    let finished = |_: &Fx, t: &Task| {
        let diffs = t
            .artifacts
            .iter()
            .filter(|a| a.kind == corral_core::ArtifactKind::Diff)
            .count();
        let reports = &t.runs[0].agent_host.as_ref().unwrap().reports;
        let reported = reports.len() == 1 && reports[0].contract_sha256.as_deref() == Some("c0");
        usize::from(t.status == Status::Review && t.runs[0].outcome == Some(RunOutcome::Done))
            * usize::from(reported)
            * diffs
    };
    fault_matrix(&|fx| bound(fx, "done-1", "done"), done, &finished, "done");
    let audited = |fx: &Fx| {
        let observation = criteria(fx, true);
        crate::completion::record_evidence(&fx.ctx, &fx.id, &observation).unwrap();
        bound(fx, "done-1", "done")
    };
    let audits = |fx: &Fx, t: &Task| {
        let audit = |a: &&corral_core::Artifact| a.title == "Completion audit" && a.run == "R1";
        finished(fx, t) * t.artifacts.iter().filter(audit).count()
    };
    fault_matrix(&audited, done, &audits, "done");

    let verdict: &SweptOp = &|ctx, o| {
        let report = Report {
            origin: o,
            gate_ref: None,
            contract_sha256: None,
        };
        let id = o.identity.task.as_deref().unwrap();
        crate::reviewer::verdict_request(ctx, id, Verdict::Approve, "Looks right.", None, &report)
    };
    let recorded = |_: &Fx, t: &Task| {
        let notes = t
            .artifacts
            .iter()
            .filter(|a| a.title.starts_with("Review ("))
            .count();
        let child = t.runs[0].review_runs[0].agent_host.as_ref().unwrap();
        let reported = child.reports.len() == 1 && child.binding.closing;
        usize::from(
            t.runs[0]
                .review
                .as_ref()
                .is_some_and(|r| r.verdict.is_some()),
        ) * usize::from(reported)
            * notes
    };
    fault_matrix(&|fx| fx.reviewer(), verdict, &recorded, "verdict");
}

/// Snapshot files of R1 whose names end with `suffix`.
fn blobs(fx: &Fx, suffix: &str) -> usize {
    let dir = fx.ctx.store.artifacts_dir(&fx.id, "R1");
    let names = std::fs::read_dir(dir).into_iter().flatten().flatten();
    names
        .filter(|e| e.file_name().to_string_lossy().ends_with(suffix))
        .count()
}

/// Registers criterion C1 (audited or not) and returns a passing no-gate observation of R1.
fn criteria(fx: &Fx, audited: bool) -> EvidenceObservation {
    let manifest = CriteriaManifest {
        version: 1,
        spec_digest: crate::completion::completion_spec_digest(&fx.task()).unwrap(),
        reviewed_by: "owner".into(),
        audited,
        require_gate: false,
        criteria: vec![Criterion {
            id: "C1".into(),
            required: true,
            live: false,
            scenario: "fixture".into(),
            expected: "zero failures".into(),
        }],
        owner_decisions: vec![],
    };
    crate::completion::register_criteria(&fx.ctx, &fx.id, &manifest).unwrap();
    let file = fx.root().join("evidence.log");
    std::fs::write(&file, "evidence\n").unwrap();
    let proof = crate::add_artifact(&fx.ctx, &fx.id, &file, None, None)
        .unwrap()
        .path;
    EvidenceObservation {
        version: 1,
        key: "capture-1".into(),
        criterion: "C1".into(),
        run: "R1".into(),
        attempt: "no-gate".into(),
        status: ObservationStatus::Pass,
        action: "ran".into(),
        environment: "isolated".into(),
        expected: "zero failures".into(),
        observed: "zero failures".into(),
        artifacts: vec![proof],
        captured: "2026-01-02T00:00:00Z".parse().unwrap(),
        producer: "pi".into(),
        live: false,
        cleanup: None,
    }
}

/// A checkpoint over the current readback (expected revision 0, no progress yet).
fn checkpoint_input(ctx: &Ctx, id: &str) -> corral_core::CheckpointUpdate {
    let back = crate::progress_readback(ctx, id).unwrap();
    serde_json::from_value(json!({
        "task": back.task, "run": back.run, "spec_revision": back.spec_revision,
        "expected_revision": 0, "progress_revision": 0, "head": back.head, "base": back.base,
        "tree": back.tree, "criterion_ids": [], "evidence_refs": [], "next_action": "Continue",
        "decisions": [], "waits": [], "resources": [],
    }))
    .unwrap()
}

#[test]
fn receipts_commit_in_each_operation_and_a_fence_refuses_inside_the_final_transaction() {
    let fx = fixture(false);
    let g = fx.bind();
    let spec = crate::progress_spec_revision(&fx.task()).unwrap();
    let progress: corral_core::ProgressUpdate = serde_json::from_value(json!({
        "task": fx.id, "run": "R1", "spec_revision": spec, "expected_revision": 0,
        "phases": [{"id": "p", "label": "P", "steps": [
            {"id": "s", "label": "S", "status": {"state": "in_progress"}, "criteria": []}]}],
    }))
    .unwrap();
    let o = fx.call(g, "progress-1", "progress");
    let first = crate::progress::update_progress_request(&fx.ctx, &fx.id, &progress, &o).unwrap();
    let again = crate::progress::update_progress_request(&fx.ctx, &fx.id, &progress, &o).unwrap();
    assert!(!first.replayed && again.replayed && again.receipt == first.receipt);
    assert_eq!(fx.events("progress_updated", &o.request_id), (1, 1));

    let observation = criteria(&fx, false);
    let o = fx.call(g, "evidence-1", "evidence");
    let evidence = crate::completion::record_evidence_request(&fx.ctx, &fx.id, &observation, &o);
    assert_eq!(evidence.unwrap().receipt.result["appended"], true);
    assert_eq!(fx.task().runs[0].completion.evidence.len(), 1);

    // Given a crash right after a wait committed, when sync reconciles the task, then the
    // owed event and reconciliation run once and their flags clear.
    let o = fx.call(g, "wait-1", "wait");
    let now = jiff::Timestamp::now();
    let wait =
        || crate::obligations::register_wait_request(&fx.ctx, &fx.id, wait_request(), now, &o);
    assert!(crash_at("swept_after_commit", wait));
    effects::reconcile_task(&fx.ctx, &fx.id).unwrap();
    assert!(fx.receipt(&o).unwrap().post_pending.is_empty());
    assert_eq!(fx.events("wait_registered", &o.request_id), (1, 1));

    let o = fx.call(g, "done-1", "done");
    let gate_ref = corral_core::agent::AgentGateRef {
        attempt: "ag-0".into(),
        tier: "full".into(),
        receipt_sha256: "00".into(),
    };
    let report = Report {
        origin: &o,
        gate_ref: Some(gate_ref),
        contract_sha256: None,
    };
    let stale = crate::review::done_request(&fx.ctx, &fx.id, SUMMARY, None, &report);
    assert!(format!("{:#}", stale.unwrap_err()).contains("gate_ref_stale"));
    assert!(fx.receipt(&o).is_none());

    // Given a fenced generation, when a new request reaches the operation's final update,
    // then that transaction refuses agent_fenced and the task file is untouched.
    fence_generation(&fx.ctx.store, &fx.worker(), g, "resume").unwrap();
    let before = std::fs::read(fx.task().path).unwrap();
    let o = fx.call(g, "wait-2", "wait");
    let error = crate::obligations::register_wait_request(
        &fx.ctx,
        &fx.id,
        wait_request(),
        jiff::Timestamp::now(),
        &o,
    )
    .unwrap_err();
    let refusal = error.downcast_ref::<core_agent::AgentRefusal>().unwrap();
    assert_eq!(refusal.code.as_str(), "agent_fenced");
    assert_eq!(std::fs::read(fx.task().path).unwrap(), before);
}

#[test]
fn pm_comment_commits_its_receipt_on_the_target_task_inside_the_pm_scope() {
    let fx = fixture(false);
    let target = BindTarget::Pm {
        project: "app".into(),
    };
    let g = open_generation(&fx.ctx.store, &target, "pm-pane")
        .unwrap()
        .generation;
    let o = origin(&fx, ContainerRef::Pm, g, "comment-1", "comment");
    let first =
        crate::comments::comment_request(&fx.ctx, &fx.id, "Use the typed payload.", &o).unwrap();
    let again =
        crate::comments::comment_request(&fx.ctx, &fx.id, "Use the typed payload.", &o).unwrap();
    assert!(again.replayed && again.receipt == first.receipt);
    let task = fx.task();
    assert_eq!((task.comments.len(), task.agent_pm_receipts.len()), (1, 1));
    let other = fx
        .ctx
        .store
        .create("Elsewhere", None, Status::Backlog, "")
        .unwrap();
    let o = origin(&fx, ContainerRef::Pm, g, "comment-2", "comment");
    let error = crate::comments::comment_request(&fx.ctx, &other.id, "x", &o).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<super::Rejection>()
            .unwrap()
            .code
            .as_str(),
        "out_of_scope"
    );
    assert!(fx.ctx.store.get(&other.id).unwrap().comments.is_empty());
}

#[test]
fn done_and_verdict_refuse_pending_questions_before_any_pre_commit_effect() {
    for state in ["open", "escalated"] {
        let fx = fixture(false);
        let g = fx.bind();
        let key = fx.ask_in(&fx.worker(), state);
        let before = std::fs::read(fx.task().path).unwrap();
        let plain = crate::done(&fx.ctx, &fx.id, SUMMARY).unwrap_err();
        let o = fx.call(g, "done-1", "done");
        let report = Report {
            origin: &o,
            gate_ref: None,
            contract_sha256: None,
        };
        let agent = crate::review::done_request(&fx.ctx, &fx.id, SUMMARY, None, &report);
        for error in [plain, agent.unwrap_err()] {
            let refusal = format!("open_questions: 1 pending ({key})");
            assert!(format!("{error:#}").contains(&refusal), "{state}");
        }
        assert_eq!(std::fs::read(fx.task().path).unwrap(), before, "{state}");
    }

    let fx = fixture(false);
    let o = fx.reviewer();
    let key = fx.ask(&o.binding.container);
    let plain = crate::verdict(&fx.ctx, &fx.id, Verdict::Approve, "ok").unwrap_err();
    let report = Report {
        origin: &o,
        gate_ref: None,
        contract_sha256: None,
    };
    let agent =
        crate::reviewer::verdict_request(&fx.ctx, &fx.id, Verdict::Approve, "ok", None, &report);
    for error in [plain, agent.unwrap_err()] {
        assert!(
            format!("{error:#}").contains(&format!("review_questions_pending: 1 pending ({key})"))
        );
    }
    let task = fx.task();
    assert!(
        task.artifacts
            .iter()
            .all(|a| !a.title.starts_with("Review ("))
    );
    assert!(task.runs[0].review.is_none() && task.status == Status::Review);
}

#[test]
fn pm_capture_and_legacy_reply_reconcile_only_from_their_marker() {
    let fx = fixture(false);
    let target = BindTarget::Pm {
        project: "app".into(),
    };
    let g = open_generation(&fx.ctx.store, &target, "pm-pane")
        .unwrap()
        .generation;
    let reserve = |call: &str, kind: &str| {
        let o = origin(&fx, ContainerRef::Pm, g, call, kind);
        effects::pm_reserve(&fx.ctx.store, &o, kind, "pm")
            .unwrap()
            .receipt
            .request_id
    };
    let (marked, unmarked, reply) = (
        reserve("c1", "capture"),
        reserve("c2", "capture"),
        reserve("r1", "reply"),
    );
    for request in [&marked, &unmarked, &reply] {
        let alive = effects::reconcile_pm(&fx.ctx, request).unwrap();
        assert_eq!(
            alive.state,
            ReceiptState::Reserved,
            "a live executor stays in_progress"
        );
    }
    let created = fx
        .ctx
        .store
        .create("Captured", Some("app"), Status::Backlog, "")
        .unwrap();
    let origin = json!({"request_id": marked, "session_id": "s1", "generation": g, "at": "2026-01-02T00:00:00Z"});
    fx.ctx
        .store
        .update(&created.id, |t| {
            t.agent_origin = serde_json::from_value(origin.clone()).unwrap();
            Ok(())
        })
        .unwrap();
    pm_state::update(&fx.ctx.store, |pm| {
        for receipt in &mut pm.as_mut().unwrap().state.receipts {
            receipt.executor = Some(Executor {
                pid: u32::MAX - 1,
                start_time: "0".into(),
            });
        }
        Ok(((), true))
    })
    .unwrap();
    let state = |request: &str| effects::reconcile_pm(&fx.ctx, request).unwrap().state;
    assert_eq!(state(&marked), ReceiptState::Committed);
    assert_eq!(state(&unmarked), ReceiptState::Abandoned);
    assert_eq!(state(&reply), ReceiptState::Uncertain);
    assert_eq!(
        fx.ctx.store.list().unwrap().len(),
        2,
        "nothing was re-executed"
    );
}

#[test]
fn verdict_waits_until_the_reviewer_observed_every_answer_and_correction() {
    use corral_core::inbox::{
        Inbox, MessageBinding, Post, Receipt, ReceiptLevel, Recipient, TerminalState, Urgency,
    };
    for kind in ["question_answer", "question_correction"] {
        // Given a bound review child with an unobserved answer or correction message.
        let fx = fixture(false);
        let o = fx.reviewer();
        let review = "R1-review1".to_owned();
        let recipient = Recipient::ReviewRun {
            task: fx.id.clone(),
            run: "R1".into(),
            review,
        };
        let inbox = Inbox::new(&fx.ctx.store, recipient).unwrap();
        assert_eq!(Recipient::parse_key(inbox.key()).unwrap(), inbox.recipient);
        let plan_review = Recipient::parse_key(&format!("{}_P1_PR1", fx.id)).unwrap();
        assert!(matches!(plan_review, Recipient::PlanReview { .. }));
        let message = inbox
            .post(Post {
                kind,
                urgency: Urgency::Wake,
                author: "pm",
                body: "Use the typed payload.",
                binding: MessageBinding::default(),
                subject: None,
                max_payload: 64 * 1024,
            })
            .unwrap();

        // When the plain and the request-origin verdicts arrive, then both are refused with
        // the message ID before the notes artifact, and the review stays pending.
        let plain = crate::verdict(&fx.ctx, &fx.id, Verdict::Approve, "ok").unwrap_err();
        let report = Report {
            origin: &o,
            gate_ref: None,
            contract_sha256: None,
        };
        let agent = crate::reviewer::verdict_request(
            &fx.ctx,
            &fx.id,
            Verdict::Approve,
            "ok",
            None,
            &report,
        );
        for error in [plain, agent.unwrap_err()] {
            let refusal = format!("review_answers_unobserved: 1 ({})", message.id);
            assert!(format!("{error:#}").contains(&refusal), "{kind}: {error:#}");
        }
        let task = fx.task();
        assert!(
            task.artifacts
                .iter()
                .all(|a| !a.title.starts_with("Review ("))
        );
        assert!(task.runs[0].review.is_none());

        // When the reviewer observes it (or it ends unobserved), then the verdict commits.
        if kind == "question_answer" {
            let receipt = Receipt {
                envelope: None,
                binding: None,
                observed_at: jiff::Timestamp::now(),
                via: "test".into(),
                detail: json!({}),
            };
            inbox
                .record_receipt(message.seq, ReceiptLevel::Observed, &receipt)
                .unwrap();
        } else {
            let cancelled = TerminalState::Cancelled;
            inbox
                .mark_terminal(message.seq, cancelled, "withdrawn", "test")
                .unwrap();
        }
        let committed = crate::reviewer::verdict_request(
            &fx.ctx,
            &fx.id,
            Verdict::Approve,
            "ok",
            None,
            &report,
        );
        assert_eq!(
            committed.unwrap().receipt.state,
            ReceiptState::Committed,
            "{kind}"
        );
    }
}

#[path = "effects_gate_tests.rs"]
mod gate;

#[path = "effects_queue_tests.rs"]
mod queue;

#[path = "effects_recovery_tests.rs"]
mod recovery;
