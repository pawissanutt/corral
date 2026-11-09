use super::*;

#[test]
fn reserve_is_checked_against_normal_and_total_usage_separately() {
    // One ad-hoc job must not consume the normal allowance a second time.
    let pool = CapacityPool {
        normal: 2,
        adhoc: 1,
    };
    assert!(pool.permits(CapacityLane::Normal, 4, 1));
    let pool = CapacityPool {
        normal: 3,
        adhoc: 0,
    };
    assert!(!pool.permits(CapacityLane::Normal, 4, 1));
    assert!(pool.permits(CapacityLane::Adhoc, 4, 1));
    let pool = CapacityPool {
        normal: 3,
        adhoc: 1,
    };
    assert!(!pool.permits(CapacityLane::Adhoc, 4, 1));
    assert!(!pool.permits(CapacityLane::Normal, 4, 1));
}

#[test]
fn reserve_cannot_eliminate_normal_progress_or_allow_zero_gate_capacity() {
    let limits = CapacityLimits {
        unfinished: 5,
        agents: 8,
        gates: 2,
        adhoc_reserve: 5,
    };
    assert!(!limits.problems().is_empty());
    let limits = CapacityLimits {
        unfinished: 1,
        agents: 1,
        gates: 1,
        adhoc_reserve: 0,
    };
    assert!(limits.problems().is_empty());
    let limits = CapacityLimits { gates: 0, ..limits };
    assert!(!limits.problems().is_empty());
}

#[test]
fn validated_activation_publishes_a_compatible_fence_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = crate::Store::init(dir.path()).unwrap();
    assert!(store.require_capacity_schema().is_err());
    let tracker = store
        .create("legacy checkpoint", None, crate::Status::Ready, "")
        .unwrap();
    store
        .update(&tracker.id, |task| {
            task.tracker = true;
            Ok(())
        })
        .unwrap();
    store
        .save_record(
            "plan-checkpoints",
            &tracker.id,
            &serde_json::json!({
                "version": 1, "attempts": [{"id": "C1", "gate": {
                    "result": "running", "at": "2026-01-01T00:00:00Z", "sha": "head"
                }}]
            }),
        )
        .unwrap();
    store.config.limits.capacity = Some(CapacityLimits {
        unfinished: 6,
        agents: 4,
        gates: 1,
        adhoc_reserve: 1,
    });
    let original = std::fs::read_to_string(store.config_path()).unwrap();
    std::fs::write(store.config_path(), store.config.to_toml()).unwrap();
    assert!(
        crate::Store::open(dir.path()).is_err(),
        "raw config must not activate new scheduling on an old fence"
    );
    assert!(store.require_capacity_schema().is_err());
    std::fs::write(store.config_path(), original).unwrap();
    store.save_config().unwrap();
    store.require_capacity_schema().unwrap();
    assert_eq!(
        store
            .head(&tracker.id)
            .unwrap()
            .capacity_checkpoint
            .as_deref(),
        Some("C1")
    );
    assert_eq!(
        std::fs::read_to_string(store.dir().join("state/schema-version")).unwrap(),
        "5\n"
    );
    drop(store);
    let reopened = crate::Store::open(dir.path()).unwrap();
    assert_eq!(reopened.config.limits.capacity.as_ref().unwrap().agents, 4);
    reopened.check_runtime().unwrap();
    let fresh = tempfile::tempdir().unwrap();
    let initialized = crate::Store::init_with(fresh.path(), |store| {
        // Initial preferences are available before the first configuration observation.
        let mut config = store.config_layers()?.config()?;
        config.limits.capacity = reopened.config.limits.capacity.clone();
        std::fs::write(store.config_path(), config.to_toml())?;
        Ok(())
    })
    .unwrap();
    initialized.require_capacity_schema().unwrap();

    // A crash after the fence but before backfill/completion must not expose the model.
    reopened
        .update_head(&tracker.id, |head| {
            head.capacity_checkpoint = None;
            Ok(())
        })
        .unwrap();
    reopened
        .delete_record("capacity-model", "activation")
        .unwrap();
    assert!(
        crate::Store::open(dir.path()).is_err(),
        "an incomplete activation must fail closed"
    );
    {
        let _dispatch = reopened.lock("dispatch").unwrap();
        let _config = reopened.lock("config").unwrap();
        super::activation::activate(&reopened, false).unwrap();
    }
    assert!(
        crate::Store::open(dir.path()).is_err(),
        "backfill alone cannot publish activation before the config write"
    );
    reopened.edit_config(|_| Ok(())).unwrap();
    assert_eq!(
        reopened
            .head(&tracker.id)
            .unwrap()
            .capacity_checkpoint
            .as_deref(),
        Some("C1")
    );
    crate::Store::open(dir.path())
        .unwrap()
        .require_capacity_schema()
        .unwrap();
}

#[test]
fn legacy_limits_do_not_infer_new_pool_or_lane_configuration() {
    let limits: crate::Limits = toml::from_str(
        "running = 8\nrunning_target = 3\nwip = 5\nreview = 2\ncount_gates = false\ncount_reviews = true\n",
    ).unwrap();
    assert!(limits.capacity.is_none());
    let mut limits = limits;
    limits.capacity = Some(CapacityLimits {
        unfinished: 6,
        agents: 4,
        gates: 1,
        adhoc_reserve: 1,
    });
    let encoded = toml::to_string(&limits).unwrap();
    assert_eq!(toml::from_str::<crate::Limits>(&encoded).unwrap(), limits);
    let mut value = serde_json::to_value(&limits).unwrap();
    value["capacity"] = serde_json::json!(false);
    assert!(
        serde_json::from_value::<crate::Limits>(value.clone())
            .unwrap()
            .capacity
            .is_none()
    );
    value["capacity"] = serde_json::json!(true);
    assert!(serde_json::from_value::<crate::Limits>(value).is_err());
    let mut base = crate::Config::default_for(std::path::Path::new("/tmp"));
    base.limits.capacity = limits.capacity.clone();
    let mut draft = base.clone();
    draft.limits.capacity = None;
    let mut doc = crate::ConfigDoc::default();
    doc.apply_config_changes(&base, &draft).unwrap();
    assert_eq!(
        doc.get("limits.capacity")
            .unwrap()
            .unwrap()
            .as_value()
            .unwrap()
            .as_bool(),
        Some(false)
    );
    assert!(
        serde_json::from_str::<CapacityLimits>(
            r#"{"unfinished":6,"agents":4,"gates":1,"adhoc_reserve":1,"agent":99}"#,
        )
        .is_err()
    );
}
