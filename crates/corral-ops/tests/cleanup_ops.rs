//! Stateful fake-herdr integration tests: real git/store, synchronous gates, no live panes or waits.
#![cfg(unix)] // Fixtures execute a POSIX-shebang engine and use arg0/signals.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

mod support;

use corral_core::{
    Accept, AgentCmd, AutoStage, CheckpointUpdate, GateResult, ProfileChoice, Run, Status, Store,
    Verdict,
};
use corral_herdr::Herdr;
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};
use serde_json::{Value, json};

#[path = "cleanup_ops/lifecycle.rs"]
mod lifecycle;
#[path = "cleanup_ops/worktree.rs"]
mod worktree;

// Every invocation reloads durable state. Closing the last pane deliberately leaves an empty
// workspace; workspace close destroys any remaining panes, just as an unsafe real close would.
const FAKE_HERDR: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, subprocess, sys
root = pathlib.Path(__file__).parent
os.environ['CORRAL_ROOT'] = str(root)
for name in ('CORRAL_GATE_ATTEMPT', 'CORRAL_CANDIDATE_HEAD', 'CORRAL_INTEGRATION_BASE',
             'HERDR_PANE_ID', 'HERDR_WORKSPACE_ID', 'HERDR_TAB_ID'):
    os.environ.pop(name, None)
path = root / 'herdr-state.json'
s = json.loads(path.read_text())
a = sys.argv[1:]
if a[:1] == ['--session']:
    a = a[2:]
s['calls'].append(a)
event = s.get('block_event_log')
if event:
    event = pathlib.Path(event)
    backup = event.with_suffix('.saved')
    if backup.exists():
        event.rmdir()
        backup.rename(event)
def finish(result=None, error=None):
    path.write_text(json.dumps(s))
    print(json.dumps({'id': 'fake', 'error': error} if error else
                     {'id': 'fake', 'result': result or {}}))
    sys.exit(1 if error else 0)
def fail(code, message):
    finish(error={'code': code, 'message': message})
key = ' '.join(a[:3])
if key in s['fail']:
    if event:
        event.rename(backup)
        event.mkdir()
    fail('cleanup_rejected', 'injected-cleanup-error: ' + key)
op = a[:2]
if op == ['workspace', 'list']:
    finish({'workspaces': list(s['workspaces'].values())})
elif op in (['workspace', 'create'], ['tab', 'create']):
    n = s['next']
    s['next'] += 1
    ws = 'w' + str(n) if a[0] == 'workspace' else a[a.index('--workspace') + 1]
    if a[0] == 'workspace':
        s['workspaces'][ws] = {'workspace_id': ws, 'label': a[a.index('--label') + 1]}
    elif ws not in s['workspaces']:
        fail('workspace_not_found', ws)
    p = {'workspace_id': ws, 'tab_id': ws + ':t' + str(n),
         'pane_id': ws + ':p' + str(n), 'agent_status': 'idle',
         'cwd': a[a.index('--cwd') + 1]}
    s['panes'][p['pane_id']] = p
    finish({'workspace': s['workspaces'][ws], 'root_pane': p})
elif op == ['pane', 'list']:
    # A separate observer supplies a known inventory after owned panes close.
    finish({'panes': [*s['panes'].values(), {'pane_id': 'w0:p1', 'workspace_id': 'w0'}]})
elif op == ['pane', 'get']:
    if a[2] not in s['panes']:
        fail('pane_not_found', a[2])
    finish({'pane': s['panes'][a[2]]})
elif op == ['pane', 'close']:
    if s.get('require_worktree') and not pathlib.Path(s['require_worktree']).is_dir():
        fail('worktree_removed_too_soon', s['require_worktree'])
    if a[2] not in s['panes']:
        fail('pane_not_found', a[2])
    del s['panes'][a[2]]
    finish()
elif op == ['workspace', 'close']:
    ws = a[2]
    if ws not in s['workspaces']:
        fail('workspace_not_found', ws)
    del s['workspaces'][ws]
    s['panes'] = {k: p for k, p in s['panes'].items() if p['workspace_id'] != ws}
    finish()
elif op == ['pane', 'run']:
    subprocess.run(['bash', '-c', a[3]], stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    # A live agent never returns to its shell, so it never leaves a launch receipt.
    receipt = a[3].split("> '", 1)[-1].split("'", 1)[0]
    if '/launch-exits/' in receipt:
        pathlib.Path(receipt).unlink(missing_ok=True)
    finish()
elif op == ['pane', 'read']:
    if a[2] not in s['panes']:
        fail('pane_not_found', a[2])
    finish({'text': '> '})
elif op in (['pane', 'report-metadata'], ['workspace', 'focus'],
            ['agent', 'focus'], ['agent', 'prompt'], ['notification', 'show']):
    finish()
else:
    fail('unexpected_command', repr(a))
"#;

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    state: PathBuf,
    app: PathBuf,
    id: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("herdr");
        std::fs::write(&bin, FAKE_HERDR).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let state = tmp.path().join("herdr-state.json");
        std::fs::write(
            &state,
            json!({"next":1,"workspaces":{},"panes":{},"calls":[],"fail":[]}).to_string(),
        )
        .unwrap();
        let app = tmp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        git(&app, &["config", "user.name", "Corral Test"]);
        git(&app, &["config", "user.email", "corral@test.invalid"]);
        git(&app, &["config", "commit.gpgsign", "false"]);
        std::fs::write(app.join("README.md"), "base\n").unwrap();
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "initial"]);
        let mut store = Store::init(tmp.path()).unwrap();
        for name in ["fake", "rev"] {
            store.config.agents.insert(
                name.into(),
                AgentCmd {
                    command: "true".into(),
                },
            );
        }
        store.config.pm.agent = "rev".into();
        store.config.pm.push_events = false;
        store.config.review.gate = Some(vec!["true".into()]);
        store.config.review.reviewer = Some("rev".into());
        store.config.review.accept = Some(Accept::Owner);
        store.save_config().unwrap();
        let mut ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None)
                .with_poll_wait(|_| panic!("unexpected delivery wait")),
        };
        let task = ctx
            .store
            .create(
                "Cleanup regression",
                Some("app"),
                Status::Ready,
                "## Goal\nShip.\n",
            )
            .unwrap();
        let preview = ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: [task.id.clone()].into(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    accept: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        ctx.store.config = ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
            .unwrap();
        assert!(matches!(
            corral_ops::dispatch(
                &ctx,
                &task.id,
                &DispatchOpts {
                    profile: Some("fake".into()),
                    ..DispatchOpts::default()
                }
            )
            .unwrap(),
            DispatchOutcome::Started { .. }
        ));
        let fx = Self {
            _tmp: tmp,
            ctx,
            state,
            app,
            id: task.id,
        };
        fx.checkpoint();
        fx
    }

    fn run(&self) -> Run {
        self.ctx
            .store
            .get(&self.id)
            .unwrap()
            .current_run()
            .unwrap()
            .clone()
    }

    fn state(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.state).unwrap()).unwrap()
    }

    fn edit(&self, edit: impl FnOnce(&mut Value)) {
        let mut state = self.state();
        edit(&mut state);
        std::fs::write(&self.state, serde_json::to_vec(&state).unwrap()).unwrap();
    }

    fn reset_calls(&self) {
        self.edit(|s| s["calls"] = json!([]));
    }

    fn called(&self, args: &[&str]) -> bool {
        self.state()["calls"]
            .as_array()
            .unwrap()
            .contains(&json!(args))
    }

    fn pane_exists(&self, pane: &str) -> bool {
        !self.state()["panes"][pane].is_null()
    }

    fn workspace_exists(&self, ws: &str) -> bool {
        !self.state()["workspaces"][ws].is_null()
    }

    fn sync(&self) {
        corral_ops::sync_at(&self.ctx, "2026-10-15T12:00:00Z".parse().unwrap()).unwrap();
    }

    fn done(&self) {
        let run = self.run();
        std::fs::write(run.worktree.join("feature.txt"), format!("{}\n", run.id)).unwrap();
        git(&run.worktree, &["add", "feature.txt"]);
        git(&run.worktree, &["commit", "-q", "-m", "Implement feature"]);
        self.checkpoint();
        corral_ops::done(&self.ctx, &self.id, "Implemented feature").unwrap();
    }

    fn checkpoint(&self) {
        let state = corral_ops::progress_readback(&self.ctx, &self.id).unwrap();
        corral_ops::save_checkpoint(
            &self.ctx,
            &self.id,
            &CheckpointUpdate {
                task: self.id.clone(),
                run: state.run,
                spec_revision: state.spec_revision,
                expected_revision: state
                    .checkpoints
                    .last()
                    .map_or(0, |c| c.checkpoint.revision),
                progress_revision: state.progress.as_ref().map_or(0, |p| p.revision),
                head: state.head,
                base: state.base,
                tree: state.tree,
                criterion_ids: Vec::new(),
                evidence_refs: Vec::new(),
                next_action: "Continue the cleanup lifecycle scenario".into(),
                decisions: Vec::new(),
                waits: Vec::new(),
                resources: Vec::new(),
            },
        )
        .unwrap();
    }

    fn review(&self) -> Vec<String> {
        self.done();
        self.sync();
        let run = self.run();
        assert_eq!(run.autopilot.as_ref().unwrap().stage, AutoStage::Reviewing);
        assert_eq!(run.gate.as_ref().unwrap().result, GateResult::Pass);
        vec![
            run.herdr.unwrap().pane_id,
            run.gate.unwrap().pane.unwrap(),
            run.review.unwrap().pane.unwrap(),
        ]
    }

    fn owner_ready(&self) -> Vec<String> {
        let panes = self.review();
        corral_ops::verdict(&self.ctx, &self.id, Verdict::Approve, "Checked.").unwrap();
        self.sync();
        assert_eq!(self.ctx.store.get(&self.id).unwrap().status, Status::Review);
        assert!(self.ctx.store.get(&self.id).unwrap().awaiting_owner());
        for pane in &panes {
            assert!(self.pane_exists(pane), "owner gate must retain {pane}");
        }
        panes
    }

    fn assert_closed(&self, panes: &[String], ws: &str) {
        for pane in panes {
            assert!(!self.pane_exists(pane), "owned pane leaked: {pane}");
            assert!(
                self.called(&["pane", "close", pane]),
                "not explicitly closed: {pane}"
            );
        }
        assert!(
            !self.workspace_exists(ws),
            "empty owned workspace leaked: {ws}"
        );
        assert!(self.called(&["workspace", "close", ws]));
    }

    fn restore_inventory(&self, snapshot: &Value) {
        self.edit(|s| {
            for key in ["workspaces", "panes"] {
                s[key]
                    .as_object_mut()
                    .unwrap()
                    .extend(snapshot[key].as_object().unwrap().clone());
            }
        });
    }
}

fn approve_case(strategy: ApproveStrategy, remove_worktree: bool, policy_keeps: bool) {
    let mut fx = Fx::new();
    if policy_keeps {
        fx.ctx.store.config.review.remove_worktree = Some(false);
    }
    if strategy == ApproveStrategy::Rebase {
        std::fs::write(fx.app.join("main-only.txt"), "main advanced\n").unwrap();
        git(&fx.app, &["add", "main-only.txt"]);
        git(&fx.app, &["commit", "-q", "-m", "advance main"]);
    }
    // Verify the integrated candidate before testing successful approval cleanup.
    let panes = fx.owner_ready();
    let run = fx.run();
    let ws = run.herdr.unwrap().workspace_id;
    fx.reset_calls();
    corral_ops::approve(&fx.ctx, &fx.id, strategy, remove_worktree, false).unwrap();
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().status, Status::Done);
    assert_eq!(run.worktree.exists(), !remove_worktree || policy_keeps);
    assert_eq!(
        fx.app.join("feature.txt").exists(),
        strategy != ApproveStrategy::KeepBranch
    );
    fx.assert_closed(&panes, &ws);
}

macro_rules! approval_test {
    ($name:ident, $strategy:ident, $remove:expr) => {
        #[test]
        fn $name() {
            approve_case(ApproveStrategy::$strategy, $remove, false);
        }
    };
}

approval_test!(approve_fast_forward_releases_tools, FastForward, true);
approval_test!(approve_rebase_releases_tools, Rebase, true);
approval_test!(approve_keep_branch_releases_tools, KeepBranch, true);
approval_test!(
    approve_fast_forward_kept_worktree_releases_tools,
    FastForward,
    false
);
approval_test!(approve_rebase_kept_worktree_releases_tools, Rebase, false);
approval_test!(
    approve_keep_branch_kept_worktree_releases_tools,
    KeepBranch,
    false
);

#[test]
fn approval_policy_keeps_worktree_but_releases_terminal_resources() {
    approve_case(ApproveStrategy::Rebase, true, true);
}

#[test]
fn discard_releases_agent_gate_review_workspace_and_worktree() {
    let fx = Fx::new();
    let panes = fx.review();
    let run = fx.run();
    fx.reset_calls();
    let task = corral_ops::discard(&fx.ctx, &fx.id).unwrap();
    assert_eq!(task.status, Status::Dropped);
    assert_eq!(
        task.current_run().unwrap().outcome,
        Some(corral_core::RunOutcome::Discarded)
    );
    assert!(!run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_err());
    fx.assert_closed(&panes, &run.herdr.unwrap().workspace_id);
}

#[test]
fn auto_land_releases_agent_gate_reviewer_and_workspace() {
    let mut fx = Fx::new();
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    fx.ctx.store.save_config().unwrap();
    let panes = fx.review();
    let run = fx.run();
    corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Approve, "Checked.").unwrap();
    fx.reset_calls();
    fx.sync();
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().status, Status::Done);
    assert_eq!(fx.run().autopilot.unwrap().stage, AutoStage::Done);
    assert!(fx.app.join("feature.txt").exists());
    assert!(!run.worktree.exists());
    fx.assert_closed(&panes, &run.herdr.unwrap().workspace_id);
}

fn autopilot_rejection(max_rounds: u32, status: Status, stage: AutoStage) {
    let mut fx = Fx::new();
    fx.ctx.store.config.review.max_rounds = Some(max_rounds);
    fx.ctx.store.save_config().unwrap();
    let panes = fx.review();
    let run = fx.run();
    let h = run.herdr.unwrap();
    corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Changes, "Fix the edge case.").unwrap();
    fx.reset_calls();
    fx.sync();
    let task = fx.ctx.store.get(&fx.id).unwrap();
    assert_eq!(task.status, status);
    assert_eq!(task.runs.len(), 1);
    assert_eq!(fx.run().autopilot.unwrap().stage, stage);
    assert!(run.worktree.exists());
    assert!(fx.pane_exists(&h.pane_id));
    assert!(fx.workspace_exists(&h.workspace_id));
    assert!(!fx.called(&["pane", "close", &h.pane_id]));
    assert!(!fx.called(&["workspace", "close", &h.workspace_id]));
    for pane in &panes[1..] {
        assert!(!fx.pane_exists(pane), "autopilot leaked review tool {pane}");
        assert!(fx.called(&["pane", "close", pane]));
    }
}

#[test]
fn autopilot_changes_releases_tools_but_keeps_agent_and_workspace() {
    autopilot_rejection(2, Status::Running, AutoStage::Changes);
}

#[test]
fn autopilot_escalation_releases_tools_but_keeps_agent_and_workspace() {
    autopilot_rejection(0, Status::NeedsMe, AutoStage::Escalated);
}

#[test]
fn send_back_releases_review_tools_but_keeps_agent_and_workspace() {
    let fx = Fx::new();
    let panes = fx.owner_ready();
    let before = fx.run();
    let h = before.herdr.as_ref().unwrap();
    fx.reset_calls();
    let task = corral_ops::request_changes(&fx.ctx, &fx.id, "Fix the edge case.").unwrap();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.current_run().unwrap().id, before.id);
    assert!(task.current_run().unwrap().outcome.is_none());
    assert!(fx.pane_exists(&h.pane_id));
    assert!(fx.workspace_exists(&h.workspace_id));
    assert!(before.worktree.exists());
    assert!(!fx.called(&["pane", "close", &h.pane_id]));
    assert!(!fx.called(&["workspace", "close", &h.workspace_id]));
    for pane in &panes[1..] {
        assert!(!fx.pane_exists(pane), "send-back leaked review tool {pane}");
    }
    let calls = fx.state()["calls"].to_string();
    let head = format!(r#"["agent","prompt","{}","NEW CHANGES ROUND 1:"#, h.pane_id);
    assert!(calls.contains(&head) && calls.contains(r#" Fix the edge case."]"#));
}

fn terminal_history(status: Status) {
    let fx = Fx::new();
    let mut owned = fx.review();
    let old = fx.run();
    let inventory = fx.state();
    assert!(matches!(
        corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap(),
        DispatchOutcome::Started { .. }
    ));
    owned.extend(fx.review());
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().runs.len(), 2);
    // Model resources left behind by a previous process/version, without changing ownership records.
    fx.restore_inventory(&inventory);
    corral_ops::move_task(&fx.ctx, &fx.id, status, true).unwrap();
    fx.reset_calls();
    fx.sync();
    let current = fx.run();
    for pane in &owned {
        assert!(
            !fx.pane_exists(pane),
            "{status:?} leaked historical pane {pane}"
        );
        assert!(fx.called(&["pane", "close", pane]));
    }
    for run in [old, current] {
        let ws = run.herdr.unwrap().workspace_id;
        assert!(!fx.workspace_exists(&ws));
        assert!(fx.called(&["workspace", "close", &ws]));
    }
    fx.reset_calls();
    fx.sync();
    assert!(
        fx.state()["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a[1] != "close"),
        "repeated sync must skip already dead resources"
    );
}

#[test]
fn sync_done_reaps_every_run_including_old_gate_and_reviewer() {
    terminal_history(Status::Done);
}

#[test]
fn sync_dropped_reaps_every_run_including_old_gate_and_reviewer() {
    terminal_history(Status::Dropped);
}

#[test]
fn sync_uses_gate_started_and_review_started_for_replaced_tools_in_dispatch_workspace() {
    let fx = Fx::new();
    let panes = fx.review();
    let historical = fx.state();
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
    corral_ops::review_run(&fx.ctx, &fx.id, Some("rev")).unwrap();
    let run = fx.run();
    assert_ne!(run.gate.unwrap().pane.as_deref(), Some(panes[1].as_str()));
    assert_ne!(run.review.unwrap().pane.as_deref(), Some(panes[2].as_str()));
    let events = fx.ctx.store.events().unwrap();
    assert!(!events.iter().any(|e| e.kind == "run_workspace_created"));
    for (kind, pane) in [("gate_started", &panes[1]), ("review_started", &panes[2])] {
        assert!(events.iter().any(|e| {
            e.kind == kind
                && e.task.as_deref() == Some(&fx.id)
                && e.detail["run"] == "R1"
                && e.detail["pane"] == *pane
        }));
    }
    fx.restore_inventory(&historical);
    corral_ops::move_task(&fx.ctx, &fx.id, Status::Done, true).unwrap();
    fx.reset_calls();
    fx.sync();
    fx.assert_closed(&panes, &fx.run().herdr.unwrap().workspace_id);
}

#[test]
fn discard_preserves_current_pm_reviewer_and_its_workspace() {
    let fx = Fx::new();
    let owned = fx.owner_ready();
    let pm = corral_ops::open_pm(&fx.ctx).unwrap();
    let record = corral_ops::review_run(&fx.ctx, &fx.id, Some("pm")).unwrap();
    assert_eq!(record.reviewer, "pm");
    assert_eq!(record.pane.as_deref(), Some(pm.as_str()));
    let pm_ws = fx.state()["panes"][&pm]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    fx.reset_calls();
    corral_ops::discard(&fx.ctx, &fx.id).unwrap();
    fx.sync();
    assert!(fx.pane_exists(&pm));
    assert!(fx.workspace_exists(&pm_ws));
    assert!(!fx.called(&["pane", "close", &pm]));
    assert!(!fx.called(&["workspace", "close", &pm_ws]));
    for pane in owned {
        assert!(!fx.pane_exists(&pane));
    }
}

#[test]
fn sync_reaps_event_only_historical_tools_and_fallback_workspaces() {
    let fx = Fx::new();
    fx.done();
    let h = fx.run().herdr.unwrap();
    // Losing the dispatch workspace forces each tool to open its own owned fallback workspace.
    fx.edit(|s| {
        s["panes"].as_object_mut().unwrap().remove(&h.pane_id);
        s["workspaces"]
            .as_object_mut()
            .unwrap()
            .remove(&h.workspace_id);
    });
    let gate = corral_ops::gate_start(&fx.ctx, &fx.id)
        .unwrap()
        .pane
        .unwrap();
    assert!(corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().is_some());
    let review = corral_ops::review_run(&fx.ctx, &fx.id, Some("rev"))
        .unwrap()
        .pane
        .unwrap();
    let historical = fx.state();
    // Normal replacements overwrite the gate/review records; only creation events remember these.
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
    corral_ops::review_run(&fx.ctx, &fx.id, Some("rev")).unwrap();
    assert_ne!(fx.run().gate.unwrap().pane.as_deref(), Some(gate.as_str()));
    assert_ne!(
        fx.run().review.unwrap().pane.as_deref(),
        Some(review.as_str())
    );
    fx.restore_inventory(&historical);
    let inventory = fx.state();
    let events = fx.ctx.store.events().unwrap();
    for (pane, info) in inventory["panes"].as_object().unwrap() {
        assert!(
            events.iter().any(|e| {
                e.task.as_deref() == Some(&fx.id)
                    && e.kind == "run_workspace_created"
                    && e.detail["run"] == "R1"
                    && e.detail["pane"] == *pane
                    && e.detail["workspace"] == info["workspace_id"]
            }),
            "fallback workspace ownership not logged for {pane}"
        );
    }
    corral_ops::move_task(&fx.ctx, &fx.id, Status::Dropped, false).unwrap();
    fx.reset_calls();
    fx.sync();
    for pane in inventory["panes"].as_object().unwrap().keys() {
        assert!(
            !fx.pane_exists(pane),
            "historical fallback pane leaked: {pane}"
        );
        assert!(fx.called(&["pane", "close", pane]));
    }
    for ws in inventory["workspaces"].as_object().unwrap().keys() {
        assert!(!fx.workspace_exists(ws), "fallback workspace leaked: {ws}");
        assert!(fx.called(&["workspace", "close", ws]));
    }
}

#[test]
fn cleanup_preserves_user_panes_user_workspaces_and_pm_review_history() {
    let fx = Fx::new();
    let owned = fx.owner_ready();
    let h = fx.run().herdr.unwrap();
    let pm = corral_ops::open_pm(&fx.ctx).unwrap();
    let pm_ws = fx.state()["panes"][&pm]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    // PM review is replaced, leaving both a PM accounting child and a historical review_started event.
    corral_ops::review_run(&fx.ctx, &fx.id, Some("pm")).unwrap();
    corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Approve, "PM checked.").unwrap();
    let review = corral_ops::review_run(&fx.ctx, &fx.id, Some("rev"))
        .unwrap()
        .pane
        .unwrap();
    fx.edit(|s| {
        // Deliberately misleading labels/cwd must never confer ownership.
        s["panes"]["user-in-run"] = json!({
            "pane_id":"user-in-run","workspace_id":h.workspace_id,
            "tab_id":"user-tab","title":"T-001 gate","cwd":fx.run().worktree
        });
        s["workspaces"]["user"] = json!({"workspace_id":"user","label":"T-001 review"});
        s["workspaces"]["user-empty"] = json!({"workspace_id":"user-empty","label":"T-001"});
        s["panes"]["user-pane"] = json!({
            "pane_id":"user-pane","workspace_id":"user","tab_id":"user-tab",
            "title":"corral agent","cwd":fx.run().worktree
        });
    });
    fx.reset_calls();
    corral_ops::discard(&fx.ctx, &fx.id).unwrap();
    fx.sync();
    for pane in ["user-in-run", "user-pane", pm.as_str()] {
        assert!(fx.pane_exists(pane), "non-owned pane closed: {pane}");
        assert!(!fx.called(&["pane", "close", pane]));
    }
    for ws in [
        "user",
        "user-empty",
        h.workspace_id.as_str(),
        pm_ws.as_str(),
    ] {
        assert!(fx.workspace_exists(ws), "protected workspace closed: {ws}");
        assert!(!fx.called(&["workspace", "close", ws]));
    }
    for pane in owned.iter().chain(std::iter::once(&review)) {
        assert!(!fx.pane_exists(pane));
    }
}

#[test]
fn sync_skips_dead_panes_but_closes_empty_owned_workspace() {
    let fx = Fx::new();
    let panes = fx.review();
    let ws = fx.run().herdr.unwrap().workspace_id;
    fx.edit(|s| s["panes"] = json!({}));
    corral_ops::move_task(&fx.ctx, &fx.id, Status::Done, true).unwrap();
    fx.reset_calls();
    fx.sync();
    for pane in &panes {
        assert!(
            !fx.called(&["pane", "close", pane]),
            "dead pane must not be closed"
        );
    }
    assert!(!fx.workspace_exists(&ws));
    assert!(fx.called(&["workspace", "close", &ws]));
    assert!(
        !fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "cleanup_failed")
    );
}

fn failure_probe(block_event_log: bool) {
    let fx = Fx::new();
    let panes = fx.review();
    let ws = fx.run().herdr.unwrap().workspace_id;
    fx.edit(|s| {
        s["fail"] = json!([format!("pane close {}", panes[0])]);
        if block_event_log {
            s["block_event_log"] = json!(fx.ctx.store.dir().join("events.jsonl"));
        }
    });
    fx.reset_calls();
    assert_eq!(
        corral_ops::discard(&fx.ctx, &fx.id).unwrap().status,
        Status::Dropped
    );
    assert!(fx.pane_exists(&panes[0]));
    assert!(fx.workspace_exists(&ws));
    assert!(
        !fx.called(&["workspace", "close", &ws]),
        "failed pane close must protect workspace"
    );
    for pane in &panes[1..] {
        assert!(
            !fx.pane_exists(pane),
            "one failure must not stop other pane closes"
        );
    }
    if !block_event_log {
        assert!(fx.ctx.store.events().unwrap().iter().any(|e| {
            e.kind == "cleanup_failed"
                && e.task.as_deref() == Some(&fx.id)
                && e.detail["resource"] == panes[0]
                && e.detail["error"]
                    .as_str()
                    .unwrap()
                    .contains("injected-cleanup-error")
        }));
    }
    fx.edit(|s| {
        s["fail"] = json!([format!("workspace close {ws}")]);
        s.as_object_mut().unwrap().remove("block_event_log");
    });
    fx.sync();
    assert!(!fx.pane_exists(&panes[0]), "sync retries the failed pane");
    assert!(fx.workspace_exists(&ws));
    assert!(fx.ctx.store.events().unwrap().iter().any(|e| {
        e.kind == "cleanup_failed"
            && e.task.as_deref() == Some(&fx.id)
            && e.detail["resource"] == ws
            && e.detail["error"]
                .as_str()
                .unwrap()
                .contains("injected-cleanup-error")
    }));
    fx.edit(|s| s["fail"] = json!([]));
    fx.sync();
    assert!(
        !fx.workspace_exists(&ws),
        "sync retries the failed workspace"
    );
}

#[test]
fn cleanup_errors_warn_but_event_append_failures_stay_quiet() {
    const PROBE: &str = "CORRAL_CLEANUP_TEST_PROBE";
    if std::env::var_os(PROBE).is_some() {
        failure_probe(false);
        failure_probe(true);
        return;
    }
    // Capture the public ops' stderr in an isolated test process, without changing global env or
    // redirecting descriptors shared with parallel tests. Dispatch's normal diagnostics are allowed.
    let tmp = tempfile::tempdir().unwrap();
    let output = support::command(std::env::current_exe().unwrap(), tmp.path())
        .args([
            "--exact",
            "cleanup_errors_warn_but_event_append_failures_stay_quiet",
            "--nocapture",
        ])
        .env(PROBE, "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "probe failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning: discard"), "{stderr}");
    assert!(stderr.contains("injected-cleanup-error"), "{stderr}");
    for forbidden in ["cleanup_failed", "events.jsonl"] {
        assert!(
            !stderr.contains(forbidden),
            "cleanup/event failure printed: {stderr}"
        );
    }
}
