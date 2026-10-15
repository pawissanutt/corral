use super::*;
use corral_core::{Status, Store};
use corral_herdr::Herdr;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

mod batches;
mod receipts;

const HERDR: &str = r#"#!/bin/sh
dir=$(dirname "$0")
case "$1 $2" in
  "workspace list") echo '{"result":{"workspaces":[]}}' ;;
  "pane get")
    if [ -f "$dir/missing" ]; then echo '{"error":{"code":"pane_not_found","message":"gone"}}'; exit 1; fi
    printf '{"result":{"pane":{"pane_id":"w1:p1","workspace_id":"%s","tab_id":"w1:t1","agent":"fake","agent_status":"working","cwd":"%s"}}}\n' "$(cat "$dir/workspace")" "$(cat "$dir/cwd")" ;;
  "pane process-info")
    printf '{"result":{"process_info":{"shell_pid":%s,"foreground_processes":[]}}}\n' "$(cat "$dir/pid")" ;;
  "pane read") printf '> \n' ;;
  "agent prompt")
    if [ -f "$dir/fail" ]; then echo '{"error":{"code":"failed","message":"transport refused"}}'; exit 1; fi
    printf '%s\n' "$3" >> "$dir/sent"
    printf '%s\n' "$4" >> "$dir/messages"
    echo '{"result":{}}' ;;
  *) echo '{"error":{"code":"unexpected","message":"unexpected command"}}'; exit 1 ;;
esac
"#;

pub(super) fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout)
        .unwrap()
        .trim_end()
        .to_string()
}

fn commit(repo: &Path) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", "fixture"]);
    git(repo, &["rev-parse", "HEAD"])
}

// A concurrent fork can briefly inherit another fixture's locked descriptors before exec.
// Serialize executable fixture lifetimes; state transitions still use an explicit clock.
static EXECUTABLE_FIXTURE: Mutex<()> = Mutex::new(());

struct Fx {
    _serial: MutexGuard<'static, ()>,
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    app: PathBuf,
    worker: PathBuf,
    bin: PathBuf,
    task: Task,
    now: Timestamp,
}

impl Fx {
    fn new() -> Self {
        let serial = EXECUTABLE_FIXTURE.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        std::fs::create_dir(&app).unwrap();
        git(&app, &["init", "-qb", "main"]);
        git(&app, &["config", "user.name", "corral test"]);
        git(&app, &["config", "user.email", "corral@test.invalid"]);
        git(&app, &["config", "commit.gpgsign", "false"]);
        std::fs::write(app.join("file.rs"), "fn initial() {}\n").unwrap();
        std::fs::write(app.join(".gitignore"), ".worktrees/\n").unwrap();
        std::fs::write(app.join("other.txt"), "initial\n").unwrap();
        let base = commit(&app);
        let worker = app.join(".worktrees/worker");
        corral_git::worktree_add(&app, &worker, "corral/worker", "main").unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let now: Timestamp = "2026-10-15T09:00:00Z".parse().unwrap();
        let task = store
            .create("Active worker", Some("app"), Status::Running, "")
            .unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"fake", "harness":"fake", "worktree":worker,
            "branch":"corral/worker", "base":"main", "base_sha":base,
            "started":now, "herdr":{"workspace_id":"w1","pane_id":"w1:p1"}
        }))
        .unwrap();
        let task = store
            .update(&task.id, |t| {
                t.runs.push(run);
                t.rounds = 1;
                Ok(())
            })
            .unwrap();
        let bin = tmp.path().join("fake-herdr");
        std::fs::create_dir(&bin).unwrap();
        std::fs::write(bin.join("herdr"), HERDR).unwrap();
        std::fs::set_permissions(bin.join("herdr"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::write(bin.join("cwd"), worker.to_str().unwrap()).unwrap();
        std::fs::write(bin.join("workspace"), "w1").unwrap();
        std::fs::write(bin.join("pid"), "1234").unwrap();
        let herdr = Herdr::with_bin(bin.join("herdr"), None).with_poll_wait(|_| {});
        Self {
            _serial: serial,
            _tmp: tmp,
            ctx: Ctx { store, herdr },
            app,
            worker,
            bin,
            task,
            now,
        }
    }

    fn worker_change(&self, path: &str) {
        std::fs::write(self.worker.join(path), "fn worker() {}\n").unwrap();
        commit(&self.worker);
    }

    fn land(&self, path: &str, content: &str) -> Landing {
        let previous = git(&self.app, &["rev-parse", "HEAD"]);
        if let Some(parent) = self.app.join(path).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(self.app.join(path), content).unwrap();
        let sha = commit(&self.app);
        self.record_landing(previous, sha)
    }

    fn record_landing(&self, previous: String, sha: String) -> Landing {
        let task = self
            .ctx
            .store
            .create("Landed feature", Some("app"), Status::Done, "")
            .unwrap();
        let landing = Landing {
            task: task.id.clone(),
            run: Some("R1".into()),
            sha,
            previous_sha: Some(previous),
        };
        self.ctx
            .store
            .log_event(
                Some(&task.id),
                "approved",
                json!({
                    "project":"app","base":"main","run":landing.run,"sha":landing.sha,
                    "previous_sha":landing.previous_sha,"strategy":"fast_forward"
                }),
            )
            .unwrap();
        landing
    }

    fn tick(&self, secs: i64) {
        reconcile(&self.ctx, self.now + SignedDuration::from_secs(secs), true).unwrap();
    }

    fn state(&self) -> State {
        serde_json::from_slice(&std::fs::read(state_path(&self.ctx, &self.task.id)).unwrap())
            .unwrap()
    }

    fn sent(&self) -> Vec<String> {
        std::fs::read_to_string(self.bin.join("sent"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn receipts(&self) -> Vec<Event> {
        self.ctx
            .store
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "main_movement_notice")
            .collect()
    }

    fn queued(&self, landing: Landing) -> State {
        let task = self.ctx.store.get(&self.task.id).unwrap();
        let notice = construct(&self.ctx, &task, &[landing]).unwrap().unwrap();
        State {
            target: Some(notice.target.clone()),
            notices: vec![notice],
            ..State::default()
        }
    }
}

#[test]
fn overlap_sends_context_once_without_changing_worker_state_or_git() {
    // Given committed overlap and independent worker state to preserve.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    let landed = fx.land("file.rs", "fn landed() {}\n");
    let before = fx.ctx.store.get(&fx.task.id).unwrap();
    let head = git(&fx.worker, &["rev-parse", "HEAD"]);
    let index = std::fs::read(fx.worker.join(".git")).unwrap();
    let index_path = git(&fx.worker, &["rev-parse", "--git-path", "index"]);
    let index_bytes = std::fs::read(&index_path).unwrap();
    let status = git(&fx.worker, &["status", "--porcelain=v1", "-z"]);
    // When a batch reaches its quiet boundary and is synced repeatedly.
    fx.tick(0);
    fx.tick(2);
    fx.tick(3);
    // Then one contextual submission has exact identities and no worker mutation.
    let state = fx.state();
    assert_eq!(fx.sent(), ["w1:p1"]);
    assert_eq!(state.notices.len(), 1);
    let notice = &state.notices[0];
    assert_eq!(notice.state, DeliveryState::Delivered);
    assert_eq!(notice.overlapping_paths, ["file.rs"]);
    assert_eq!(notice.landings, std::slice::from_ref(&landed));
    assert!(
        notice
            .context
            .missing_from_candidate
            .iter()
            .any(|c| c.sha == landed.sha && c.tasks.iter().any(|t| t.id == landed.task))
    );
    assert_eq!(notice.context.key.head, head);
    assert_eq!(notice.rust_headroom[0].candidate, Some(799));
    assert_eq!(notice.rust_headroom[0].main, Some(799));
    assert!(Path::new(notice.artifact.as_ref().unwrap()).is_file());
    let after = fx.ctx.store.get(&fx.task.id).unwrap();
    assert_eq!(after.status, before.status);
    assert_eq!(after.rounds, before.rounds);
    assert_eq!(after.runs, before.runs);
    assert_eq!(git(&fx.worker, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&fx.worker, &["branch", "--show-current"]),
        "corral/worker"
    );
    assert_eq!(git(&fx.worker, &["status", "--porcelain=v1", "-z"]), status);
    assert_eq!(std::fs::read(&index_path).unwrap(), index_bytes);
    assert_eq!(std::fs::read(fx.worker.join(".git")).unwrap(), index);
}

#[test]
fn disjoint_changes_send_no_notice() {
    // Given paths with no observed or declared overlap.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    fx.land("other.txt", "landed\n");
    // When the batch is reconciled.
    fx.tick(0);
    fx.tick(2);
    // Then no transport or notice artifact is created.
    assert!(fx.sent().is_empty());
    assert!(fx.state().notices.is_empty());
    assert!(fx.ctx.store.get(&fx.task.id).unwrap().artifacts.is_empty());
}

#[test]
fn main_rename_includes_the_old_path_for_overlap() {
    // Given the worker editing a path that main renamed.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    let previous = git(&fx.app, &["rev-parse", "HEAD"]);
    git(&fx.app, &["mv", "file.rs", "renamed.rs"]);
    let landing = fx.record_landing(previous, commit(&fx.app));
    // When constructing the overlap batch.
    let state = fx.queued(landing);
    // Then the original path still warns the worker.
    assert_eq!(state.notices[0].overlapping_paths, ["file.rs"]);
    assert!(
        state.notices[0]
            .context
            .main_changed_paths
            .contains(&"renamed.rs".into())
    );
}

#[test]
fn worker_rename_includes_the_old_path_for_overlap() {
    // Given a worker rename with a main edit to its original path.
    let fx = Fx::new();
    git(&fx.worker, &["mv", "file.rs", "worker.rs"]);
    commit(&fx.worker);
    let landing = fx.land("file.rs", "fn landed() {}\n");
    // When constructing the notice.
    let state = fx.queued(landing);
    // Then source overlap is retained, despite the worker deleting its old name.
    assert_eq!(state.notices[0].overlapping_paths, ["file.rs"]);
}

#[test]
fn staged_unstaged_and_untracked_paths_each_trigger_overlap() {
    for mode in ["staged", "unstaged", "untracked"] {
        // Given a worker change not present in its committed HEAD.
        let fx = Fx::new();
        let path = if mode == "untracked" {
            "new.rs"
        } else {
            "file.rs"
        };
        std::fs::write(fx.worker.join(path), "fn local() {}\n").unwrap();
        if mode == "staged" {
            git(&fx.worker, &["add", "--", path]);
        }
        let status = git(&fx.worker, &["status", "--porcelain=v1", "-z"]);
        let landing = fx.land(path, "fn landed() {}\n");
        // When the observed paths join committed task changes.
        let state = fx.queued(landing);
        // Then the factual path is included, leaving local state unchanged.
        assert_eq!(state.notices[0].observed_paths, [path]);
        assert_eq!(state.notices[0].overlapping_paths, [path]);
        assert_eq!(git(&fx.worker, &["status", "--porcelain=v1", "-z"]), status);
    }
}

#[test]
fn declared_write_scope_warns_before_the_worker_edits() {
    // Given a persisted declaration and no observed worker changes.
    let fx = Fx::new();
    fx.ctx
        .store
        .update(&fx.task.id, |t| {
            t.write_scope = vec!["src/**".into()];
            Ok(())
        })
        .unwrap();
    let landing = fx.land("src/new.rs", "fn landed() {}\n");
    // When constructing the batch from a reloaded task.
    let state = fx.queued(landing);
    // Then only the declaration explains this overlap.
    assert!(state.notices[0].observed_paths.is_empty());
    assert!(state.notices[0].context.candidate_changed_paths.is_empty());
    assert_eq!(state.notices[0].overlapping_paths, ["src/new.rs"]);
    assert_eq!(state.notices[0].declared_scope, ["src/**"]);
}

#[test]
fn fast_landings_coalesce_even_when_later_changes_revert_earlier_paths() {
    // Given two fast recorded landings, the second reverting the first.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    let first = fx.land("file.rs", "fn landed() {}\n");
    fx.tick(0);
    let second = fx.land("file.rs", "fn initial() {}\n");
    fx.tick(1);
    // When the quiet boundary is reached.
    fx.tick(2);
    assert!(fx.sent().is_empty());
    fx.tick(3);
    // Then there is one message for both landings, even though the net main diff is empty.
    assert_eq!(fx.sent().len(), 1);
    assert_eq!(fx.state().notices[0].landings, [first, second]);
    assert_eq!(fx.state().notices[0].overlapping_paths, ["file.rs"]);
}

#[test]
fn duplicate_approval_and_restart_do_not_repeat_a_delivered_notice() {
    // Given a delivered notice plus a replayed approval event.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    let landing = fx.land("file.rs", "fn landed() {}\n");
    fx.tick(0);
    fx.tick(2);
    fx.record_landing(landing.previous_sha.unwrap(), landing.sha);
    let restarted = Ctx {
        store: Store::open(fx.ctx.store.root()).unwrap(),
        herdr: fx.ctx.herdr.clone(),
    };
    // When a fresh process representation reconciles the durable queue.
    reconcile(&restarted, fx.now + SignedDuration::from_secs(5), true).unwrap();
    reconcile(&restarted, fx.now + SignedDuration::from_secs(7), true).unwrap();
    // Then both the notice identity and delivery remain singular.
    assert_eq!(fx.sent().len(), 1);
    assert_eq!(fx.state().notices.len(), 1);
}

#[test]
fn integrated_approval_and_keep_branch_approval_do_not_notify() {
    // Given a main commit already incorporated into the worker.
    let fx = Fx::new();
    let landing = fx.land("file.rs", "fn landed() {}\n");
    git(&fx.worker, &["merge", "--ff-only", "main"]);
    fx.worker_change("file.rs");
    fx.ctx
        .store
        .log_event(
            Some(&landing.task),
            "approved",
            json!({
                "sha":git(&fx.worker,&["rev-parse","HEAD"]), "strategy":"keep_branch",
                "project":"app", "base":"main"
            }),
        )
        .unwrap();
    // When the recorded approvals are reconciled.
    fx.tick(0);
    fx.tick(2);
    // Then neither represents missing integration work.
    assert!(fx.sent().is_empty());
}
