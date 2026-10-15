use corral_core::Status;
use corral_herdr::Herdr;
use corral_ops::Ctx;

use crate::queue_support::Fixture;

struct ClosePanes<'a>(&'a Ctx);
impl Drop for ClosePanes<'_> {
    fn drop(&mut self) {
        for task in self.0.store.list().unwrap() {
            for run in task.runs {
                if let Some(h) = run.herdr
                    && self.0.herdr.pane_get(&h.pane_id).unwrap().is_some()
                {
                    self.0.herdr.pane_close(&h.pane_id).unwrap();
                }
            }
        }
    }
}

#[test]
fn isolated_corral_test_fills_and_refills_authorized_slots() {
    let herdr = Herdr::new(Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: herdr --session corral-test workspace list unavailable");
        return;
    }
    let mut f = Fixture::new();
    f.ctx.herdr = herdr;
    f.bin = "herdr".into();
    f.ctx.store.config.limits.running = 2;
    f.ctx.store.save_config().unwrap();
    let _cleanup = ClosePanes(&f.ctx);
    let first = f.task("app", Status::Ready);
    let dependent = f.task("app", Status::Ready);
    let other = f.task("other", Status::Ready);
    f.ctx
        .store
        .update(&dependent.id, |t| {
            t.depends_on.push(first.id.clone());
            Ok(())
        })
        .unwrap();
    for task in [&first, &dependent, &other] {
        f.ok(&["queue", "add", &task.id, "--profile", "generic"]);
    }

    let synced = f.ok(&["sync"]);
    assert_eq!(
        corral_ops::slot_usage(&f.ctx).unwrap().total(),
        2,
        "sync: {synced:?}; events: {:?}",
        f.ctx.store.events().unwrap()
    );
    assert!(f.ctx.store.get(&dependent.id).unwrap().runs.is_empty());
    f.ok(&[
        "done",
        &first.id,
        "--summary",
        "Verified: isolated generic fixture\nNot verified: none",
    ]);
    f.ok(&["sync"]);
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().total(), 1);
    assert!(f.ctx.store.get(&dependent.id).unwrap().runs.is_empty());
    f.ok(&["approve", &first.id, "--merge", "keep", "--keep-worktree"]);
    f.ok(&["sync"]);
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().total(), 2);
    let started = f.ctx.store.get(&dependent.id).unwrap();
    assert_eq!(started.status, Status::Running);
    let pane = &started
        .current_run()
        .unwrap()
        .herdr
        .as_ref()
        .unwrap()
        .pane_id;
    assert!(f.ctx.herdr.pane_get(pane).unwrap().is_some());
    let events = f.ctx.store.events().unwrap();
    for kind in ["dispatch_claimed", "dispatch_released", "dispatched"] {
        assert_eq!(
            events.iter().filter(|e| e.kind == kind).count(),
            3,
            "{kind}"
        );
    }
    println!(
        "LIVE corral-test: two generic slots filled; Review did not satisfy dependency; approval refilled; three claims/releases/dispatches; fixture panes cleaned."
    );
    if let Some(dir) = std::env::var_os("CORRAL_SLOT_EVIDENCE") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("live-queue-events.json"),
            serde_json::to_vec_pretty(&events).unwrap(),
        )
        .unwrap();
    }
}
