//! Worktree-removal process checks (T-334): a resident harness is protected through the run's
//! `harnesses` fact and the recorded identities of every task holding an unreleased pane (open
//! or finished, configured or not), with the same decision and the same counters whatever the
//! run's review children or the board's finished history hold.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use corral_core::config::Project;
use corral_core::work::{Counter, snapshot};
use corral_core::{Run, Status, Store};
use serde_json::json;

use super::remove;
use crate::Ctx;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "corral test")
        .env("GIT_AUTHOR_EMAIL", "corral@test.invalid")
        .env("GIT_COMMITTER_NAME", "corral test")
        .env("GIT_COMMITTER_EMAIL", "corral@test.invalid")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Resident(Child);

impl Resident {
    fn start(dir: &Path, name: &str) -> Resident {
        Self::with_marker(dir, name, None)
    }

    fn with_marker(dir: &Path, name: &str, marker: Option<&str>) -> Resident {
        let mut child = Command::new("python3")
            .args([
                "-c",
                "import signal; print('ready', flush=True); signal.pause()",
            ])
            .arg0(name)
            .current_dir(dir)
            .env_remove("HERDR_PANE_ID")
            .env_remove("CORRAL_PROCESS_OWNER")
            .envs(marker.map(|marker| ("CORRAL_PROCESS_OWNER", marker)))
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
    }

    fn alive(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if self.alive() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}

fn run(id: &str, worktree: &Path, harness: &str, herdr: bool) -> serde_json::Value {
    let mut run = json!({
        "id": id,
        "agent": harness,
        "harness": harness,
        "worktree": worktree,
        "branch": format!("corral/{id}"),
        "base": "main",
        "base_sha": "0".repeat(40),
        "started": corral_core::clock::now(),
    });
    if herdr {
        run["herdr"] = json!({"workspace_id": format!("w-{id}"), "pane_id": format!("p-{id}")});
    }
    run
}

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    repo: PathBuf,
    id: String,
    worktree: PathBuf,
}

/// A Review task run by `coder` whose run has `children` review children run by `reviewbot`, `terminal` Done tasks whose panes
/// were released, one Done peer whose pane is unreleased and whose harness
/// `retired-custom-harness` is absent from configuration and one
/// open task whose live pane runs `liveagent`.
fn fixture(children: usize, terminal: usize) -> Fx {
    // One timestamp width at every size, as the budget fixtures pin it.
    corral_core::clock::pin_thread(Some("2026-10-29T00:00:00Z".parse().unwrap()));
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let repo = root.join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let mut store = Store::init(&root).unwrap();
    store.config.projects.insert(
        "app".into(),
        Project {
            path: "app".into(),
            dispatch: Default::default(),
            default_profile: None,
            base: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: vec![],
        },
    );
    store.save_config().unwrap();
    let ctx = Ctx {
        resource_probe: crate::resources::FixedResourceProbe::arc(),
        store,
        herdr: corral_herdr::Herdr::with_bin(root.join("absent-herdr"), None),
    };
    let add = |status: Status, run: serde_json::Value| {
        let task = ctx.store.create("work", Some("app"), status, "").unwrap();
        let run: Run = serde_json::from_value(run).unwrap();
        ctx.store
            .update(&task.id, |t| {
                t.runs.push(run);
                Ok(())
            })
            .unwrap();
        task.id
    };
    // History: finished tasks whose panes were released (none recorded).
    for n in 0..terminal {
        add(
            Status::Done,
            run("R1", &root.join(format!("done-{n}")), "old-harness", false),
        );
    }
    // A finished peer whose pane is still unreleased, run by an unconfigured harness.
    add(
        Status::Done,
        run("R1", &root.join("finished"), "retired-custom-harness", true),
    );

    add(
        Status::Running,
        run("R1", &root.join("live"), "liveagent", true),
    );
    let worktree = repo.join(".worktrees").join("subject");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "corral/subject",
            &worktree.to_string_lossy(),
            "main",
        ],
    );
    let mut subject = run("R1", &worktree, "coder", true);
    subject["branch"] = json!("corral/subject");
    subject["review_runs"] = (0..children)
        .map(|n| {
            let mut child = run(
                &format!("R1-review{:03}", n + 1),
                &worktree,
                "reviewbot",
                false,
            );
            child["ended"] = json!(corral_core::clock::now());
            child
        })
        .collect();
    let id = add(Status::Review, subject);
    Fx {
        _tmp: tmp,
        ctx,
        repo,
        id,
        worktree,
    }
}

/// The removal's error and its counter totals, counted inside a hot scope as on Land and
/// approve paths reached from sync; nothing is removed while a harness is resident.
fn attempt(fx: &Fx) -> (String, BTreeMap<Counter, u64>) {
    let head = fx.ctx.store.head(&fx.id).unwrap();
    let run = head.current_run.as_deref().unwrap();
    let before = snapshot();
    let error = {
        let _hot = corral_core::work::HotScope::enter();
        remove(&fx.ctx, &head, &fx.repo, run, false).unwrap_err()
    };
    let after = snapshot();
    let counters = Counter::ALL
        .iter()
        .map(|&c| (c, after.total(c) - before.total(c)))
        .collect();
    (format!("{error:#}"), counters)
}

#[test]
fn a_review_child_harness_is_protected_through_the_harnesses_fact_at_any_size() {
    let mut results = Vec::new();
    for children in [1, 11] {
        let fx = fixture(children, 0);
        let mut reviewer = Resident::start(&fx.worktree, "reviewbot");
        let (error, counters) = attempt(&fx);
        assert!(
            error.contains(&format!("protected process {}", reviewer.0.id())),
            "{error}"
        );
        assert!(reviewer.alive() && fx.worktree.exists());
        results.push(counters);
    }
    assert_eq!(results[0], results[1], "identical counters at both sizes");
    assert!(
        results[0][&Counter::TaskHeadDecodes] > 0,
        "counted: {:?}",
        results[0]
    );
    assert_eq!(results[0][&Counter::FullTaskHydrations], 0);
}

#[test]
fn live_and_finished_pane_harnesses_are_protected_without_reading_finished_tasks() {
    let mut results = Vec::new();
    for terminal in [1, 10] {
        let fx = fixture(1, terminal);
        let mut live = Resident::start(&fx.worktree, "liveagent");
        let mut finished = Resident::start(&fx.worktree, "retired-custom-harness");
        let (error, counters) = attempt(&fx);
        for resident in [&live, &finished] {
            assert!(
                error.contains(&format!("protected process {}", resident.0.id())),
                "{error}"
            );
        }
        assert!(live.alive() && finished.alive() && fx.worktree.exists());
        results.push(counters);
    }
    assert_eq!(results[0], results[1], "identical counters at both sizes");
    assert!(
        results[0][&Counter::TaskHeadDecodes] > 0,
        "counted: {:?}",
        results[0]
    );
    assert_eq!(results[0][&Counter::FullTaskHydrations], 0);
}

#[test]
fn an_unprotected_resident_is_stopped_and_the_worktree_removed() {
    let fx = fixture(1, 1);
    let receipt = crate::launch_exit::receipt(&fx.ctx, &fx.id, "R1").unwrap();
    crate::launch_exit::arm(&fx.ctx, "true", &receipt).unwrap();
    let marker = std::fs::read_to_string(receipt.with_extension("owner")).unwrap();
    let mut stray = Resident::with_marker(&fx.worktree, "stray-tool", Some(&marker));
    let head = fx.ctx.store.head(&fx.id).unwrap();
    remove(
        &fx.ctx,
        &head,
        &fx.repo,
        head.current_run.as_deref().unwrap(),
        false,
    )
    .unwrap();
    assert!(!stray.alive());
    assert!(!fx.worktree.exists());
}

#[test]
fn an_unmarked_resident_remains_protected() {
    let fx = fixture(1, 1);
    let mut stray = Resident::start(&fx.worktree, "stray-tool");
    let (error, _) = attempt(&fx);
    assert!(
        error.contains(&format!("protected process {}", stray.0.id())),
        "{error}"
    );
    assert!(stray.alive() && fx.worktree.exists());
}

#[test]
fn an_earlier_runs_unreleased_pane_harness_is_protected() {
    let fx = fixture(1, 0);
    let peer = fx
        .ctx
        .store
        .create("peer", Some("app"), Status::Done, "")
        .unwrap();
    let older: Run = serde_json::from_value(run(
        "R1",
        &fx.repo.join("older"),
        "older-custom-harness",
        true,
    ))
    .unwrap();
    let newer: Run =
        serde_json::from_value(run("R2", &fx.repo.join("newer"), "newer-harness", false)).unwrap();
    fx.ctx
        .store
        .update(&peer.id, |t| {
            t.runs.push(older);
            t.runs.push(newer);
            Ok(())
        })
        .unwrap();
    let unreleased = fx
        .ctx
        .store
        .head(&peer.id)
        .unwrap()
        .unreleased_panes(false)
        .unwrap();
    assert_eq!(unreleased.len(), 1, "{unreleased:?}");
    let mut resident = Resident::start(&fx.worktree, "older-custom-harness");
    let (error, _) = attempt(&fx);
    assert!(
        error.contains(&format!("protected process {}", resident.0.id())),
        "{error}"
    );
    assert!(resident.alive() && fx.worktree.exists());
}
