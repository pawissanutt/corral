//! T-575 rewinds through a real fenced resume: a worker's linked worktree with host-style
//! checkpoints in its bound Pi session, a host that died inside its second turn, foreign edits
//! between turns and while the rewind runs, and a restore interrupted after its first file.
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use corral_core::agent::{ReceiptState, RewindStage};
use corral_core::inbox::Recipient;
use serde_json::{Value, json};

use super::*;
use crate::agent::bridge::fixture::{self as f, HostProc};
use crate::agent::launch::{BOOTSTRAP_FILE, generation_dir};
use crate::agent::recovery::resume_container;
use crate::agent::test_support as ts;

fn run_git(dir: &Path, args: &[&str]) -> String {
    git(dir, args).unwrap()
}

fn read(dir: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(path)).ok()
}

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A bound worker whose run works in a linked worktree; the session file is written by `lines`.
struct Wt {
    fx: f::Fx,
    dir: PathBuf,
    host: HostProc,
    bound: ts::Host,
    lines: Vec<Value>,
    at: u32,
}

fn worktree() -> Wt {
    let fx = f::fixture(false, 5);
    let dir = fx.root().join("wt");
    run_git(
        &fx.app(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "corral/T-1",
            dir.to_str().unwrap(),
        ],
    );
    let update = |t: &mut corral_core::Task| {
        t.runs[0].worktree = dir.clone();
        Ok(())
    };
    fx.ctx.store.update(&fx.id, update).unwrap();
    let host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let header = json!({"type": "session", "version": 3, "id": f::SESSION,
        "timestamp": "2026-10-29T00:00:00Z"});
    Wt {
        fx,
        dir,
        host,
        bound,
        lines: vec![header],
        at: 0,
    }
}

impl Wt {
    fn stamp(&mut self) -> String {
        self.at += 1;
        format!("2026-10-29T00:00:{:02}Z", self.at)
    }

    fn entry(&mut self, id: &str, value: Value) {
        let mut value = value;
        value["id"] = json!(id);
        value["timestamp"] = json!(self.stamp());
        self.lines.push(value);
        let text: String = self.lines.iter().map(|l| format!("{l}\n")).collect();
        f::write_session(&self.bound, &text);
    }

    /// The checkpoint the host takes: a private-index snapshot, its ref, then the session entry.
    fn checkpoint(&mut self, phase: &str, turn: u64, leaf: Option<&str>) -> String {
        let id = format!("g1-t{turn}");
        let (commit, head) =
            snapshot(&self.dir, &format!("corral checkpoint {id} {phase}")).unwrap();
        let reference = format!("refs/worktree/corral/checkpoints/R1/{id}-{phase}");
        run_git(&self.dir, &["update-ref", &reference, &commit]);
        let mut data = json!({"id": id, "phase": phase, "generation": 1, "turn": turn,
            "commit": commit, "head": head, "ref": reference});
        if let Some(leaf) = leaf {
            data["leaf"] = json!(leaf);
        }
        self.entry(
            &format!("cp-{id}-{phase}"),
            json!({"type": "custom", "customType": CHECKPOINT_ENTRY, "data": data}),
        );
        commit
    }

    fn assistant(&mut self, id: &str, response: &str) {
        let message = json!({"role": "assistant", "model": "gpt-fixture", "responseId": response,
            "content": [{"type": "text", "text": "ok"}], "stopReason": "stop",
            "usage": {"input": 100, "output": 10, "cost": {"total": 0.25}}});
        self.entry(id, json!({"type": "message", "message": message}));
    }

    fn container(&self) -> ContainerRef {
        self.fx.worker()
    }

    fn state(&self) -> AgentRunState {
        worker(&self.fx.ctx, &self.container()).unwrap().state
    }

    fn usage(&self) -> String {
        let task = self.fx.task();
        format!(
            "{:?}",
            crate::agent::usage::collect(&task.runs[0], &self.fx.ctx.store).unwrap()
        )
    }

    /// Two turns: the first writes notes.txt; the owner then edits `tracked` and adds owner.txt
    /// between turns; the second commits notes.txt, rewrites it, adds new/deep.txt and dies.
    fn two_turns(&mut self) {
        self.entry(
            "b1",
            json!({"type": "custom", "customType": "corral_host_bound", "data": {}}),
        );
        self.checkpoint("start", 1, Some("b1"));
        write(&self.dir, "notes.txt", "turn1\n");
        self.assistant("a1", "resp-1");
        self.checkpoint("end", 1, None);
        write(&self.dir, "tracked", "owner edit\n");
        write(&self.dir, "owner.txt", "owner\n");
        self.checkpoint("start", 2, Some("a1"));
        run_git(&self.dir, &["add", "notes.txt"]);
        run_git(&self.dir, &["commit", "-qm", "agent commit"]);
        write(&self.dir, "notes.txt", "turn2\n");
        write(&self.dir, "new/deep.txt", "agent\n");
        self.assistant("a2", "resp-2");
        self.host.kill();
    }
}

fn interrupt_after_first_restore(wt: &Wt, to: &str) -> String {
    let hook = |point: &str| {
        if point == "rewind_restored_path" {
            panic!("rewind interrupted");
        }
    };
    crate::agent::receipts::FAILPOINT.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
    let result = catch_unwind(AssertUnwindSafe(|| {
        rewind_container(&wt.fx.ctx, &wt.container(), Some(to), false)
    }));
    crate::agent::receipts::FAILPOINT.with(|h| *h.borrow_mut() = None);
    assert!(result.is_err(), "the restore was interrupted");
    wt.state().pending_rewind().unwrap().id.clone()
}

#[test]
fn an_interrupted_rewind_continues_to_the_checkpoint_and_keeps_receipts_usage_and_foreign_edits() {
    // Given a worker whose host died inside turn 2, a receipt and billed usage on that turn:
    let mut wt = worktree();
    wt.two_turns();
    let start_head = wt.state().rewinds.len();
    assert_eq!(start_head, 0);
    let receipt = ts::receipt(
        &wt.bound,
        "c-kept",
        "progress",
        "committed",
        json!({"ok": true}),
    );
    let add = |t: &mut corral_core::Task| {
        t.runs[0]
            .agent_host
            .as_mut()
            .unwrap()
            .receipts
            .push(receipt.clone());
        Ok(())
    };
    wt.fx.ctx.store.update(&wt.fx.id, add).unwrap();
    let usage = wt.usage();
    let before_head = run_git(&wt.dir, &["rev-parse", "HEAD"]);
    let listed = list(&wt.fx.ctx, &wt.container()).unwrap();
    let ids: Vec<&str> = listed["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["g1-t1", "g1-t2"]);

    // When the owner rewinds to the start of turn 2 and the restore is interrupted after one file:
    let id = interrupt_after_first_restore(&wt, "g1-t2");

    // Then the old generation is fenced for the rewind, the stage says how far it got, and a
    // plain resume refuses to step over it:
    let state = wt.state();
    assert_eq!(
        state.generation(1).unwrap().fenced.as_ref().unwrap().reason,
        "rewind"
    );
    let pending = state.pending_rewind().unwrap();
    assert_eq!(pending.stage, RewindStage::Restoring);
    let saved = pending.before.clone().unwrap();
    let before_ref = pending.before_ref.clone().unwrap();
    assert_eq!(run_git(&wt.dir, &["rev-parse", &before_ref]), saved);
    let refused = resume_container(&wt.fx.ctx, &wt.container(), false).unwrap_err();
    assert!(
        format!("{refused:#}").contains("rewind_pending"),
        "{refused:#}"
    );
    assert_eq!(
        wt.state().binding.generation,
        1,
        "the refused resume opened nothing"
    );

    // And when the owner edits a file the rewind still had to restore, then continues it:
    write(&wt.dir, "new/deep.txt", "owner during rewind\n");
    let outcome = rewind_container(&wt.fx.ctx, &wt.container(), None, false).unwrap();

    // Then turn 2's start is back: notes.txt as turn 1 left it, untracked again, and HEAD
    // before the agent's commit; edits that were not the agent's stay:
    assert_eq!(read(&wt.dir, "notes.txt").as_deref(), Some("turn1\n"));
    assert_ne!(run_git(&wt.dir, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(
        run_git(&wt.dir, &["rev-parse", "HEAD"]),
        run_git(&wt.dir, &["rev-parse", "corral/T-1"])
    );
    let status = run_git(&wt.dir, &["status", "--porcelain"]);
    assert!(status.contains("?? notes.txt"), "{status}");
    assert_eq!(read(&wt.dir, "tracked").as_deref(), Some("owner edit\n"));
    assert_eq!(read(&wt.dir, "owner.txt").as_deref(), Some("owner\n"));
    assert_eq!(
        read(&wt.dir, "new/deep.txt").as_deref(),
        Some("owner during rewind\n")
    );
    let done = &outcome["rewind"];
    assert_eq!(done["id"], json!(id));
    assert_eq!(done["stage"], "launched");
    assert_eq!(done["generation"], 2);
    assert_eq!(done["kept"], json!(["new/deep.txt"]));

    // And the next generation opens the same session at turn 2's prompt parent:
    let recipient = Recipient::Run {
        task: wt.fx.id.clone(),
        run: "R1".into(),
    };
    let dir = generation_dir(&wt.fx.ctx.store, &recipient, 2).unwrap();
    let bootstrap: Value =
        serde_json::from_slice(&std::fs::read(dir.join(BOOTSTRAP_FILE)).unwrap()).unwrap();
    assert_eq!(bootstrap["resume_leaf"], "a1");
    assert_eq!(
        bootstrap["resume_session"],
        json!(f::session_file(&wt.bound))
    );
    assert_eq!(
        wt.state().rewind_cursor(),
        None,
        "a launched rewind's cursor is used once"
    );

    // And nothing that was recorded is rolled back: the saved worktree, receipts and usage:
    assert_eq!(
        run_git(&wt.dir, &["show", &format!("{saved}:notes.txt")]),
        "turn2"
    );
    let after = wt.state();
    let kept = after.receipt(&receipt.request_id).unwrap();
    assert_eq!(kept.state, ReceiptState::Committed);
    assert_eq!(wt.usage(), usage);
    assert_eq!(wt.fx.events("agent_rewound").len(), 1);
}

#[test]
fn a_path_the_owner_also_changed_between_turns_refuses_the_rewind_before_anything_is_fenced() {
    // Given an owner edit between the turns to the file both turns of the agent changed:
    let mut wt = worktree();
    wt.entry(
        "b1",
        json!({"type": "custom", "customType": "corral_host_bound", "data": {}}),
    );
    wt.checkpoint("start", 1, Some("b1"));
    write(&wt.dir, "notes.txt", "turn1\n");
    wt.checkpoint("end", 1, None);
    write(&wt.dir, "notes.txt", "owner\n");
    wt.checkpoint("start", 2, Some("b1"));
    write(&wt.dir, "notes.txt", "turn2\n");
    wt.host.kill();

    // When the owner rewinds to turn 1:
    let refused = rewind_container(&wt.fx.ctx, &wt.container(), Some("g1-t1"), false).unwrap_err();

    // Then it is refused naming the path, and nothing was recorded, fenced or written:
    let text = format!("{refused:#}");
    assert!(
        text.contains("rewind_conflict") && text.contains("notes.txt"),
        "{text}"
    );
    let state = wt.state();
    assert!(state.rewinds.is_empty());
    assert!(state.generation(1).unwrap().fenced.is_none());
    assert_eq!(read(&wt.dir, "notes.txt").as_deref(), Some("turn2\n"));
}

#[test]
fn an_aborted_rewind_puts_restored_files_back_and_lets_resume_continue() {
    // Given a rewind interrupted after its first restored file:
    let mut wt = worktree();
    wt.two_turns();
    let head = run_git(&wt.dir, &["rev-parse", "HEAD"]);
    interrupt_after_first_restore(&wt, "g1-t2");

    // When the owner aborts it:
    let aborted = abort(&wt.fx.ctx, &wt.container()).unwrap();

    // Then the worktree is as the fence left it and an ordinary resume is admitted:
    assert_eq!(aborted["rewind"]["stage"], "aborted");
    assert_eq!(read(&wt.dir, "notes.txt").as_deref(), Some("turn2\n"));
    assert_eq!(read(&wt.dir, "new/deep.txt").as_deref(), Some("agent\n"));
    assert_eq!(run_git(&wt.dir, &["rev-parse", "HEAD"]), head);
    let resumed = resume_container(&wt.fx.ctx, &wt.container(), false).unwrap();
    assert_eq!(resumed["generation"], 2);
    let recipient = Recipient::Run {
        task: wt.fx.id.clone(),
        run: "R1".into(),
    };
    let dir = generation_dir(&wt.fx.ctx.store, &recipient, 2).unwrap();
    let bootstrap: Value =
        serde_json::from_slice(&std::fs::read(dir.join(BOOTSTRAP_FILE)).unwrap()).unwrap();
    assert!(bootstrap.get("resume_leaf").is_none());
}

#[test]
fn a_live_host_without_replace_or_a_shared_checkout_is_refused_without_a_pending_rewind() {
    // Given a live host with a checkpoint:
    let mut wt = worktree();
    wt.entry(
        "b1",
        json!({"type": "custom", "customType": "corral_host_bound", "data": {}}),
    );
    wt.checkpoint("start", 1, Some("b1"));

    // When the owner rewinds without --replace, the refusal before the fence closes the record:
    let alive = rewind_container(&wt.fx.ctx, &wt.container(), Some("g1-t1"), false).unwrap_err();
    assert!(format!("{alive:#}").contains("host_alive"), "{alive:#}");
    let state = wt.state();
    assert_eq!(state.rewinds[0].stage, RewindStage::Aborted);
    assert!(state.pending_rewind().is_none());
    assert!(state.generation(1).unwrap().fenced.is_none());

    // And a run working in the project's own checkout is never rewound:
    let app = wt.fx.app();
    let shared = |t: &mut corral_core::Task| {
        t.runs[0].worktree = app.clone();
        Ok(())
    };
    wt.fx.ctx.store.update(&wt.fx.id, shared).unwrap();
    let refused = rewind_container(&wt.fx.ctx, &wt.container(), Some("g1-t1"), true).unwrap_err();
    assert!(
        format!("{refused:#}").contains("not_isolated"),
        "{refused:#}"
    );
}

fn set_mtime(path: &Path, at: std::time::SystemTime) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(at).unwrap();
}

#[test]
fn a_snapshot_rehashes_a_racily_clean_file_as_git_does() {
    // Given a committed file whose index entry is as new as the index file itself, then rewritten
    // with the same size and mtime (one coarse timestamp tick), with ctime not trusted:
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    run_git(dir, &["init", "-qb", "main"]);
    for (key, value) in [
        ("user.name", "t"),
        ("user.email", "t@example.invalid"),
        ("core.trustctime", "false"),
    ] {
        run_git(dir, &["config", key, value]);
    }
    let tick = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000);
    write(dir, "a.txt", "one\n");
    set_mtime(&dir.join("a.txt"), tick);
    run_git(dir, &["add", "a.txt"]);
    run_git(dir, &["commit", "-qm", "initial"]);
    let index = run_git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    );
    set_mtime(Path::new(&index), tick);
    write(dir, "a.txt", "two\n");
    set_mtime(&dir.join("a.txt"), tick);

    // When it is snapshotted, then the snapshot has the new content, as `git status` sees it:
    let (commit, _) = snapshot(dir, "racy").unwrap();
    assert_eq!(run_git(dir, &["show", &format!("{commit}:a.txt")]), "two");
    assert_eq!(run_git(dir, &["status", "--porcelain"]), "M a.txt");
}

fn point(id: &str, phase: CheckpointPhase, generation: u32) -> Checkpoint {
    let entry = CheckpointEntry {
        id: id.into(),
        phase,
        generation,
        turn: 1,
        commit: Some("c".into()),
        head: Some("h".into()),
        git_ref: None,
        leaf: Some("l".into()),
        error: None,
    };
    Checkpoint {
        entry,
        commit: "c".into(),
        head: "h".into(),
        at: None,
    }
}

#[test]
fn a_launched_rewind_abandons_the_checkpoints_after_its_target() {
    // Given turns g1-t1..t3, a rewind to g1-t2 that launched generation 2, and g2's own turn:
    use CheckpointPhase::{End, Start};
    let all = vec![
        point("g1-t1", Start, 1),
        point("g1-t1", End, 1),
        point("g1-t2", Start, 1),
        point("g1-t2", End, 1),
        point("g1-t3", Start, 1),
        point("g2-t1", Start, 2),
    ];
    let rewind: RewindRecord = serde_json::from_value(json!({
        "id": "rw-1", "checkpoint": "g1-t2", "commit": "c", "head": "h", "leaf": "l",
        "from_generation": 1, "stage": "launched", "generation": 2,
        "at": "2026-10-29T00:00:00Z", "updated": "2026-10-29T00:00:00Z"}))
    .unwrap();

    // Then the timeline continues from g1-t2's start into generation 2:
    let line = timeline(all, &[rewind]);
    let ids: Vec<(String, CheckpointPhase)> = line
        .iter()
        .map(|c| (c.entry.id.clone(), c.entry.phase))
        .collect();
    let expected = [
        ("g1-t1", Start),
        ("g1-t1", End),
        ("g1-t2", Start),
        ("g2-t1", Start),
    ];
    let expected: Vec<(String, CheckpointPhase)> =
        expected.iter().map(|(i, p)| ((*i).into(), *p)).collect();
    assert_eq!(ids, expected);
}
