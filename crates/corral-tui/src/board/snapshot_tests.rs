use super::tests::{key, seeded};
use super::{BoardSnapshot, Status};
use crate::View;

#[test]
fn slow_resource_sample_does_not_expire_frame_artifact_reads() {
    use corral_core::{
        Store,
        backend::{BackendConfig, ManualClock, fake::FakeBackend},
    };
    use corral_ops::{
        Ctx,
        resources::{FixedResourceProbe, ResourceProbe},
    };
    use std::sync::Arc;
    use std::time::Duration;

    struct SlowProbe(Arc<ManualClock>);
    impl std::fmt::Debug for SlowProbe {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("SlowProbe")
        }
    }
    impl ResourceProbe for SlowProbe {
        fn sample(&self, store: &Store) -> anyhow::Result<corral_core::ResourceSnapshot> {
            self.0.advance(Duration::from_secs(31));
            FixedResourceProbe::healthy().sample(store)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(jiff::Timestamp::UNIX_EPOCH));
    let store = Store::init(dir.path())
        .unwrap()
        .with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            clock.clone(),
            Arc::default(),
        )));
    let task = store.create("Review", None, Status::Review, "").unwrap();
    store
        .update(&task.id, |task| {
            task.artifacts
                .push(serde_json::from_value(serde_json::json!({
                    "run":"R1", "kind":"report", "title":"Review me", "path":"artifacts/report.md",
                    "source":"/tmp/report.md", "sha256":"0".repeat(64), "bytes":1,
                    "added":jiff::Timestamp::UNIX_EPOCH
                }))?);
            Ok(())
        })
        .unwrap();
    let ctx = Ctx {
        resource_probe: Arc::new(SlowProbe(clock)),
        ..Ctx::from_store(store)
    };
    let board = super::BoardView::new(&ctx).unwrap();
    assert_eq!(board.unreviewed.get(&task.id), Some(&1));
}

#[test]
fn snapshot_updates_board_when_store_is_unavailable() {
    // Given a selected card that moved and a newly pinned Done card in the worker snapshot.
    let mut fx = seeded();
    let mut board = fx.board();
    board.on_key(key('j'), &fx.ctx);
    assert_eq!(board.selected_task().unwrap().id, "T-002");
    fx.ctx
        .store
        .update("T-002", |task| {
            task.status = Status::Review;
            task.title = "Moved by the worker".into();
            Ok(())
        })
        .unwrap();
    fx.ctx.store.pin("T-007", None, false).unwrap();
    fx.ctx.store.config.ui.columns = vec![
        Status::Backlog,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
    ];
    fx.ctx.store.config.ui.done_max = 0;
    let snapshot = BoardSnapshot::load(&fx.ctx).unwrap();
    let resource_line = snapshot.resources.line();
    std::fs::remove_dir_all(fx.ctx.store.dir()).unwrap();

    // When the already-built snapshot is applied with no readable store.
    board.apply_snapshot(snapshot);

    // Then the moved card stays selected in its new column and store-derived decorations update.
    let selected = board.selected_task().unwrap();
    assert_eq!(
        (&*selected.id, selected.status, &*selected.title),
        ("T-002", Status::Review, "Moved by the worker")
    );
    assert_eq!(
        board.columns(),
        vec![
            Status::Backlog,
            Status::NeedsMe,
            Status::Review,
            Status::Done
        ]
    );
    assert_eq!(
        board
            .done_cards()
            .0
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["T-007"]
    );
    assert!(board.pinned.contains("T-007"));
    assert_eq!(board.pins.len(), 1);
    assert_eq!(board.unreviewed.get("T-006"), Some(&1));
    assert_eq!(board.resources.line(), resource_line);
}

#[test]
fn snapshot_preserves_done_toggle_when_columns_are_unchanged() {
    // Given a board whose owner temporarily hid Done.
    let fx = seeded();
    let mut board = fx.board();
    board.on_key(key('D'), &fx.ctx);
    assert!(!board.show_done);
    let snapshot = BoardSnapshot::load(&fx.ctx).unwrap();

    // When an ordinary worker refresh keeps the configured columns.
    board.apply_snapshot(snapshot);

    // Then the owner's local Done toggle is retained.
    assert!(!board.columns().contains(&Status::Done));
}
