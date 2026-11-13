use super::*;
use corral_core::{Origin, Store};

fn ctx() -> (tempfile::TempDir, Ctx) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    (dir, ctx)
}

/// One approved tracker with one member; returns `(tracker, member)`.
fn tracker(ctx: &Ctx, n: usize) -> (String, String) {
    let source = ctx.store.root().join(format!("plan-{n}.md"));
    std::fs::write(
        &source,
        format!(
            "---\ntitle: Plan {n}\napproved_scope:\n  goal: Ship {n}\n  acceptance:\n    - {{criterion: C1, scenario: check, expected: pass}}\ntasks:\n  - {{key: member, title: Member {n}}}\n---\n"
        ),
    )
    .unwrap();
    let imported = crate::import_plan(ctx, &source, false, Origin::Agent).unwrap();
    (imported.tracker.id, imported.tasks[0].id.clone())
}

fn at(base: Timestamp, seconds: i64) -> Timestamp {
    base.checked_add(SignedDuration::from_secs(seconds))
        .unwrap()
}

fn inputs(now: Timestamp) -> ClassicInputs {
    ClassicInputs {
        now,
        attention: None,
        force: false,
    }
}

/// Trackers whose cached progress was replaced by the refresh after one member changed.
fn recomputed_after_member_change(trackers: usize) -> usize {
    let (_dir, ctx) = ctx();
    let plans: Vec<_> = (0..trackers).map(|n| tracker(&ctx, n)).collect();
    let base = Timestamp::now();
    let mut cache = ClassicCache::new();
    cache.refresh(&ctx, inputs(base)).unwrap();
    // Settles anything the first refresh's own reads wrote.
    cache.refresh(&ctx, inputs(at(base, 1))).unwrap();
    let before: BTreeMap<String, *const plan::Tracker> = cache
        .trackers
        .iter()
        .map(|(id, tracker)| (id.clone(), Arc::as_ptr(tracker)))
        .collect();
    assert_eq!(before.len(), trackers);
    ctx.store
        .update(&plans[0].1, |task| {
            task.title = "Member renamed".into();
            Ok(())
        })
        .unwrap();
    cache.refresh(&ctx, inputs(at(base, 2))).unwrap();
    let frame = cache.current().unwrap();
    let progress = frame.plan_progress[&plans[0].0].as_ref().unwrap();
    assert_eq!(progress.work[0].title, "Member renamed");
    cache
        .trackers
        .iter()
        .filter(|(id, tracker)| before.get(*id) != Some(&Arc::as_ptr(tracker)))
        .count()
}

#[test]
fn one_member_change_recomputes_one_tracker() {
    // Given 2, and separately 4, trackers whose progress the cache holds.
    // When one member of the first tracker changes.
    // Then only that tracker's progress is recomputed, independently of the tracker count.
    assert_eq!(recomputed_after_member_change(2), 1);
    assert_eq!(recomputed_after_member_change(4), 1);
}

#[test]
fn quiet_refresh_publishes_nothing() {
    // Given a computed frame, refreshed once more to settle its own reads.
    let (_dir, ctx) = ctx();
    tracker(&ctx, 0);
    let now = Timestamp::now();
    let mut cache = ClassicCache::new();
    assert_eq!(cache.refresh(&ctx, inputs(now)).unwrap().unwrap().rev, 1);
    cache.refresh(&ctx, inputs(now)).unwrap();
    let rev = cache.current().unwrap().rev;
    // When nothing changed and no part is due.
    let quiet = cache.refresh(&ctx, inputs(now)).unwrap();
    // Then nothing is published and the revision stays.
    assert!(quiet.is_none());
    assert_eq!(cache.current().unwrap().rev, rev);
}

/// The frame without the parts allowed to follow the clock.
fn clock_free(frame: &ClassicFrame) -> serde_json::Value {
    let mut value = serde_json::to_value(frame).unwrap();
    let object = value.as_object_mut().unwrap();
    for field in [
        "rev",
        "seq",
        "as_of",
        "attention",
        "operating",
        "resources",
        "waits",
    ] {
        object.remove(field);
    }
    value
}

#[test]
fn frame_parts_depend_on_clock_only_where_declared() {
    // Given a tracker, a pinned Ready task and a Done task.
    let (_dir, ctx) = ctx();
    tracker(&ctx, 0);
    let ready = ctx
        .store
        .create("Ready", None, corral_core::Status::Ready, "")
        .unwrap();
    ctx.store.pin(&ready.id, None, false).unwrap();
    ctx.store
        .create("Finished", None, corral_core::Status::Done, "")
        .unwrap();
    let now = Timestamp::now();
    let mut cache = ClassicCache::new();
    cache.refresh(&ctx, inputs(now)).unwrap();
    // Settles anything the first refresh's own reads wrote.
    cache.refresh(&ctx, inputs(at(now, 1))).unwrap();
    let early = cache.current().unwrap();
    // When an hour passes without a store change (a full recompute of every part).
    cache.refresh(&ctx, inputs(at(now, 3600))).unwrap();
    let late = cache.current().unwrap();
    // Then only attention and resources may differ.
    assert!(!early.heads.is_empty());
    assert!(!early.pins.is_empty());
    assert!(!early.plan_progress.is_empty());
    assert_eq!(clock_free(&early), clock_free(&late));
}
