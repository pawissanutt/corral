//! T-252 reviewer-role acceptance: `corral review-run` with a pi-native profile binds the review
//! child from the launcher, and the bound host's verdict, questions, supersession, stale attempts
//! and silent turns go through `corral agent call` envelopes and the sync hook.
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

use corral_core::agent::{
    self as core_agent, AgentRole, ContainerRef, QuestionState, ReceiptState, RequestIdentity,
    TurnClass, paths,
};
use corral_core::inbox::{Inbox, Receipt, ReceiptLevel, Recipient, TerminalState};
use corral_core::{Status, Store, Task, Verdict};
use corral_herdr::Herdr;
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::launch::{BOOTSTRAP_FILE, Bootstrap, generation_dir, runtime_root};
use crate::agent::test_support::{self as ts, Host, committed, envelope, hello, send};
use crate::agent::{CallResponse, event_request_id, tool_request_id};
use crate::questions::{Actor, AnswerInput, QuestionRefusal, RefusalKind};

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
  "pane close") rm -f "$3"; ok '{}' ;;
  "pane read") if [ -f "$3" ]; then ok '{"text":"screen"}'; else printf '{"error":{"code":"pane_not_found","message":"gone"}}\n'; exit 1; fi ;;
  *) ok '{}' ;;
esac
"#;

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

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A Review task in project `corral` whose done run R1 is reviewed by the pi-native profile `pi`
/// through `corral review-run`; `host` is the review child's host after hello.
struct Review {
    fx: ts::Fx,
    id: String,
    child: String,
    host: Host,
    seq: u64,
}

impl Drop for Review {
    fn drop(&mut self) {
        let root = runtime_root().join(paths::store_id(&self.fx.ctx.store));
        let _ = std::fs::remove_dir_all(root);
    }
}

fn review() -> Review {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let app = root.join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-qb", "main"]);
    for (key, value) in [("user.name", "t"), ("user.email", "t@example.invalid")] {
        git(&app, &["config", key, value]);
    }
    std::fs::write(app.join("tracked"), "initial\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "initial"]);
    let bin = root.join("herdr");
    std::fs::write(&bin, FAKE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let assets = root.join("assets");
    write(&assets, "compat.json", r#"{"verdict":"GO"}"#);
    for module in [
        "src/roles/reviewer/index.ts",
        "src/delivery/index.ts",
        "src/auth/index.ts",
    ] {
        write(&assets, module, "");
    }
    let mut store = Store::init(&root).unwrap();
    store.config.limits.review = 4;
    store.config.limits.count_reviews = false;
    store.config.limits.count_gates = false;
    store.config.dispatch.paused = true;
    store.config.agent.assets_dir = Some(assets);
    let project = serde_json::from_value(json!({"path": "app"})).unwrap();
    store.config.projects.insert("corral".into(), project);
    let profile = serde_json::from_value(json!({
        "harness": "pi-native", "model": "gpt-6.1-sol", "vendor": "openai", "effort": "high"
    }))
    .unwrap();
    store.config.profiles.insert("pi".into(), profile);
    store.save_config().unwrap();
    let ctx = ctx_at(&root);
    let task = ctx
        .store
        .create("Pi review", Some("corral"), Status::Review, "spec")
        .unwrap();
    let herdr = Herdr::with_bin(bin, None);
    let run: corral_core::Run = serde_json::from_value(json!({
        "id": "R1", "agent": "omo", "vendor": "anthropic", "worktree": app, "branch": "main",
        "base": "main", "base_sha": git(&app, &["rev-parse", "HEAD"]), "outcome": "done",
        "summary": "Did it.", "started": "2026-01-01T00:00:00Z",
        "herdr": {"workspace_id": "w0", "pane_id": "w0:p1", "socket": herdr.socket_path().unwrap()},
    }))
    .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    let fx = ts::Fx { _tmp: tmp, ctx };
    let (child, host) = launch(&fx, &task.id);
    Review {
        fx,
        id: task.id,
        child,
        host,
        seq: 0,
    }
}

/// `corral review-run <task> pi`, then the host the launcher's bootstrap describes says hello.
fn launch(fx: &ts::Fx, id: &str) -> (String, Host) {
    crate::review_run(&fx.ctx, id, Some("pi")).unwrap();
    let task = fx.ctx.store.get(id).unwrap();
    let child = task.runs[0].review_runs.last().unwrap().clone();
    let generation = child.agent_host.as_ref().unwrap().binding.generation;
    let recipient = Recipient::ReviewRun {
        task: id.into(),
        run: "R1".into(),
        review: child.id.clone(),
    };
    let dir = generation_dir(&fx.ctx.store, &recipient, generation).unwrap();
    let bootstrap: Bootstrap =
        serde_json::from_slice(&std::fs::read(dir.join(BOOTSTRAP_FILE)).unwrap()).unwrap();
    let session = format!("s-{}", child.id);
    let host = Host {
        identity: RequestIdentity {
            store: bootstrap.store.clone(),
            project: bootstrap.project.clone(),
            role: bootstrap.role,
            task: bootstrap.task.clone(),
            run: bootstrap.run.clone(),
            review: bootstrap.review.clone(),
            callback: bootstrap.callback.clone(),
            session_id: session.clone(),
            generation: bootstrap.generation,
            turn: None,
        },
        host_cap: ts::secret(&format!("{session}-host"), generation),
        ctl_cap: ts::secret(&format!("{session}-ctl"), generation),
        nonce: bootstrap.bind_nonce.clone(),
        pane: bootstrap.pane.clone(),
        sessions: bootstrap.session_dir.clone(),
    };
    committed(&hello(fx, &host));
    (child.id, host)
}

impl Review {
    fn ctx(&self) -> &Ctx {
        &self.fx.ctx
    }

    fn task(&self) -> Task {
        self.ctx().store.get(&self.id).unwrap()
    }

    fn container(&self) -> ContainerRef {
        ContainerRef::Reviewer {
            task: self.id.clone(),
            run: "R1".into(),
            review: self.child.clone(),
        }
    }

    fn state(&self) -> core_agent::AgentRunState {
        ts::state(&self.fx, &self.container())
    }

    fn inbox(&self) -> Inbox<'_> {
        let recipient = Recipient::ReviewRun {
            task: self.id.clone(),
            run: "R1".into(),
            review: self.child.clone(),
        };
        Inbox::new(&self.ctx().store, recipient).unwrap()
    }

    fn envelope(&self, turn: Option<u64>, call: &str, op: &str, payload: Value) -> Vec<u8> {
        let mut host = self.host.clone();
        host.identity.turn = turn;
        let id = tool_request_id(&host.identity.session_id, call);
        envelope(&host, op, &id, payload, false)
    }

    fn op(&self, turn: Option<u64>, call: &str, op: &str, payload: Value) -> CallResponse {
        send(&self.fx, &self.envelope(turn, call, op, payload), op)
    }

    fn event(&mut self, kind: &str, turn: u64) -> CallResponse {
        self.seq += 1;
        let id = event_request_id(self.host.identity.generation, self.seq);
        let payload = json!({"kind": kind, "turn": turn});
        send(
            &self.fx,
            &envelope(&self.host, "event", &id, payload, false),
            "event",
        )
    }

    fn class(&self, turn: u64) -> Option<TurnClass> {
        let state = self.state();
        state.turns.iter().find(|t| t.seq == turn)?.class
    }

    fn sync(&self) {
        crate::agent::hooks::sync::task(self.ctx(), &self.task(), jiff::Timestamp::now()).unwrap();
    }

    fn ask(&self, call: &str, blocking: bool) -> String {
        let payload = json!({"kind": "clarification", "text": "Is the fixture enough?", "blocking": blocking});
        let receipt = committed(&self.op(Some(1), call, "ask", payload));
        receipt.result["key"].as_str().unwrap().to_owned()
    }

    fn revision(&self, key: &str) -> u32 {
        let state = self.state();
        state
            .questions
            .iter()
            .find(|q| q.key == key)
            .unwrap()
            .revision
    }

    /// The PM answers, citing an owner comment as its basis.
    fn pm_answer(&self, key: &str) -> anyhow::Result<crate::questions::Applied> {
        let owner = corral_core::CommentAuthor::Owner;
        crate::comments::comment(
            self.ctx(),
            &self.id,
            "The fixture is the agreed evidence.",
            owner,
            false,
        )?;
        let comments = self.task().comments.len();
        let input = AnswerInput {
            text: "Yes, the fixture is enough.".into(),
            option: None,
            basis_refs: vec![format!("comment:{}#{comments}", self.id)],
            scope_note: Some("inside the approved scope".into()),
        };
        crate::questions::answer(self.ctx(), key, self.revision(key), Actor::Pm(None), input)
    }

    fn calls_of(&self, prefix: &str) -> usize {
        let log = self.ctx().store.root().join("calls.log");
        let log = std::fs::read_to_string(log).unwrap_or_default();
        log.lines().filter(|l| l.starts_with(prefix)).count()
    }
}

fn rejection(response: &CallResponse) -> (&'static str, String) {
    let rejection = response.rejection().expect("a rejection");
    (rejection.code.as_str(), rejection.reason.clone())
}

fn observe(inbox: &Inbox<'_>, kind: &str) {
    let message = inbox
        .messages()
        .unwrap()
        .into_iter()
        .find(|m| m.kind == kind)
        .unwrap();
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
}

#[test]
fn review_run_binds_the_parent_run_review_child_and_callback_token_from_the_launcher() {
    let review = review();
    let task = review.task();
    let parent = &task.runs[0];
    let child = parent.review_runs.last().unwrap();
    let state = child.agent_host.as_ref().unwrap();
    let token = crate::review_callback_token(parent.review.as_ref().unwrap()).unwrap();
    assert_eq!(state.binding.role, AgentRole::Reviewer);
    assert_eq!(state.binding.callback.as_deref(), Some(token.as_str()));
    let pane = &child.herdr.as_ref().unwrap().pane_id;
    assert_eq!(&state.generation(1).unwrap().pane, pane);
    assert_eq!(child.harness.as_deref(), Some("pi-native"));
    let identity = &review.host.identity;
    assert_eq!(
        (identity.run.as_deref(), identity.review.as_deref()),
        (Some("R1"), Some(review.child.as_str()))
    );
    assert_eq!(identity.callback.as_deref(), Some(token.as_str()));
    assert_eq!(review.host.pane, *pane);
    // The pane ran the launcher, not the model, with the one-shot bootstrap.
    assert_eq!(review.calls_of("pane run"), 1);
    let log = std::fs::read_to_string(review.ctx().store.root().join("calls.log")).unwrap();
    assert!(log.contains("agent launch --bootstrap"), "{log}");
    // hello advertises exactly the reviewer's landed ops.
    let receipt = state_hello(&review);
    assert_eq!(
        receipt,
        json!([
            "hello",
            "event",
            "receipt_lookup",
            "ask",
            "withdraw",
            "evidence",
            "artifact",
            "verdict"
        ])
    );
}

fn state_hello(review: &Review) -> Value {
    let state = review.state();
    let receipt = state.receipts.iter().find(|r| r.op == "hello").unwrap();
    receipt.result["ops"].clone()
}

#[test]
fn a_verdict_whose_ack_was_lost_replays_its_receipt_and_the_turn_end_fences_the_binding() {
    let mut review = review();
    let prompt = crate::agent::hooks::review::review_prompt_file(review.ctx(), &review.id, "R1");
    let gate = "## Gate contract (review, authoritative)\n\n- Tier: quick\n";
    let text = std::fs::read_to_string(&prompt).unwrap();
    std::fs::write(&prompt, format!("{text}\n{gate}\n## Review packet\n\nx\n")).unwrap();
    let section = crate::agent::hooks::review::gate_section(review.ctx(), &review.id, "R1")
        .unwrap()
        .unwrap();
    assert_eq!(section.text, gate);
    committed(&review.event("turn_started", 1));
    let notes = "Changes needed:\n- add the regression test";
    let payload = json!({"verdict": "changes", "notes": notes});
    let input = review.envelope(Some(1), "call-verdict", "verdict", payload);
    let first = committed(&send(&review.fx, &input, "verdict"));
    assert_eq!(first.result["verdict"], "changes");
    // The verdict is the one corral verdict --attempt records, with the VerdictReport beside it.
    let task = review.task();
    let record = task.runs[0].review.clone().unwrap();
    assert_eq!(record.verdict, Some(Verdict::Changes));
    assert_eq!(record.notes.as_deref(), Some(notes));
    assert!(
        task.artifacts
            .iter()
            .any(|a| a.title.starts_with("Review ("))
    );
    let state = review.state();
    let report = state.reports.last().unwrap();
    assert_eq!(report.verdict, Some(Verdict::Changes));
    assert_eq!(
        report.contract_sha256.as_deref(),
        Some(section.contract_sha256.as_str())
    );
    assert!(state.binding.closing);

    // The open turn's turn_ended is the final receipt; then the binding is fenced.
    let ended = committed(&review.event("turn_ended", 1));
    assert_eq!(ended.result["fenced"], true);
    assert_eq!(review.class(1), Some(TurnClass::Completed));
    let evidence = review.op(Some(2), "call-late", "artifact", json!({"path": "tracked"}));
    assert_eq!(rejection(&evidence).0, "review_closed");
    // The acknowledgement was lost: the replay returns the stored receipt, also after a newer
    // review child started.
    review
        .ctx()
        .store
        .update(&review.id, |t| {
            let mut newer = t.runs[0].review_runs.last().unwrap().clone();
            newer.id = format!("{}-newer", newer.id);
            newer.agent_host = None;
            t.runs[0].review_runs.push(newer);
            Ok(())
        })
        .unwrap();
    match send(&review.fx, &input, "verdict") {
        CallResponse::Receipt { receipt, replayed } => {
            assert!(replayed);
            assert_eq!(receipt, first);
        }
        other => panic!("expected the stored receipt, got {other:?}"),
    }
}

#[test]
fn a_blocking_question_refuses_the_verdict_until_the_answer_is_observed() {
    let mut review = review();
    committed(&review.event("turn_started", 1));
    let key = review.ask("call-ask", true);
    let refused = review.op(
        Some(1),
        "call-v1",
        "verdict",
        json!({"verdict": "approve", "notes": "Looks right."}),
    );
    let (code, reason) = rejection(&refused);
    assert_eq!(code, "question_state");
    assert!(reason.contains(&key), "{reason}");
    committed(&review.event("turn_ended", 1));
    assert_eq!(review.class(1), Some(TurnClass::AwaitingAnswer));

    // The PM answers; the unobserved answer still refuses the verdict.
    review.pm_answer(&key).unwrap();
    committed(&review.event("turn_started", 2));
    let unread = review.op(
        Some(2),
        "call-v2",
        "verdict",
        json!({"verdict": "approve", "notes": "Looks right."}),
    );
    let (code, reason) = rejection(&unread);
    assert_eq!(code, "question_state");
    assert!(reason.starts_with("review_answers_unobserved"), "{reason}");
    observe(&review.inbox(), "question_answer");
    let verdict = review.op(
        Some(2),
        "call-v3",
        "verdict",
        json!({"verdict": "approve", "notes": "Looks right."}),
    );
    assert_eq!(committed(&verdict).result["verdict"], "approve");

    // A later correction for that key is refused question_closed.
    let correction = AnswerInput {
        text: "Actually, add a test.".into(),
        option: None,
        basis_refs: vec![],
        scope_note: None,
    };
    let rev = review.revision(&key);
    let error =
        crate::questions::answer(review.ctx(), &key, rev, Actor::Owner, correction).unwrap_err();
    let refusal = error.downcast_ref::<QuestionRefusal>().unwrap();
    assert_eq!(refusal.kind, RefusalKind::Closed, "{refusal:?}");
}

#[test]
fn a_new_candidate_before_the_answer_closes_the_child_and_no_answer_crosses_attempts() {
    let mut review = review();
    committed(&review.event("turn_started", 1));
    let answered = review.ask("call-ask", true);
    review.pm_answer(&answered).unwrap();
    let old_key = review.ask("call-ask-open", false);
    let old_inbox = review.inbox();
    let old_pane = review.host.pane.clone();
    let pending = old_inbox
        .messages()
        .unwrap()
        .into_iter()
        .find(|m| m.kind == "question_answer")
        .unwrap();

    // The parent gets a new candidate, and review-run starts the next review child.
    let app = review.task().runs[0].worktree.clone();
    std::fs::write(app.join("tracked"), "second\n").unwrap();
    git(&app, &["commit", "-qam", "second"]);
    let old_child = review.child.clone();
    let (child, host) = launch(&review.fx, &review.id);
    assert_ne!(child, old_child);
    review.sync();

    // supersede closed the old child: its open question is cancelled review_superseded, the
    // unread answer message cancelled recipient_closed, and the old pane was closed.
    let task = review.task();
    let old = ContainerRef::Reviewer {
        task: review.id.clone(),
        run: "R1".into(),
        review: old_child.clone(),
    };
    let old_state = core_agent::container_state(&task, &old).unwrap();
    let question = old_state
        .questions
        .iter()
        .find(|q| q.key == old_key)
        .unwrap();
    assert_eq!(question.state, QuestionState::Cancelled);
    assert_eq!(
        question.terminal.as_ref().unwrap().reason,
        "review_superseded"
    );
    let terminal = old_inbox.terminal(pending.seq).unwrap().unwrap();
    assert_eq!(terminal, TerminalState::Cancelled);
    let record = old_inbox.record_path(pending.seq, TerminalState::Cancelled.suffix());
    let record: Value = serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
    assert_eq!(record["reason"], "recipient_closed");
    let log = std::fs::read_to_string(review.ctx().store.root().join("calls.log")).unwrap();
    assert!(log.contains(&format!("pane close {old_pane}")), "{log}");

    // The next review child asks under its own key; an answer naming the old key is refused.
    review.child = child;
    review.host = host;
    review.seq = 0;
    committed(&review.event("turn_started", 1));
    let new_key = review.ask("call-ask-2", true);
    assert_eq!(new_key, format!("{}/R1/{}/Q1", review.id, review.child));
    let late = AnswerInput {
        text: "Late.".into(),
        option: None,
        basis_refs: vec![],
        scope_note: None,
    };
    let rev = question.revision;
    let error =
        crate::questions::answer(review.ctx(), &old_key, rev, Actor::Owner, late).unwrap_err();
    let refusal = error.downcast_ref::<QuestionRefusal>().unwrap();
    assert_eq!(refusal.kind, RefusalKind::Closed, "{refusal:?}");
    let correction = AnswerInput {
        text: "Corrected.".into(),
        option: None,
        basis_refs: vec![],
        scope_note: None,
    };
    let rev = old_state
        .questions
        .iter()
        .find(|q| q.key == answered)
        .unwrap()
        .revision;
    let error = crate::questions::answer(review.ctx(), &answered, rev, Actor::Owner, correction)
        .unwrap_err();
    let refusal = error.downcast_ref::<QuestionRefusal>().unwrap();
    assert_eq!(refusal.kind, RefusalKind::Closed, "{refusal:?}");
    let new_inbox = review.inbox();
    assert!(new_inbox.messages().unwrap().is_empty());
}

#[test]
fn a_verdict_for_a_stale_candidate_is_rejected_by_the_verdict_binding_and_recorded() {
    let mut review = review();
    committed(&review.event("turn_started", 1));
    let app = review.task().runs[0].worktree.clone();
    std::fs::write(app.join("tracked"), "moved\n").unwrap();
    git(&app, &["commit", "-qam", "moved"]);
    let before = ts::events(&review.fx, "agent_request_rejected");
    let stale = review.op(
        Some(1),
        "call-stale",
        "verdict",
        json!({"verdict": "approve", "notes": "Looks right."}),
    );
    let (code, reason) = rejection(&stale);
    assert_eq!(code, "review_closed");
    assert!(reason.starts_with("stale review:"), "{reason}");
    assert_eq!(stale.to_json().unwrap()["state"], "rejected");
    assert_eq!(ts::events(&review.fx, "agent_request_rejected"), before + 1);
    let task = review.task();
    assert!(task.runs[0].review.as_ref().unwrap().verdict.is_none());
    assert!(
        review
            .state()
            .receipts
            .iter()
            .all(|r| r.op != "verdict" || r.state != ReceiptState::Committed)
    );
}

#[test]
fn a_silent_reviewer_turn_is_no_verdict_gets_one_nudge_then_escalates_without_the_idle_window() {
    let mut review = review();
    assert!(review.ctx().store.config.sync.review_idle_after_secs >= 1800);
    committed(&review.event("turn_started", 1));
    committed(&review.event("turn_ended", 1));
    assert_eq!(review.class(1), Some(TurnClass::NoVerdict));
    review.sync();
    review.sync();
    let nudges: Vec<_> = review
        .inbox()
        .messages()
        .unwrap()
        .into_iter()
        .filter(|m| m.kind == "turn_nudge")
        .collect();
    assert_eq!(nudges.len(), 1, "one nudge per generation");
    assert_eq!(review.task().status, Status::Review);

    committed(&review.event("turn_started", 2));
    committed(&review.event("turn_ended", 2));
    assert_eq!(review.class(2), Some(TurnClass::NoVerdict));
    review.sync();
    let task = review.task();
    assert_eq!(task.status, Status::NeedsMe);
    let excerpt = task.runs[0].blocked_excerpt.clone().unwrap();
    assert!(
        excerpt.starts_with(crate::agent::hooks::review::REVIEW_HOLD_PREFIX),
        "{excerpt}"
    );
    assert_eq!(ts::events(&review.fx, "review_escalated"), 1);
    let nudges = review.inbox().messages().unwrap();
    assert_eq!(nudges.iter().filter(|m| m.kind == "turn_nudge").count(), 1);
}

/// Store setup for the reviewer cross-lane fixture
/// (packages/corral-pi/src/roles/reviewer/cross-lane.test.ts): a Review task whose done R1 has a
/// pending review and a pi-native review child in the fixture's private herdr pane, a review
/// prompt with a Gate contract (review) section, and generation 1 opened for the in-process host.
/// It prints one `REVIEWER_LANE <json>` line with the bind nonce the host proves at hello.
#[test]
#[ignore = "driven by packages/corral-pi/src/roles/reviewer/cross-lane.test.ts"]
fn reviewer_cross_lane_setup() {
    let Ok(spec) = std::env::var("CORRAL_REVIEWER_LANE") else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let text = |key: &str| spec[key].as_str().unwrap().to_owned();
    let root = std::path::PathBuf::from(text("root"));
    let mut store = Store::init(&root).unwrap();
    store.config.dispatch.paused = true;
    let project = serde_json::from_value(json!({"path": "app"})).unwrap();
    store.config.projects.insert("corral".into(), project);
    store.save_config().unwrap();
    let ctx = Ctx::from_store(Store::discover(&root).unwrap());
    let task = ctx
        .store
        .create("Pi reviewer lane", Some("corral"), Status::Review, "spec")
        .unwrap();
    let app = root.join("app");
    let head = git(&app, &["rev-parse", "HEAD"]);
    let herdr = json!({"workspace_id": text("workspace"), "pane_id": text("pane"),
                       "socket": ctx.herdr.socket_path().unwrap()});
    let parent: corral_core::Run = serde_json::from_value(json!({
        "id": "R1", "agent": "omo", "worktree": app, "branch": "main", "base": "main",
        "base_sha": head, "outcome": "done", "summary": "Shipped it.",
        "started": jiff::Timestamp::now(),
    }))
    .unwrap();
    let child: corral_core::Run = serde_json::from_value(json!({
        "id": "R1-review1", "agent": "pi", "harness": "pi-native", "delivery": "inbox",
        "worktree": app, "branch": "main", "base": "main", "base_sha": head,
        "started": jiff::Timestamp::now(), "herdr": herdr,
    }))
    .unwrap();
    let saved = ctx
        .store
        .update(&task.id, |t| {
            t.runs.push(parent);
            Ok(())
        })
        .unwrap();
    let run = &saved.runs[0];
    let record = corral_core::ReviewRecord {
        evidence_digest: Some(crate::verification::evidence_digest(run).unwrap()),
        identity: Some(
            crate::verification::review_identity(&ctx, &saved, run, "xl-attempt").unwrap(),
        ),
        verdict: None,
        process_lost: false,
        reviewer: "pi".into(),
        notes: None,
        sha: head.clone(),
        at: jiff::Timestamp::now(),
        pane: Some(text("pane")),
        route: None,
    };
    let callback = crate::review_callback_token(&record).unwrap();
    ctx.store
        .update(&task.id, |t| {
            let mut child = child.clone();
            child.review = Some(record.clone());
            t.runs[0].review = Some(record.clone());
            t.runs[0].review_runs.push(child);
            Ok(())
        })
        .unwrap();
    let prompt = crate::agent::hooks::review::review_prompt_file(&ctx, &task.id, "R1");
    std::fs::create_dir_all(prompt.parent().unwrap()).unwrap();
    let gate = "## Gate contract (review, authoritative)\n\n- Tier: quick\n";
    std::fs::write(
        &prompt,
        format!("# Review\n\nReview it.\n\n{gate}\n## Packet\n"),
    )
    .unwrap();
    let section = crate::agent::hooks::review::gate_section(&ctx, &task.id, "R1")
        .unwrap()
        .unwrap();
    let target = crate::agent::BindTarget::Reviewer {
        task: task.id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
        callback: callback.clone(),
    };
    let grant = crate::agent::open_generation(&ctx.store, &target, &text("pane")).unwrap();
    let recipient = Recipient::ReviewRun {
        task: task.id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    let line = json!({
        "task": task.id, "run": "R1", "review": "R1-review1", "callback": callback,
        "generation": grant.generation, "nonce": grant.bind_nonce,
        "store": paths::store_id(&ctx.store), "project": "corral", "prompt_file": prompt,
        "contract_sha256": section.contract_sha256,
        "sessions": paths::container_sessions_dir(&ctx.store, &grant.container),
        "generation_dir": generation_dir(&ctx.store, &recipient, grant.generation).unwrap(),
    });
    println!("REVIEWER_LANE {line}");
}
