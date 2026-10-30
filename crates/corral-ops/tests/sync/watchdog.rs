//! Deterministic watchdog scenarios using fake herdr and injected process control.
use std::cell::RefCell;
use std::collections::BTreeMap;

use corral_core::StuckProcess;
use corral_ops::{ProcessControl, ProcessSnapshot};
use serde_json::json;

use super::Fx;

#[derive(Default)]
pub(super) struct Processes {
    live: RefCell<BTreeMap<u32, ProcessSnapshot>>,
    pub(super) killed: RefCell<Vec<u32>>,
}

impl ProcessControl for Processes {
    fn inspect(&self, pid: u32) -> anyhow::Result<Option<ProcessSnapshot>> {
        Ok(self.live.borrow().get(&pid).cloned())
    }

    fn terminate(&self, process: &StuckProcess) -> anyhow::Result<()> {
        let old = self.live.borrow_mut().remove(&process.pid).unwrap();
        assert_eq!(old.started, process.started);
        assert_eq!(old.argv, process.argv);
        self.killed.borrow_mut().push(process.pid);
        Ok(())
    }
}

pub(super) fn foreground(fx: &Fx, processes: &Processes, commands: &[(u32, &[&str])], age: u64) {
    // Watchdog delivery scenarios need an actual empty/pasted/submitted composer,
    // not the question-only screen used by the sync excerpt fixtures.
    std::fs::write(fx.bin_dir.join("terminal"), "").unwrap();
    let mut entries = Vec::new();
    processes.live.borrow_mut().clear();
    for (pid, argv) in commands {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        processes.live.borrow_mut().insert(
            *pid,
            ProcessSnapshot {
                parent_pid: 41,
                started: format!("start-{pid}"),
                elapsed_secs: age,
                argv: argv.clone(),
            },
        );
        entries.push(json!({"pid": pid, "argv": argv}));
    }
    std::fs::write(
        fx.bin_dir.join("processes.json"),
        json!({
            "result": {"process_info": {"shell_pid": 10, "foreground_processes": entries}}
        })
        .to_string(),
    )
    .unwrap();
}

fn sync(fx: &Fx, processes: &Processes, secs: i64) -> corral_core::Task {
    corral_ops::sync_at_with_processes(&fx.ctx, fx.at(secs), processes).unwrap();
    fx.ctx.store.get("T-001").unwrap()
}

#[test]
fn long_idle_foreign_foreground_is_flagged_and_notified_once() {
    // Given raw idle with a long-lived grep, even without ever seeing working.
    let fx = Fx::new("omo");
    let processes = Processes::default();
    foreground(&fx, &processes, &[(42, &["grep", "-cvE", "pattern"])], 700);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    // When the idle threshold is exceeded across multiple syncs.
    sync(&fx, &processes, 301);
    let task = sync(&fx, &processes, 600);
    // Then one durable warning and one notification identify the foreign process.
    let stuck = task.current_run().unwrap().stuck.as_ref().unwrap();
    assert_eq!(stuck.pid, 42);
    assert_eq!(stuck.argv, ["grep", "-cvE", "pattern"]);
    assert_eq!(task.status, corral_core::Status::Running);
    assert_eq!(fx.events("stuck"), 1);
    assert_eq!(fx.notifications(), "T-001 stuck\n");
}

#[test]
fn harness_and_known_helpers_are_never_stuck() {
    let fx = Fx::new("omo");
    let processes = Processes::default();
    foreground(
        &fx,
        &processes,
        &[
            (42, &["omo", "-n", "task"]),
            (
                43,
                &[
                    "bun.exe",
                    "/opt/node_modules/@code-yeongyu/senpi/dist/bundle/cli.js",
                ],
            ),
            (
                44,
                &[
                    "bun",
                    "/opt/omo-ai/plugin/runtime/ast-grep-mcp/cli.js",
                    "mcp",
                ],
            ),
            (
                45,
                &[
                    "bun",
                    "/opt/node_modules/@code-yeongyu/senpi-codemode/src/kernels/js/worker-core.js",
                ],
            ),
            (10, &["bash"]),
            (
                46,
                &[
                    "bun",
                    "/opt/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js",
                ],
            ),
            (
                47,
                &[
                    "python3",
                    "-u",
                    "/opt/node_modules/@code-yeongyu/senpi-codemode/src/kernels/py/worker.py",
                ],
            ),
            (48, &["custom-launcher"]),
        ],
        700,
    );
    processes.live.borrow_mut().get_mut(&48).unwrap().parent_pid = 10;
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    let task = sync(&fx, &processes, 301);
    assert!(task.current_run().unwrap().stuck.is_none());
    assert_eq!(fx.events("stuck"), 0);
    assert!(fx.notifications().is_empty());
}

/// A pi-native launch as herdr reports it when the pane shell (bash, 10) starts a nested
/// interactive shell (fish, 20) that runs corral's wrapper (42), the template shell (43), the
/// compiled host (44) and the agent the host drives (45), plus any `extra` foreground processes.
fn pi_native_pane(fx: &Fx, processes: &Processes, extra: &[(u32, u32, &[&str])]) {
    const SCRIPT: &str = "/bin/sh -c \"$1\" corral-launch; corral_launch_exit=$?; printf '%s\\n' \"$corral_launch_exit\" > '/s/T-001-R1.exited'";
    const LAUNCH: &str = "/opt/corral agent launch --bootstrap /s/bootstrap.json";
    let mut tree: Vec<(u32, u32, &[&str])> = vec![
        (42, 20, &["/bin/sh", "-c", SCRIPT, "corral-launch", LAUNCH]),
        (43, 42, &["/bin/sh", "-c", LAUNCH, "corral-launch"]),
        (
            44,
            43,
            &["/opt/corral-pi/corral-pi-host", "--launch-fd", "3"],
        ),
        (45, 44, &["claude", "-p"]),
    ];
    tree.extend_from_slice(extra);
    let commands: Vec<(u32, &[&str])> = tree.iter().map(|(pid, _, argv)| (*pid, *argv)).collect();
    foreground(fx, processes, &commands, 9_000);
    let mut live = processes.live.borrow_mut();
    for (pid, parent, _) in &tree {
        live.get_mut(pid).unwrap().parent_pid = *parent;
    }
    live.insert(
        20,
        ProcessSnapshot {
            parent_pid: 10,
            started: "start-20".into(),
            elapsed_secs: 9_000,
            argv: vec!["fish".into()],
        },
    );
}

#[test]
fn pi_native_launch_wrapper_under_a_nested_shell_is_never_stuck() {
    // Given a long-idle pi-native run whose foreground is corral's own launch tree.
    let fx = Fx::new("pi-native");
    let processes = Processes::default();
    pi_native_pane(&fx, &processes, &[]);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    // When the idle threshold is exceeded.
    let task = sync(&fx, &processes, 301);
    // Then no process is flagged and the wrapper cannot be terminated even if forged.
    assert!(task.current_run().unwrap().stuck.is_none());
    assert_eq!(fx.events("stuck"), 0);
    let live = processes.live.borrow().get(&42).cloned().unwrap();
    let wrapper = StuckProcess {
        pid: 42,
        argv: live.argv,
        started: live.started,
    };
    fx.ctx
        .store
        .update("T-001", |t| {
            t.current_run_mut().unwrap().stuck = Some(wrapper.clone());
            Ok(())
        })
        .unwrap();
    assert!(corral_ops::unstick_with(&fx.ctx, "T-001", &wrapper, &processes).is_err());
    assert!(processes.killed.borrow().is_empty());
}

#[test]
fn foreign_processes_in_a_pi_native_pane_are_still_stuck() {
    // Given a command the host spawned, and one typed into the nested shell.
    for (pid, parent) in [(46, 44), (47, 20)] {
        let fx = Fx::new("pi-native");
        let processes = Processes::default();
        pi_native_pane(&fx, &processes, &[(pid, parent, &["sleep", "9999"])]);
        fx.herdr_says("idle");
        sync(&fx, &processes, 0);
        // When the idle threshold is exceeded.
        let task = sync(&fx, &processes, 301);
        // Then exactly that foreign process is flagged.
        let stuck = task.current_run().unwrap().stuck.clone().unwrap();
        assert_eq!(
            (stuck.pid, stuck.argv),
            (pid, vec!["sleep".into(), "9999".into()])
        );
        assert_eq!(fx.events("stuck"), 1);
    }
}

#[test]
fn process_age_and_idle_threshold_both_must_be_exceeded() {
    let mut fx = Fx::new("fake");
    fx.ctx.store.config.sync.stuck_after_secs = 30;
    let processes = Processes::default();
    foreground(&fx, &processes, &[(42, &["grep"])], 30);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    assert!(
        sync(&fx, &processes, 30)
            .current_run()
            .unwrap()
            .stuck
            .is_none()
    );
    assert!(
        sync(&fx, &processes, 31)
            .current_run()
            .unwrap()
            .stuck
            .is_none()
    );
    foreground(&fx, &processes, &[(42, &["grep"])], 31);
    assert!(
        sync(&fx, &processes, 32)
            .current_run()
            .unwrap()
            .stuck
            .is_some()
    );
}

#[test]
fn unstick_refuses_a_harness_even_if_stuck_metadata_was_forged() {
    let fx = Fx::new("omo");
    let processes = Processes::default();
    foreground(&fx, &processes, &[(42, &["omo"])], 700);
    let expected = StuckProcess {
        pid: 42,
        argv: vec!["omo".into()],
        started: "start-42".into(),
    };
    fx.ctx
        .store
        .update("T-001", |t| {
            t.current_run_mut().unwrap().stuck = Some(expected.clone());
            Ok(())
        })
        .unwrap();
    assert!(corral_ops::unstick_with(&fx.ctx, "T-001", &expected, &processes).is_err());
    assert!(processes.killed.borrow().is_empty());
}

#[test]
fn fresh_omo_activity_does_not_hide_a_raw_idle_stuck_process() {
    let fx = Fx::new("omo");
    let processes = Processes::default();
    foreground(&fx, &processes, &[(42, &["node", "/tmp/wedged.js"])], 700);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    fx.omo_session_touched(300);
    let task = sync(&fx, &processes, 301);
    assert!(task.current_run().unwrap().stuck.is_some());
}

#[test]
fn resumed_activity_clears_stuck_but_keeps_undelivered_reply() {
    let fx = Fx::new("omo");
    let processes = Processes::default();
    foreground(&fx, &processes, &[(42, &["grep"])], 700);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    sync(&fx, &processes, 301);
    corral_ops::reply(&fx.ctx, "T-001", "go").unwrap();
    fx.herdr_says("working");
    let task = sync(&fx, &processes, 302);
    let run = task.current_run().unwrap();
    assert!(run.stuck.is_none() && run.herdr_idle_since.is_none());
    assert_eq!(run.pending_reply.as_deref(), Some("go"));
}

#[test]
fn unstick_kills_only_confirmed_foreign_pid_then_resends_latest_reply() {
    let fx = Fx::new("omo");
    fx.ctx
        .store
        .set_attribution(corral_core::Actor::Owner, "cli")
        .unwrap();
    let processes = Processes::default();
    foreground(&fx, &processes, &[(41, &["omo"]), (42, &["grep"])], 700);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    let task = sync(&fx, &processes, 301);
    corral_ops::reply(&fx.ctx, "T-001", "first").unwrap();
    corral_ops::reply(&fx.ctx, "T-001", "latest").unwrap();
    assert!(!fx.bin_dir.join("replies.log").exists());
    let expected = task.current_run().unwrap().stuck.as_ref().unwrap();
    let recovered = corral_ops::unstick_with(&fx.ctx, "T-001", expected, &processes).unwrap();
    assert_eq!(*processes.killed.borrow(), [42]);
    assert!(processes.live.borrow().contains_key(&41));
    assert!(recovered.current_run().unwrap().stuck.is_none());
    assert_eq!(
        std::fs::read_to_string(fx.bin_dir.join("replies.log")).unwrap(),
        "latest\n"
    );
    assert_eq!(fx.events("unstuck"), 1);
    let events = fx.ctx.store.events().unwrap();
    let unstuck = events.iter().find(|e| e.kind == "unstuck").unwrap();
    assert_eq!(unstuck.actor, Some(corral_core::Actor::System));
    assert_eq!(unstuck.via.as_deref(), Some("cli"));
}

#[test]
fn failed_reply_is_retained_and_resubmitted_after_recovery() {
    let fx = Fx::new("omo");
    let processes = Processes::default();
    foreground(&fx, &processes, &[(42, &["grep"])], 700);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    std::fs::write(fx.bin_dir.join("fail-reply"), "").unwrap();
    assert!(corral_ops::reply(&fx.ctx, "T-001", "retry me").is_err());
    let task = sync(&fx, &processes, 301);
    std::fs::remove_file(fx.bin_dir.join("fail-reply")).unwrap();
    corral_ops::unstick_with(
        &fx.ctx,
        "T-001",
        task.current_run().unwrap().stuck.as_ref().unwrap(),
        &processes,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(fx.bin_dir.join("replies.log")).unwrap(),
        "retry me\n"
    );
}

#[test]
fn acknowledged_reply_is_not_duplicated_after_recovery() {
    let fx = Fx::new("omo");
    let processes = Processes::default();
    foreground(&fx, &processes, &[(42, &["grep"])], 700);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    corral_ops::reply(&fx.ctx, "T-001", "swallowed").unwrap();
    let task = sync(&fx, &processes, 301);
    corral_ops::unstick_with(
        &fx.ctx,
        "T-001",
        task.current_run().unwrap().stuck.as_ref().unwrap(),
        &processes,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(fx.bin_dir.join("replies.log")).unwrap(),
        "swallowed\n"
    );
}

#[test]
fn unstick_rejects_reused_pid_or_changed_foreground_without_killing() {
    let fx = Fx::new("omo");
    let processes = Processes::default();
    foreground(&fx, &processes, &[(42, &["grep"])], 700);
    fx.herdr_says("idle");
    sync(&fx, &processes, 0);
    let task = sync(&fx, &processes, 301);
    let expected = task.current_run().unwrap().stuck.as_ref().unwrap();
    processes.live.borrow_mut().get_mut(&42).unwrap().started = "reused".into();
    assert!(corral_ops::unstick_with(&fx.ctx, "T-001", expected, &processes).is_err());
    foreground(&fx, &processes, &[(42, &["omo"])], 700);
    assert!(corral_ops::unstick_with(&fx.ctx, "T-001", expected, &processes).is_err());
    assert!(processes.killed.borrow().is_empty());
}
