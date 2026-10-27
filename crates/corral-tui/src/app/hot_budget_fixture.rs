//! Stores for the hot-reads budget (plan T-253 section 4, "Budget fixture rules"): one fixed
//! set of active tasks, grown only in history. Added titles share no keyword with an open task,
//! added runs' worktrees never exist, and the probe pins machine readings over the real walk.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

use corral_core::{
    ApprovedScope, Event, OperatingSelection, PlanDeliveryTarget, ResourceSnapshot, Status, Store,
    Task,
};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use corral_ops::resources::{PinnedReadingsProbe, ResourceProbe};

use super::hot_budget_rows::{T0, grow};

/// One variant of the fixture: terminal tasks, history per active task and nested history
/// inside the current containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    pub(crate) name: &'static str,
    pub(crate) terminal: usize,
    pub(crate) history: usize,
    pub(crate) nested: usize,
}

pub(crate) const BASE: Shape = Shape {
    name: "base",
    terminal: 12,
    history: 2,
    nested: 1,
};
pub(crate) const GROWN: Shape = Shape {
    name: "grown",
    terminal: 120,
    history: 20,
    nested: 1,
};
pub(crate) const NESTED: Shape = Shape {
    name: "nested",
    terminal: 12,
    history: 2,
    nested: 10,
};

pub(crate) fn shape(name: &str) -> Shape {
    [BASE, GROWN, NESTED]
        .into_iter()
        .find(|shape| shape.name == name)
        .unwrap()
}

/// The active board: (status, title). No two titles share a word longer than three letters.
const ACTIVE: [(Status, &str); 8] = [
    (Status::Idea, "Quokka widget"),
    (Status::Backlog, "Basalt parser"),
    (Status::Backlog, "Cobalt ledger"),
    (Status::Ready, "Dune exporter"),
    (Status::Running, "Ember scheduler"),
    (Status::Running, "Fjord indexer"),
    (Status::NeedsMe, "Garnet mailer"),
    (Status::Review, "Heron uploader"),
];

pub(crate) struct Fixture {
    pub(crate) _root: tempfile::TempDir,
    pub(crate) ctx: Ctx,
    pub(crate) probe: Arc<RecordingProbe>,
    /// Tasks whose current run has a real checkout: equal at every size.
    pub(crate) checkouts: Vec<PathBuf>,
    pub(crate) running: String,
}

/// The pinned admitting probe, keeping every snapshot it returns so a measured pass's own
/// samples can be checked (plan T-253 section 4, P9 preconditions).
#[derive(Debug)]
pub(crate) struct RecordingProbe {
    inner: PinnedReadingsProbe,
    seen: Mutex<Vec<ResourceSnapshot>>,
}

impl RecordingProbe {
    /// The snapshots sampled since the last call.
    pub(crate) fn take(&self) -> Vec<ResourceSnapshot> {
        std::mem::take(&mut *self.seen.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl ResourceProbe for RecordingProbe {
    fn sample(&self, store: &Store) -> anyhow::Result<ResourceSnapshot> {
        let snapshot = self.inner.sample(store)?;
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(snapshot.clone());
        Ok(snapshot)
    }
}

fn body(title: &str) -> String {
    format!("## Goal\n{title} goal.\n\n## Acceptance\n- {title} acceptance holds.\n")
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// A real checkout on `main` with one commit, so review and liveness reads take their normal
/// branch instead of failing on a missing worktree.
fn checkout(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q"]);
    std::fs::write(path.join("README.md"), "fixture\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-q", "-m", "fixture"]);
}

fn settle(task: &mut Task, status: Status, shape: Shape, open: bool) {
    grow(task, shape.history, shape.nested, open);
    if !open {
        return;
    }
    let pane = task.runs.last_mut().unwrap();
    pane.herdr = Some(corral_core::HerdrRef {
        workspace_id: "w1".into(),
        pane_id: format!("w1:{}", task.id),
        socket: None,
    });
    if status == Status::NeedsMe {
        pane.agent_state = Some("blocked".into());
        pane.blocked_excerpt = Some("Which option should I take?".into());
    }
}

/// Events on the active tasks, `per_task` each, none of a kind any derivation keys on; at
/// least three, so the PM evidence tail shows fixture events at every size.
fn events(ids: &[String], per_task: usize) -> String {
    let mut log = String::new();
    for id in ids {
        for i in 0..per_task {
            let event = Event {
                seq: 0,
                ts: T0,
                task: Some(id.clone()),
                kind: ["commented", "artifact"][i % 2].into(),
                actor: None,
                via: None,
                detail: serde_json::json!({"run": "R0000", "note": format!("entry {i:04}")}),
            };
            log.push_str(&serde_json::to_string(&event).unwrap());
            log.push('\n');
        }
    }
    log
}

/// An approved Plan over two active members, so a frame computes tracker progress.
fn plan(store: &Store, members: &[String]) {
    let mut tracker = store
        .create(
            "Iris roadmap",
            Some("app"),
            Status::Backlog,
            &body("Iris roadmap"),
        )
        .unwrap();
    tracker.created = T0;
    tracker.updated = T0;
    tracker.tracker = true;
    tracker.children = members.to_vec();
    tracker.approved_scope = Some(ApprovedScope {
        goal: "Deliver the roadmap".into(),
        acceptance: vec![],
        delivery_target: PlanDeliveryTarget::Main,
        approved_design: None,
    });
    for id in members {
        let mut member = store.get(id).unwrap();
        member.parent = Some(tracker.id.clone());
        store.fixture_put_task(&member).unwrap();
    }
    store.fixture_put_task(&tracker).unwrap();
}

/// A confirmed grant over one Backlog task, so the PM workset reaches its scoped admission
/// read (the sample at `pm_workset.rs`) on its normal branch.
fn grant(store: &mut Store, task: &str) {
    let selection = OperatingSelection {
        tasks: [task.to_owned()].into(),
        ..Default::default()
    };
    let preview = store.preview_operating_policy(&selection).unwrap();
    store.config = store
        .confirm_operating_policy(
            &preview,
            "fixture owner",
            // Whole seconds: a fractional part would vary the stored width per run.
            jiff::Timestamp::from_second(jiff::Timestamp::now().as_second()).unwrap(),
        )
        .unwrap();
    store.save_config().unwrap();
}

pub(crate) fn fixture(shape: Shape) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::init(root.path()).unwrap();
    std::fs::create_dir_all(root.path().join("app").join(".git")).unwrap();
    store.config.projects.insert(
        "app".into(),
        serde_json::from_value::<corral_core::Project>(serde_json::json!({"path": "app"})).unwrap(),
    );
    store.config.dispatch.promote_ideas = corral_core::PromoteIdeas::Suggest;
    store.config.limits.running = 10;
    store.config.storage.cache_root = Some(root.path().join("cache"));
    store.config.pm.push_events = false;
    store.config.review.autopilot = Some(false);
    store.save_config().unwrap();
    let mut store = Store::open(root.path()).unwrap();
    store
        .config
        .validate(store.root())
        .expect("the fixture must keep the validated default thresholds");
    let bin = root.path().join("herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/work_budget_herdr.sh"),
        &bin,
    )
    .unwrap();
    let herdr = Herdr::with_bin(bin, Some("corral-test".into()));
    let mut ids = Vec::new();
    let mut checkouts = Vec::new();
    for (status, title) in ACTIVE {
        let mut task = store
            .create(title, Some("app"), status, &body(title))
            .unwrap();
        task.created = T0;
        task.updated = T0;
        let open = matches!(status, Status::Running | Status::NeedsMe);
        settle(&mut task, status, shape, open);
        if open || status == Status::Review {
            let path = root.path().join("checkouts").join(&task.id);
            checkout(&path);
            task.runs.last_mut().unwrap().worktree = path.clone();
            checkouts.push(path);
        }
        store.fixture_put_task(&task).unwrap();
        ids.push((task.id, status));
    }
    let member = |wanted: Status| ids.iter().find(|(_, s)| *s == wanted).unwrap().0.clone();
    plan(&store, &[member(Status::Ready), member(Status::Running)]);
    for n in 0..shape.terminal {
        let status = if n % 10 == 9 {
            Status::Dropped
        } else {
            Status::Done
        };
        let title = format!("Hx{n:04}");
        let mut task = store
            .create(&title, Some("app"), status, &body(&title))
            .unwrap();
        task.created = T0;
        task.updated = T0;
        settle(&mut task, status, BASE, false);
        store.fixture_put_task(&task).unwrap();
    }
    let active: Vec<String> = ids.iter().map(|(id, _)| id.clone()).collect();
    store
        .fixture_append_events(&events(&active, 3 * shape.history))
        .unwrap();
    grant(&mut store, &member(Status::Backlog));
    let probe = Arc::new(RecordingProbe {
        inner: PinnedReadingsProbe::admitting(),
        seen: Mutex::default(),
    });
    let ctx = Ctx {
        store,
        herdr,
        resource_probe: probe.clone(),
    };
    Fixture {
        _root: root,
        ctx,
        probe,
        checkouts,
        running: member(Status::Running),
    }
}
