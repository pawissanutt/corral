//! Stores for the hot-reads budget (plan T-253 section 4, "Budget fixture rules"): one fixed
//! set of active tasks, grown only in history. Added titles share no keyword with an open task,
//! added runs' worktrees never exist, and the probe pins machine readings over the real walk.

use std::path::Path;
use std::sync::Arc;

use corral_core::{Event, Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use corral_ops::resources::PinnedReadingsProbe;

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
    pub(crate) probe: Arc<PinnedReadingsProbe>,
}

fn body(title: &str) -> String {
    format!("## Goal\n{title} goal.\n\n## Acceptance\n- {title} acceptance holds.\n")
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

pub(crate) fn fixture(shape: Shape) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::init(root.path()).unwrap();
    std::fs::create_dir_all(root.path().join("app")).unwrap();
    store.config.projects.insert(
        "app".into(),
        serde_json::from_value::<corral_core::Project>(serde_json::json!({"path": "app"})).unwrap(),
    );
    store.config.dispatch.paused = true;
    store.config.review.autopilot = Some(false);
    store.save_config().unwrap();
    let store = Store::open(root.path()).unwrap();
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
    for (status, title) in ACTIVE {
        let mut task = store
            .create(title, Some("app"), status, &body(title))
            .unwrap();
        task.created = T0;
        task.updated = T0;
        let open = matches!(status, Status::Running | Status::NeedsMe);
        settle(&mut task, status, shape, open);
        store.fixture_put_task(&task).unwrap();
        ids.push(task.id);
    }
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
    store
        .fixture_append_events(&events(&ids, 3 * shape.history))
        .unwrap();
    let probe = Arc::new(PinnedReadingsProbe::admitting());
    let ctx = Ctx {
        store,
        herdr,
        resource_probe: probe.clone(),
    };
    Fixture {
        _root: root,
        ctx,
        probe,
    }
}
