//! sync against a fake herdr binary: idle debounce, idle_no_report, OmO session-file activity.

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use corral_core::{HerdrRef, Run, RunMode, RunOutcome, Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use jiff::{SignedDuration, Timestamp};

const FAKE_HERDR: &str = r#"#!/usr/bin/env bash
dir="$(dirname "$0")"
case "$1 $2" in
  "workspace list") echo '{"id":"x","result":{"type":"workspace_list","workspaces":[]}}' ;;
  "pane list") printf '{"id":"x","result":{"type":"pane_list","panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent":"claude","agent_status":"%s"}]}}\n' "$(cat "$dir/status")" ;;
  "pane read")
    # Test hook: a concurrent CLI write lands while sync is between its load and its save.
    if [ -f "$dir/interleave" ]; then mv "$dir/interleave" "$(cat "$dir/interleave_to")"; fi
    echo 'QUESTION: ship it?' ;;
  "notification show") printf '%s\n' "$3" >> "$dir/notify.log"; echo '{"id":"x","result":{"type":"ok"}}' ;;
  *) echo "{\"id\":\"x\",\"error\":{\"code\":\"unexpected\",\"message\":\"$*\"}}"; exit 1 ;;
esac
"#;

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    bin_dir: PathBuf,
    agent_dir: PathBuf,
    worktree: PathBuf,
    t0: Timestamp,
}

impl Fx {
    fn new(harness: &str) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("fake-herdr");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("herdr");
        std::fs::write(&bin, FAKE_HERDR).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let agent_dir = tmp.path().join("omo-agent");
        let worktree = tmp.path().join("app/.worktrees/T-001");
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.sync.idle_after_secs = 120;
        store.config.sync.omo_agent_dir = Some(agent_dir.clone());
        let t0: Timestamp = "2026-10-15T09:00:00Z".parse().unwrap();
        let task = store
            .create("Sync me", Some("app"), Status::Ready, "")
            .unwrap();
        store
            .update(&task.id, |t| {
                t.status = Status::Running;
                t.runs.push(run(harness, worktree.clone(), t0));
                Ok(())
            })
            .unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        let fx = Fx {
            _tmp: tmp,
            ctx,
            bin_dir,
            agent_dir,
            worktree,
            t0,
        };
        fx.herdr_says("working");
        fx
    }

    fn herdr_says(&self, status: &str) {
        std::fs::write(self.bin_dir.join("status"), status).unwrap();
    }

    fn at(&self, secs: i64) -> Timestamp {
        self.t0 + SignedDuration::from_secs(secs)
    }

    fn sync(&self, secs: i64) -> Task {
        corral_ops::sync_at(&self.ctx, self.at(secs)).unwrap();
        self.ctx.store.get("T-001").unwrap()
    }

    fn notifications(&self) -> String {
        std::fs::read_to_string(self.bin_dir.join("notify.log")).unwrap_or_default()
    }

    fn events(&self, kind: &str) -> usize {
        let events = self.ctx.store.events().unwrap();
        events.iter().filter(|e| e.kind == kind).count()
    }

    fn omo_session_touched(&self, secs: i64) {
        let dir = self
            .agent_dir
            .join("sessions")
            .join("--".to_string() + &slug(&self.worktree) + "--");
        std::fs::create_dir_all(&dir).unwrap();
        let file = File::create(dir.join("2026-10-15T09-00-00-000Z_x.jsonl")).unwrap();
        file.set_modified(SystemTime::from(self.at(secs))).unwrap();
    }
}

fn slug(p: &Path) -> String {
    p.to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "-")
}

fn run(harness: &str, worktree: PathBuf, started: Timestamp) -> Run {
    Run {
        id: "R1".into(),
        agent: harness.into(),
        harness: Some(harness.into()),
        model: None,
        effort: None,
        mode: RunMode::Interactive,
        worktree,
        branch: "corral/T-001".into(),
        base: "main".into(),
        base_sha: "0".repeat(40),
        herdr: Some(HerdrRef {
            workspace_id: "w1".into(),
            pane_id: "w1:p1".into(),
        }),
        started,
        ended: None,
        outcome: None,
        summary: None,
        blocked_excerpt: None,
        agent_state: None,
        seen_working: false,
        gate: None,
        idle_since: None,
        idle_no_report: false,
        review: None,
        usage: None,
        usage_note: None,
        autopilot: None,
    }
}

#[test]
fn short_idle_stays_running_without_flag_event_or_notify() {
    let fx = Fx::new("fake");
    assert!(fx.sync(0).current_run().unwrap().seen_working);
    fx.herdr_says("idle");
    fx.sync(10);
    let t = fx.sync(100);
    let r = t.current_run().unwrap();
    assert_eq!(t.status, Status::Running);
    assert_eq!(r.agent_state.as_deref(), Some("idle"));
    assert_eq!(r.idle_since, Some(fx.at(10)));
    assert!(!r.idle_no_report);
    assert_eq!(fx.events("idle_no_report"), 0);
    assert_eq!(fx.events("needs_me"), 0);
    assert_eq!(fx.notifications(), "");
}

#[test]
fn long_idle_sets_flag_once_and_stays_running() {
    let fx = Fx::new("fake");
    fx.sync(0);
    fx.herdr_says("idle");
    fx.sync(10);
    let t = fx.sync(130);
    assert_eq!(t.status, Status::Running);
    assert!(t.current_run().unwrap().idle_no_report);
    let t = fx.sync(400);
    assert_eq!(t.status, Status::Running);
    assert!(t.current_run().unwrap().idle_no_report);
    assert_eq!(fx.events("idle_no_report"), 1);
    assert_eq!(fx.events("needs_me"), 0);
    assert_eq!(fx.notifications(), "");
}

#[test]
fn blocked_moves_to_needs_me_and_notifies() {
    let fx = Fx::new("fake");
    fx.sync(0);
    fx.herdr_says("blocked");
    let t = fx.sync(5);
    assert_eq!(t.status, Status::NeedsMe);
    assert_eq!(
        t.current_run().unwrap().blocked_excerpt.as_deref(),
        Some("QUESTION: ship it?")
    );
    assert_eq!(fx.notifications(), "▲ T-001 needs you\n");
    assert_eq!(fx.events("needs_me"), 1);
}

#[test]
fn done_landing_mid_sync_is_not_overwritten() {
    let fx = Fx::new("fake");
    fx.sync(0);
    let mut done = fx.ctx.store.get("T-001").unwrap();
    done.status = Status::Review;
    let r = done.current_run_mut().unwrap();
    r.outcome = Some(RunOutcome::Done);
    r.summary = Some("shipped".into());
    r.ended = Some(fx.at(5));
    std::fs::write(
        fx.bin_dir.join("interleave"),
        corral_core::render_task(&done),
    )
    .unwrap();
    std::fs::write(
        fx.bin_dir.join("interleave_to"),
        done.path.display().to_string(),
    )
    .unwrap();
    // Blocked makes sync read the pane after loading the Running task: `done` lands right there.
    fx.herdr_says("blocked");
    let t = fx.sync(5);
    assert!(
        !fx.bin_dir.join("interleave").exists(),
        "the concurrent write must happen mid-sync"
    );
    assert_eq!(t.status, Status::Review);
    let r = t.current_run().unwrap();
    assert_eq!(r.outcome, Some(RunOutcome::Done));
    assert_eq!(r.summary.as_deref(), Some("shipped"));
    assert_eq!(r.blocked_excerpt, None);
    assert_eq!(fx.events("needs_me"), 0);
}

#[test]
fn review_task_untouched_whatever_herdr_reports() {
    let fx = Fx::new("fake");
    fx.sync(0);
    // Worst case: Review with the run still open, so only the status keeps sync away.
    let t = fx
        .ctx
        .store
        .update("T-001", |t| {
            t.status = Status::Review;
            Ok(())
        })
        .unwrap();
    let before = std::fs::read_to_string(&t.path).unwrap();
    for (i, status) in ["idle", "done", "blocked", "working"].iter().enumerate() {
        fx.herdr_says(status);
        fx.sync(10 + 200 * i as i64);
    }
    assert_eq!(std::fs::read_to_string(&t.path).unwrap(), before);
    for kind in ["needs_me", "resumed", "idle_no_report"] {
        assert_eq!(fx.events(kind), 0, "{kind}");
    }
    assert_eq!(fx.notifications(), "");
}

#[test]
fn omo_fresh_session_file_counts_as_working() {
    let fx = Fx::new("omo");
    fx.herdr_says("idle");
    fx.omo_session_touched(50);
    let t = fx.sync(60);
    let r = t.current_run().unwrap();
    assert_eq!(t.status, Status::Running);
    assert_eq!(r.agent_state.as_deref(), Some("working"));
    assert!(r.seen_working);
    assert_eq!(r.idle_since, None);
    let t = fx.sync(160);
    let r = t.current_run().unwrap();
    assert!(!r.idle_no_report && r.idle_since.is_none());

    // Stale session file: the debounce starts at the first sample that no longer sees activity.
    let t = fx.sync(200);
    let r = t.current_run().unwrap();
    assert_eq!(r.agent_state.as_deref(), Some("idle"));
    assert_eq!(r.idle_since, Some(fx.at(200)));
    assert!(fx.sync(330).current_run().unwrap().idle_no_report);
    assert_eq!(fx.notifications(), "");
}

#[test]
fn non_omo_run_ignores_omo_session_files() {
    let fx = Fx::new("claude");
    fx.sync(0);
    fx.herdr_says("idle");
    fx.omo_session_touched(50);
    let t = fx.sync(60);
    assert_eq!(t.current_run().unwrap().idle_since, Some(fx.at(60)));
}

#[test]
fn activity_clears_idle_flag() {
    let fx = Fx::new("fake");
    fx.sync(0);
    fx.herdr_says("idle");
    fx.sync(10);
    assert!(fx.sync(200).current_run().unwrap().idle_no_report);
    fx.herdr_says("working");
    let t = fx.sync(210);
    let r = t.current_run().unwrap();
    assert_eq!(t.status, Status::Running);
    assert!(!r.idle_no_report);
    assert_eq!(r.idle_since, None);
    assert_eq!(fx.notifications(), "");
}
