use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::Child;

use super::*;

impl Fx {
    fn marker(&self) -> String {
        let run = self.run();
        let path = self
            .ctx
            .store
            .scratch_path("tmp")
            .unwrap()
            .join("launch-exits")
            .join(format!("{}-{}.owner", self.id, run.id));
        std::fs::read_to_string(path).unwrap()
    }
}

struct Resident(Child);

impl Resident {
    fn start(path: &Path, harness: Option<&str>, marker: Option<&str>) -> Self {
        // Ready is emitted by the final process, after exec and cwd establishment.
        let mut command = crate::support::command("python3", path);
        command
            .args([
                "-c",
                "import signal; print('ready', flush=True); signal.pause()",
            ])
            .current_dir(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        if let Some(name) = harness {
            command.arg0(name);
        }
        if let Some(marker) = marker {
            command.env("CORRAL_PROCESS_OWNER", marker);
        } else {
            command.env_remove("CORRAL_PROCESS_OWNER");
        }
        let mut child = command.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        Self(child)
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}

#[test]
fn approval_closes_every_pane_before_removing_worktree() {
    let fx = Fx::new();
    let panes = fx.owner_ready();
    let run = fx.run();
    fx.edit(|s| s["require_worktree"] = json!(run.worktree));
    let line =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(line.contains("worktree removed"), "{line}");
    fx.assert_closed(&panes, &run.herdr.unwrap().workspace_id);
}

#[cfg(target_os = "linux")]
#[test]
fn landing_waits_for_recorded_shell_after_pane_close() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let pane = &run.herdr.as_ref().unwrap().pane_id;
    let close_pid = fx.state.parent().unwrap().join("closing-pid");
    let mut child = crate::support::command("python3", &run.worktree)
        .current_dir(&run.worktree)
        .env_remove("CORRAL_PROCESS_OWNER")
        .env_remove("HERDR_PANE_ID")
        .env("CLOSE_PID", close_pid)
        .args(["-c", "import os,signal,select;\ndef close(*_):\n p=int(open(os.environ['CLOSE_PID']).read()); fd=os.pidfd_open(p); poll=select.poll(); poll.register(fd,select.POLLIN); poll.poll(); os._exit(0)\nsignal.signal(signal.SIGTERM,close); print('ready',flush=True); signal.pause()"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    fx.edit(|s| s["shells"] = json!({pane: child.id()}));
    let result =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    child.wait().unwrap();
    assert!(result.contains("worktree removed"), "{result}");
    assert!(!run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_err());
}

#[test]
fn approval_protects_unowned_nested_cwd_and_outside_process() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let nested = run.worktree.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let mut inside = Resident::start(&nested, None, None);
    let mut outside = Resident::start(&fx.app, None, None);
    let line =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(line.contains("protected process"), "{line}");
    assert!(run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
    assert!(inside.0.try_wait().unwrap().is_none());
    assert!(outside.0.try_wait().unwrap().is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn owned_orphan_daemon_is_removed_after_landing() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let marker = fx.marker();
    let mut parent = crate::support::command("python3", &run.worktree);
    let output = parent.arg0("rust-analyzer-proc-macro-srv").current_dir(&run.worktree).env("CORRAL_PROCESS_OWNER", marker)
        .args(["-c", "import os,signal; p=os.fork(); os._exit(0) if p else None; print(os.getpid(),flush=True); signal.pause()"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).spawn().unwrap();
    let mut parent = output;
    let mut line = String::new();
    BufReader::new(parent.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let pid: u32 = line.trim().parse().unwrap();
    parent.wait().unwrap();
    let result =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    // Ensure a failing assertion does not leak the fixture process.
    let still_alive = std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok();
    if still_alive {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status();
    }
    assert!(result.contains("worktree removed"), "{result}");
    assert!(!still_alive);
    assert!(!run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn worker_language_server_survives_watchdog_then_is_reaped_at_landing() {
    use corral_ops::{ProcessControl, SystemProcesses};
    struct Aged;
    impl ProcessControl for Aged {
        fn inspect(&self, pid: u32) -> anyhow::Result<Option<corral_ops::ProcessSnapshot>> {
            Ok(SystemProcesses.inspect(pid)?.map(|mut p| {
                p.elapsed_secs = 10_000;
                p
            }))
        }
        fn terminate(&self, p: &corral_core::StuckProcess) -> anyhow::Result<()> {
            SystemProcesses.terminate(p)
        }
    }
    let fixture = tempfile::tempdir().unwrap();
    let script = fixture.path().join("worker.py");
    let fifo = fixture.path().join("ready");
    let pidfile = fixture.path().join("pid");
    std::fs::write(&script, "import os,signal,sys\np=os.fork()\nif p == 0:\n os.execv('/usr/bin/python3', ['rust-analyzer-proc-macro-srv','-c', \"import os,signal; print(os.getpid(),flush=True); signal.pause()\"])\nsignal.pause()\n").unwrap();
    let worker = fixture.path().join("fake");
    std::os::unix::fs::symlink("/usr/bin/python3", &worker).unwrap();
    let command = format!(
        "mkfifo '{0}'; '{3}' '{1}' > '{0}' & read -r pid < '{0}'; printf '%s' \"$pid\" > '{2}'; rm '{0}'",
        fifo.display(),
        script.display(),
        pidfile.display(),
        worker.display()
    );
    let fx = Fx::with_agent(Some(&command));
    let pid: u32 = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
    struct Guard(corral_core::StuckProcess);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = SystemProcesses.terminate(&self.0);
        }
    }
    let process = SystemProcesses.inspect(pid).unwrap().unwrap();
    let _guard = Guard(corral_core::StuckProcess {
        pid,
        argv: process.argv.clone(),
        started: process.started.clone(),
    });
    let parent = process.parent_pid;
    let parent_snapshot = SystemProcesses.inspect(parent).unwrap().unwrap();
    let _parent = Guard(corral_core::StuckProcess {
        pid: parent,
        argv: parent_snapshot.argv,
        started: parent_snapshot.started,
    });
    assert!(
        std::fs::read_link(format!("/proc/{pid}/cwd"))
            .unwrap()
            .starts_with(&fx.run().worktree)
    );
    fx.edit(|s| {
        s["process_info"] =
            json!({"shell_pid":10,"foreground_processes":[{"pid":pid,"argv":process.argv}]})
    });
    let start = jiff::Timestamp::now();
    corral_ops::sync_at_with_processes(&fx.ctx, start, &Aged).unwrap();
    corral_ops::sync_at_with_processes(
        &fx.ctx,
        start + jiff::SignedDuration::from_secs(10_000),
        &Aged,
    )
    .unwrap();
    assert!(fx.run().stuck.is_none());
    let board = corral_ops::attention::board(&fx.ctx, start).unwrap();
    assert!(
        !board
            .tasks
            .iter()
            .flat_map(|t| &t.assessment.conditions)
            .any(|c| c.kind == "stuck_process")
    );
    assert!(SystemProcesses.inspect(pid).unwrap().is_some());
    // Model the real pane close: terminate the worker shell/parent, not its LSP.
    // The surviving marker-owned LSP must then be reaped by worktree cleanup.
    let pane = fx.run().herdr.unwrap().pane_id;
    fx.edit(|s| s["shells"][pane] = json!(parent));
    fx.owner_ready();
    let result =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(result.contains("worktree removed"), "{result}");
    assert!(std::fs::read_link(format!("/proc/{pid}/cwd")).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn resolved_stuck_attention_restarts_automatic_review_after_done() {
    use corral_ops::{ProcessControl, SystemProcesses};
    struct Aged;
    impl ProcessControl for Aged {
        fn inspect(&self, pid: u32) -> anyhow::Result<Option<corral_ops::ProcessSnapshot>> {
            Ok(SystemProcesses.inspect(pid)?.map(|mut p| {
                p.elapsed_secs = 10_000;
                p
            }))
        }
        fn terminate(&self, p: &corral_core::StuckProcess) -> anyhow::Result<()> {
            SystemProcesses.terminate(p)
        }
    }
    let fx = Fx::new();
    let resident = Resident::start(&fx.run().worktree, None, None);
    let pid = resident.0.id();
    let process = SystemProcesses.inspect(pid).unwrap().unwrap();
    fx.edit(|s| {
        s["process_info"] =
            json!({"shell_pid":10,"foreground_processes":[{"pid":pid,"argv":process.argv}]})
    });
    let now = jiff::Timestamp::now();
    corral_ops::sync_at_with_processes(&fx.ctx, now, &Aged).unwrap();
    let later = now + jiff::SignedDuration::from_secs(10_000);
    corral_ops::sync_at_with_processes(&fx.ctx, later, &Aged).unwrap();
    let task = fx.ctx.store.get(&fx.id).unwrap();
    assert_eq!(task.current_run().unwrap().stuck.as_ref().unwrap().pid, pid);
    assert!(
        task.attention
            .records
            .values()
            .any(|r| r.kind == "stuck_process" && r.resolved_at.is_none())
    );
    fx.done();
    // The first review pass sees the still-open record, reports a real refusal,
    // then attention reconciliation closes it at the end of the pass.
    corral_ops::sync_at_with_processes(&fx.ctx, later, &Aged).unwrap();
    let events = fx.ctx.store.events().unwrap();
    assert!(events.iter().any(|e| {
        e.kind == "automation_blocked"
            && e.detail["reason"]
                .as_str()
                .unwrap_or("")
                .contains("process recovery")
    }));
    assert!(
        fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "attention_resolved" && e.detail["kind"] == "stuck_process")
    );
    corral_ops::sync_at_with_processes(&fx.ctx, later, &Aged).unwrap();
    assert!(
        fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "automation_resumed")
    );
    assert_eq!(fx.run().autopilot.unwrap().stage, AutoStage::Reviewing);
}

#[cfg(target_os = "linux")]
#[test]
fn gate_commands_inherit_run_ownership_marker() {
    let mut fx = Fx::new();
    let observed = fx.app.join("gate-marker");
    fx.ctx.store.config.review.gate = Some(vec![format!(
        "printf '%s' \"$CORRAL_PROCESS_OWNER\" > '{}'",
        observed.display()
    )]);
    fx.ctx.store.save_config().unwrap();
    fx.review();
    assert_eq!(std::fs::read_to_string(observed).unwrap(), fx.marker());
}

#[cfg(target_os = "linux")]
#[test]
fn verdict_reaps_reviewer_resident_before_landing() {
    let fx = Fx::new();
    fx.review();
    let run = fx.run();
    let reviewer = run.review_runs.last().unwrap();
    let marker_path = fx
        .ctx
        .store
        .scratch_path("tmp")
        .unwrap()
        .join("launch-exits")
        .join(format!("{}-{}-{}.owner", fx.id, run.id, reviewer.id));
    let marker = std::fs::read_to_string(marker_path).unwrap();
    let mut resident = Resident::start(&run.worktree, None, Some(&marker));
    let pid = rustix::process::Pid::from_raw(resident.0.id().try_into().unwrap()).unwrap();
    let pidfd = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).unwrap();
    // Store event time can lag the process-monotonic review timestamp (e.g. after a clock step).
    fx.ctx
        .store
        .fixture_pin_now("2020-01-01T00:00:00Z".parse().unwrap())
        .unwrap();
    corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Approve, "Checked.").unwrap();
    // Signal delivery precedes actual exit; wait for the kernel event before reaping.
    let mut fds = [rustix::event::PollFd::new(
        &pidfd,
        rustix::event::PollFlags::IN,
    )];
    let timeout = rustix::event::Timespec {
        tv_sec: 10,
        tv_nsec: 0,
    };
    assert_eq!(
        rustix::event::poll(&mut fds, Some(&timeout)).unwrap(),
        1,
        "reviewer survived verdict"
    );
    assert!(resident.0.try_wait().unwrap().is_some());
    let saved = fx.run();
    let review = saved.review.as_ref().unwrap();
    assert!(
        fx.ctx.store.events().unwrap().iter().any(|event| {
            event.kind == "verdict"
                && event.detail["review_at"].as_str() == Some(review.at.to_string().as_str())
        }),
        "verdict event must identify the exact review independently of its wall-clock timestamp"
    );
    fx.sync();
    let result =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(result.contains("worktree removed"), "{result}");
    assert!(!run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn landing_reaps_reviewer_orphan_left_after_verdict() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let reviewer = run.review_runs.last().unwrap();
    let path = fx
        .ctx
        .store
        .scratch_path("tmp")
        .unwrap()
        .join("launch-exits")
        .join(format!("{}-{}-{}.owner", fx.id, run.id, reviewer.id));
    let marker = std::fs::read_to_string(path).unwrap();
    let mut resident = Resident::start(&run.worktree, None, Some(&marker));
    let result =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(result.contains("worktree removed"), "{result}");
    assert!(resident.0.try_wait().unwrap().is_some());
    assert!(!run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn sync_waits_for_verdict_event_before_reaping_reviewer() {
    let fx = Fx::new();
    fx.review();
    let run = fx.run();
    let reviewer = run.review_runs.last().unwrap();
    let path = fx
        .ctx
        .store
        .scratch_path("tmp")
        .unwrap()
        .join("launch-exits")
        .join(format!("{}-{}-{}.owner", fx.id, run.id, reviewer.id));
    let marker = std::fs::read_to_string(path).unwrap();
    let mut resident = Resident::start(&run.worktree, None, Some(&marker));
    fx.ctx
        .store
        .update(&fx.id, |task| {
            let run = task.current_run_mut().unwrap();
            // Simulate a backward wall-clock step before the event is written.
            let now =
                jiff::Timestamp::from_second(jiff::Timestamp::now().as_second() + 3600).unwrap();
            let review = run.review.as_mut().unwrap();
            review.verdict = Some(Verdict::Approve);
            review.at = now;
            let child = run.review_runs.last_mut().unwrap();
            child.ended = Some(now);
            child.review = Some(review.clone());
            Ok(())
        })
        .unwrap();
    fx.sync();
    assert!(
        resident.0.try_wait().unwrap().is_none(),
        "sync killed the unfinished verdict command"
    );
    fx.ctx
        .store
        .log_event(
            Some(&fx.id),
            "verdict",
            json!({"run":run.id,"reviewer":"rev", "review_at": fx.run().review.unwrap().at.to_string()}),
        )
        .unwrap();
    fx.sync();
    assert!(resident.0.try_wait().unwrap().is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn sync_reaps_abandoned_reviewer() {
    let fx = Fx::new();
    fx.review();
    let run = fx.run();
    let reviewer = run.review_runs.last().unwrap();
    let path = fx
        .ctx
        .store
        .scratch_path("tmp")
        .unwrap()
        .join("launch-exits")
        .join(format!("{}-{}-{}.owner", fx.id, run.id, reviewer.id));
    let marker = std::fs::read_to_string(path).unwrap();
    let mut resident = Resident::start(&run.worktree, None, Some(&marker));
    corral_ops::review_run(&fx.ctx, &fx.id, Some("rev")).unwrap();
    fx.sync();
    assert!(resident.0.try_wait().unwrap().is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn foreign_resident_with_recorded_pane_id_is_protected() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let pane = run.herdr.as_ref().unwrap().pane_id.clone();
    let spawn = |name: &str| {
        let mut child = crate::support::command("python3", &run.worktree)
            .current_dir(&run.worktree)
            .env_remove("CORRAL_PROCESS_OWNER")
            .env("HERDR_PANE_ID", name)
            .args([
                "-c",
                "import signal; print('ready',flush=True); signal.pause()",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        Resident(child)
    };
    let mut matching = spawn(&pane);
    let mut foreign = spawn("foreign:p1");
    let line =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(
        line.contains("HERDR_PANE_ID=foreign:p1, recorded=false"),
        "{line}"
    );
    assert!(
        line.contains(&format!("HERDR_PANE_ID={pane}, recorded=true")),
        "{line}"
    );
    assert!(matching.0.try_wait().unwrap().is_none());
    assert!(foreign.0.try_wait().unwrap().is_none());
    assert!(run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
}

#[test]
fn foreign_harness_retains_worktree_and_branch_with_warning() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let mut harness = Resident::start(&run.worktree, Some("codex"), None);
    let line =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(line.contains("protected process"), "{line}");
    assert!(line.contains("warning"), "{line}");
    assert!(run.worktree.is_dir());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
    assert!(harness.0.try_wait().unwrap().is_none());
}

#[test]
fn unrelated_editor_is_protected() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let mut editor = Resident::start(&run.worktree, Some("editor"), None);
    let line =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(
        line.contains("protected process") && line.contains("warning"),
        "{line}"
    );
    assert!(editor.0.try_wait().unwrap().is_none());
    assert!(run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
}

#[test]
fn dirty_worktree_is_retained_without_signalling_owned_process() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    std::fs::write(run.worktree.join("untracked"), "changes").unwrap();
    let marker = fx.marker();
    let mut child = Resident::start(&run.worktree, None, Some(&marker));
    let error = corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false)
        .unwrap_err();
    assert!(format!("{error:#}").contains("uncommitted changes"));
    assert!(run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
    assert!(child.0.try_wait().unwrap().is_none());
}

#[test]
fn unlanded_branch_and_worktree_are_retained() {
    // Q26: approval now grants explicit retained-branch removal. Absence or change of
    // that actual committed authority must still protect even a genuinely owned resident.
    for missing in [true, false] {
        let mut fx = Fx::new();
        fx.owner_ready();
        let run = fx.run();
        let marker = fx.marker();
        let mut child = Resident::start(&run.worktree, None, Some(&marker));
        let plain = fx.ctx.store.clone();
        let mutations = plain.clone();
        let key = format!("{}/{}", fx.id, run.id);
        fx.ctx.store = plain.with_submission_barrier(std::sync::Arc::new(move |at| {
            if at != "approval_cleanup_committed" {
                return;
            }
            let area = corral_core::records::RecordArea::<serde_json::Value>::new(
                "worktree-cleanup-approvals",
            );
            let mut actual = area.load(&mutations, &key).unwrap().unwrap();
            if missing {
                mutations
                    .delete_record("worktree-cleanup-approvals", &key)
                    .unwrap();
            } else {
                actual["remove_worktree"] = serde_json::json!(false);
                area.save(&mutations, &key, &actual).unwrap();
            }
        }));
        let line =
            corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::KeepBranch, true, false).unwrap();
        let expected = if missing {
            "missing recorded"
        } else {
            "approval changed"
        };
        assert!(line.contains(expected), "{line}");
        assert!(run.worktree.exists());
        assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
        assert!(child.0.try_wait().unwrap().is_none());
    }
}

#[test]
fn recorded_retained_branch_approval_removes_only_its_owned_worktree_and_preserves_commit() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let approved = corral_git::rev_sha(&run.worktree, "HEAD").unwrap();
    let main = corral_git::rev_sha(&fx.app, "main").unwrap();
    let marker = fx.marker();
    let mut child = Resident::start(&run.worktree, None, Some(&marker));
    let line =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::KeepBranch, true, false).unwrap();
    assert!(
        line.contains("worktree removed; branch kept (branch not merged)"),
        "{line}"
    );
    assert!(!run.worktree.exists());
    assert_eq!(corral_git::rev_sha(&fx.app, &run.branch).unwrap(), approved);
    assert_eq!(corral_git::rev_sha(&fx.app, "main").unwrap(), main);
    assert!(child.0.try_wait().unwrap().is_some());
}

#[test]
fn another_tasks_custom_reviewer_harness_is_never_signalled() {
    let mut fx = Fx::new();
    fx.ctx.store.config.agents.insert(
        "custom-review".into(),
        AgentCmd {
            command: "custom-review-harness".into(),
        },
    );
    fx.owner_ready();
    let run = fx.run();
    let other = fx
        .ctx
        .store
        .create("Other task", Some("app"), Status::Running, "")
        .unwrap();
    fx.ctx
        .store
        .update(&other.id, |task| {
            let mut parent = run.clone();
            let mut reviewer = run.clone();
            reviewer.agent = "custom-review".into();
            reviewer.harness = None;
            parent.review_runs = vec![reviewer];
            task.runs.push(parent);
            Ok(())
        })
        .unwrap();
    let mut harness = Resident::start(&run.worktree, Some("custom-review-harness"), None);
    let line =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(line.contains("protected process"), "{line}");
    assert!(run.worktree.is_dir());
    assert!(harness.0.try_wait().unwrap().is_none());
}

#[test]
fn discard_protects_unowned_resident_and_retains_worktree_and_branch() {
    let fx = Fx::new();
    fx.done();
    let run = fx.run();
    assert!(!corral_git::is_ancestor(&fx.app, &run.branch, "main").unwrap());
    std::fs::write(run.worktree.join("untracked"), "discard me").unwrap();
    let mut child = Resident::start(&run.worktree, None, None);
    corral_ops::discard(&fx.ctx, &fx.id).unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    assert!(run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
    assert!(!fx.app.join("feature.txt").exists());
    assert!(run.worktree.join("untracked").exists());
}

#[test]
fn discard_keep_branch_removes_worktree_and_retains_unmerged_commit() {
    let fx = Fx::new();
    fx.done();
    let run = fx.run();
    let head = corral_git::rev_sha(&fx.app, &run.branch).unwrap();
    corral_ops::discard_with_options(&fx.ctx, &fx.id, true).unwrap();
    assert!(!run.worktree.exists());
    assert_eq!(corral_git::rev_sha(&fx.app, &run.branch).unwrap(), head);
}

#[test]
fn discard_retries_directory_left_after_git_unregistered_worktree() {
    let fx = Fx::new();
    let run = fx.run();
    corral_git::worktree_remove(&fx.app, &run.worktree, true).unwrap();
    std::fs::create_dir_all(&run.worktree).unwrap();
    std::fs::write(run.worktree.join("leftover"), "partial Git cleanup").unwrap();
    corral_ops::discard(&fx.ctx, &fx.id).unwrap();
    assert!(!run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_err());
}

#[test]
fn discard_reports_failed_pane_close_and_keeps_worktree_branch() {
    let fx = Fx::new();
    let run = fx.run();
    fx.edit(|s| {
        s["fail"] = json!([format!(
            "pane close {}",
            run.herdr.as_ref().unwrap().pane_id
        )])
    });
    let (sender, receiver) = std::sync::mpsc::channel();
    let _capture = corral_ops::capture_messages(sender);
    corral_ops::discard(&fx.ctx, &fx.id).unwrap();
    assert!(run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
    let events = fx.ctx.store.events().unwrap();
    let discarded = events.iter().find(|e| e.kind == "discarded").unwrap();
    assert_eq!(discarded.detail["worktree_removed"], false);
    assert!(!discarded.detail["warnings"].as_array().unwrap().is_empty());
    let messages: Vec<_> = receiver
        .try_iter()
        .filter(|message| message.source == "discard")
        .collect();
    assert_eq!(
        messages.len(),
        discarded.detail["warnings"].as_array().unwrap().len()
    );
    assert!(messages.iter().all(|message| {
        message.task.as_deref() == Some(fx.id.as_str()) && message.file.is_none()
    }));
}
