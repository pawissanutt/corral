//! Races W1-W6 of plan T-253 section 12: the store gate keeps maintenance and v2 openers apart,
//! `migrate.lock` serializes maintenance, and the change count is read under exclusion. Each
//! race is driven by a barrier inside the command, never by timing.

#[path = "migrate_support/mod.rs"]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use corral_core::Store;
use support::{FENCE, Legacy, act_at};

fn published() -> Legacy {
    let legacy = Legacy::new();
    legacy.migrate().apply().unwrap();
    legacy
}

#[test]
fn w1_rollback_is_refused_while_a_v2_process_holds_the_store() {
    // Given a published store a v2 board holds open.
    let legacy = published();
    let board = Store::open(&legacy.root).unwrap();
    // When rollback waits for the gate and gives up.
    let error = legacy.migrate().rollback(None).unwrap_err();
    // Then it names the holder's pid and nothing changed.
    let pid = std::process::id().to_string();
    assert!(
        format!("{error:#}").contains(&format!("pid {pid}")),
        "{error:#}"
    );
    assert_eq!(legacy.journal().unwrap()["state"], "published");
    assert_eq!(legacy.fence().as_deref(), Some("4\n"));
    assert!(legacy.exists("db"));
    drop(board);
}

#[test]
fn w2_rollback_reads_the_change_count_under_exclusion() {
    // Given a published store with one write that --status reported.
    let legacy = published();
    let store = Store::open(&legacy.root).unwrap();
    store
        .create("first", None, corral_core::Status::Backlog, "")
        .unwrap();
    drop(store);
    let reported = legacy.migrate().status().unwrap().changes.unwrap();
    // When another write lands before `--rollback --discard-v2-changes N`.
    let store = Store::open(&legacy.root).unwrap();
    store
        .create("second", None, corral_core::Status::Backlog, "")
        .unwrap();
    drop(store);
    let error = legacy.migrate().rollback(Some(reported)).unwrap_err();
    // Then rollback refuses naming the count it read, and nothing changed.
    let now = legacy.migrate().status().unwrap().changes.unwrap();
    assert!(now > reported);
    assert!(
        format!("{error:#}").contains(&format!("{now} v2 changes")),
        "{error:#}"
    );
    assert_eq!(legacy.journal().unwrap()["state"], "published");
}

#[test]
fn w3_a_v2_open_during_rollback_sees_maintenance_then_the_legacy_store() {
    // Given a published store and a rollback that tries a v2 open while it holds the gate.
    let legacy = published();
    let root = legacy.root.clone();
    let seen = Arc::new(Mutex::new(None));
    let saw = Arc::clone(&seen);
    let barrier = act_at("B9", move || {
        let error = Store::open(&root).unwrap_err();
        *saw.lock().unwrap() = Some(format!("{error:#}"));
        Ok(())
    });
    legacy.migrate().barrier(barrier).rollback(None).unwrap();
    // Then the open during the rollback was refused as maintenance in progress.
    let during = seen.lock().unwrap().clone().unwrap();
    assert!(during.contains("maintenance in progress"), "{during}");
    // And after R7 an open is refused naming corral migrate.
    let error = Store::open(&legacy.root).unwrap_err();
    assert!(
        format!("{error:#}").contains("run `corral migrate`"),
        "{error:#}"
    );
    assert_eq!(legacy.fence().as_deref(), Some(FENCE));
}

#[test]
fn w4_an_opener_racing_publication_opens_only_after_a7() {
    // Given an apply that tries a v2 open after taking the gate (A6) and after the rename.
    let legacy = Legacy::new();
    let root = legacy.root.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let saw = Arc::clone(&seen);
    let barrier: corral_core::migrate::Barrier = Arc::new(move |id: &str| {
        if id == "B5" || id == "B6" {
            let error = Store::open(&root).unwrap_err();
            saw.lock().unwrap().push(format!("{error:#}"));
        }
        Ok(())
    });
    legacy.migrate().barrier(barrier).apply().unwrap();
    // Then both opens inside publication were refused, and after A7 the store opens.
    let refusals = seen.lock().unwrap().clone();
    assert_eq!(refusals.len(), 2);
    for refusal in refusals {
        assert!(refusal.contains("maintenance in progress"), "{refusal}");
    }
    Store::open(&legacy.root).unwrap();
}

#[test]
fn w5_finalize_and_rollback_are_serialized_and_finalize_ends_rollback() {
    // Given a published store and a rollback that tries finalize while it runs.
    let legacy = published();
    let root = legacy.root.clone();
    let seen = Arc::new(Mutex::new(None));
    let saw = Arc::clone(&seen);
    let barrier = act_at("B8", move || {
        let finalize = corral_core::migrate::Migrate::new(&root)
            .wait(Duration::ZERO)
            .finalize();
        *saw.lock().unwrap() = Some(format!("{:#}", finalize.unwrap_err()));
        Ok(())
    });
    legacy.migrate().barrier(barrier).rollback(None).unwrap();
    let during = seen.lock().unwrap().clone().unwrap();
    assert!(during.contains("migrate.lock"), "{during}");
    // And a finalize that tries rollback while it runs, after which rollback is refused.
    legacy.migrate().apply().unwrap();
    let root = legacy.root.clone();
    let seen = Arc::new(Mutex::new(None));
    let saw = Arc::clone(&seen);
    let barrier = act_at("B18", move || {
        let rollback = corral_core::migrate::Migrate::new(&root)
            .wait(Duration::ZERO)
            .rollback(None);
        *saw.lock().unwrap() = Some(format!("{:#}", rollback.unwrap_err()));
        Ok(())
    });
    legacy.migrate().barrier(barrier).finalize().unwrap();
    let during = seen.lock().unwrap().clone().unwrap();
    assert!(during.contains("migrate.lock"), "{during}");
    let error = legacy.migrate().rollback(None).unwrap_err();
    assert!(format!("{error:#}").contains("finalize"), "{error:#}");
}

#[test]
fn w6_rollback_waits_for_storage_gc_then_reports_reclaimed_build_output() {
    // Given a published store that storage gc holds open, having reclaimed build output.
    let legacy = published();
    std::fs::remove_dir_all(legacy.corral.join("state/reviews/T-001-R1-probe/target")).unwrap();
    let gc = Arc::new(Mutex::new(Some(Store::open(&legacy.root).unwrap())));
    let holder = Arc::clone(&gc);
    let n = legacy.migrate().status().unwrap().changes.unwrap();
    // When rollback finds the gate busy and gc exits.
    let barrier = act_at("gate_busy", move || {
        holder.lock().unwrap().take();
        Ok(())
    });
    let done = legacy
        .migrate()
        .wait(Duration::from_secs(30))
        .barrier(barrier)
        .rollback(Some(n).filter(|n| *n > 0))
        .unwrap();
    // Then the rollback proceeded and reported the reclaimed build output.
    assert!(gc.lock().unwrap().is_none());
    assert_eq!(done.reclaimed_build_output.len(), 1);
    assert_eq!(legacy.fence().as_deref(), Some(FENCE));
}
