use super::*;

struct Fixture {
    _temp: tempfile::TempDir,
    ctx: Ctx,
    task: corral_core::Task,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(temp.path()).unwrap();
        let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
        ctx.herdr = corral_herdr::Herdr::with_bin(
            temp.path().join("never-executed"),
            Some("corral-test".into()),
        );
        let task = ctx
            .store
            .create("Observed worker", None, corral_core::Status::Running, "")
            .unwrap();
        let run = serde_json::from_value(serde_json::json!({
            "id": "R1", "agent": "generic", "worktree": temp.path(),
            "branch": "main", "base": "main", "base_sha": "fixture",
            "started": "2026-01-01T00:00:00Z",
            "herdr": {
                "workspace_id": "w1", "pane_id": "w1:p1",
                "socket": ctx.herdr.socket_path().unwrap()
            }
        }))
        .unwrap();
        ctx.store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        let task = ctx.store.get(&task.id).unwrap();
        Self {
            _temp: temp,
            ctx,
            task,
        }
    }

    fn ticket(&self) -> Ticket {
        issue(&self.ctx).unwrap().unwrap()
    }

    fn observe(&self, ticket: Ticket, present: bool, seconds: i64) -> bool {
        let started = self.task.current_run().unwrap().started;
        gone(
            &self.ctx,
            Sample {
                task: &self.task,
                pane: Some("w1:p1"),
                started,
                seen: true,
                present,
                at: started + SignedDuration::from_secs(seconds),
                ticket: Some(ticket),
                count_miss: true,
            },
        )
        .unwrap()
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.ctx.store.state_dir().join("pane-liveness.json")).unwrap()
    }
}

#[test]
fn delayed_first_delivery_cannot_replace_a_newer_sighting_or_reuse_its_decision() {
    // Given A issued before B, but B's presence is consumed before A's first delivery.
    let fixture = Fixture::new();
    let a = fixture.ticket();
    let b = fixture.ticket();
    assert!(!fixture.observe(b, true, 2));
    let sighting = fixture.bytes();

    // When delayed A arrives, then it neither mutates B's state nor declares it gone.
    assert!(!fixture.observe(a, false, 1));
    assert_eq!(fixture.bytes(), sighting);
    let c = fixture.ticket();
    assert!(!fixture.observe(c, false, 3));
    let first_miss = fixture.bytes();
    assert!(!fixture.observe(c, false, 3));
    assert_eq!(fixture.bytes(), first_miss);

    // A second fresh miss is actionable, but stale A cannot borrow that newer decision.
    let d = fixture.ticket();
    assert!(fixture.observe(d, false, 4));
    let disappeared = fixture.bytes();
    assert!(fixture.observe(d, false, 4));
    assert_eq!(fixture.bytes(), disappeared);
    assert!(!fixture.observe(a, false, 1));
    assert_eq!(fixture.bytes(), disappeared);
}

#[test]
fn reinitialized_state_rejects_surviving_tickets_from_the_old_incarnation() {
    // Given a captured inventory whose persisted state is later rebuilt.
    let fixture = Fixture::new();
    let old = fixture.ticket();
    assert!(!fixture.observe(old, true, 1));
    std::fs::remove_file(fixture.ctx.store.state_dir().join("pane-liveness.json")).unwrap();
    let fresh = fixture.ticket();
    let rebuilt = fixture.bytes();

    // When an old ticket survives the rebuild, then it cannot populate the new state.
    assert!(!fixture.observe(old, false, 2));
    assert_eq!(fixture.bytes(), rebuilt);
    assert!(!fixture.observe(fresh, false, 3));
    let next = fixture.ticket();
    assert!(fixture.observe(next, false, 4));
}

#[test]
fn legacy_observations_keep_sightings_but_not_unordered_missing_evidence() {
    // Given old-format state with a sighting and one unorderable missing sample.
    let fixture = Fixture::new();
    let started = fixture.task.current_run().unwrap().started;
    let key = format!("w1:p1@{started}");
    let at = started + SignedDuration::from_secs(5);
    let legacy = BTreeMap::from([(
        key.clone(),
        serde_json::json!({"seen": true, "misses": 1, "at": at}),
    )]);
    std::fs::write(
        fixture.ctx.store.state_dir().join("pane-liveness.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();

    // When the first causal ticket migrates that state.
    let first = fixture.ticket();
    let state = match read(&fixture.ctx).unwrap().unwrap() {
        Stored::Current(state) => state,
        Stored::Legacy(_) => panic!("inventory issuance did not migrate legacy observations"),
    };

    // Then presence/time survive, but two fresh samples are required even on clock rollback.
    let observation = state.observations.get(&key).unwrap();
    assert!(observation.seen);
    assert_eq!(observation.at, at);
    assert_eq!(observation.misses, 0);
    assert!(!fixture.observe(first, false, 4));
    let second = fixture.ticket();
    assert!(fixture.observe(second, false, 3));
}

#[test]
fn retained_missing_projection_neither_advances_samples_nor_probes_panes() {
    // Given two causal missing samples after a sighting.
    let fixture = Fixture::new();
    assert!(!fixture.observe(fixture.ticket(), true, 1));
    assert!(!fixture.observe(fixture.ticket(), false, 2));
    assert!(fixture.observe(fixture.ticket(), false, 3));
    let before = fixture.bytes();
    let started = fixture.task.current_run().unwrap().started;
    // When the read-only projection loads confirmed absence.
    let missing = recorded_missing(&fixture.ctx).unwrap();
    // Then it identifies this launch only and leaves causal observation state unchanged.
    assert_eq!(
        missing,
        BTreeMap::from([(
            format!("w1:p1@{started}"),
            started + SignedDuration::from_secs(3)
        )])
    );
    assert_eq!(fixture.bytes(), before);
}
