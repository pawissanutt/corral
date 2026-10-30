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
            .state_dir()
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
    let output = parent.current_dir(&run.worktree).env("CORRAL_PROCESS_OWNER", marker)
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
fn verdict_reaps_reviewer_resident_before_landing() {
    let fx = Fx::new();
    fx.review();
    let run = fx.run();
    let reviewer = run.review_runs.last().unwrap();
    let marker_path = fx
        .ctx
        .store
        .state_dir()
        .join("launch-exits")
        .join(format!("{}-{}-{}.owner", fx.id, run.id, reviewer.id));
    let marker = std::fs::read_to_string(marker_path).unwrap();
    let mut resident = Resident::start(&run.worktree, None, Some(&marker));
    corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Approve, "Checked.").unwrap();
    assert!(
        resident.0.try_wait().unwrap().is_some(),
        "reviewer survived verdict"
    );
    fx.sync();
    let result =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(result.contains("worktree removed"), "{result}");
    assert!(!run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_err());
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
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let marker = fx.marker();
    let mut child = Resident::start(&run.worktree, None, Some(&marker));
    let line =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::KeepBranch, true, false).unwrap();
    assert!(line.contains("unlanded branch"), "{line}");
    assert!(run.worktree.exists());
    assert!(corral_git::rev_sha(&fx.app, &run.branch).is_ok());
    assert!(child.0.try_wait().unwrap().is_none());
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
