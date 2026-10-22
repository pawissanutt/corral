//! Disposable-store public API probe, not a recovery service or owner ceremony.
#[path = "../recovery_support/mod.rs"]
mod recovery_support;

use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::{RecoveryState, Status, Store, parse_task_record, render_task_record};

fn main() -> Result<()> {
    let root = std::env::args()
        .nth(1)
        .context("missing disposable fixture root")?;
    ensure!(
        !Path::new(&root).join(".corral").exists(),
        "fixture must be fresh"
    );
    let store = Store::init(Path::new(&root))?;
    let task = store.create(
        "Recovery API fixture",
        None,
        Status::NeedsMe,
        "Original spec\n",
    )?;
    let other = store.create("Unrelated fixture", None, Status::Backlog, "")?;
    let mut recovery = recovery_support::recovery(&store);
    ensure!(
        store.save_recovery_record(&task.id, &recovery).is_err(),
        "upgrade must be explicit"
    );
    let backup = store.upgrade_schema()?;
    let config = std::fs::read(store.config_path()).context("reading fixture config")?;
    let unrelated = std::fs::read(&other.path).context("reading unrelated fixture")?;
    recovery_support::append_outcome(&mut recovery, RecoveryState::Applying);
    recovery_support::append_outcome(&mut recovery, RecoveryState::Unknown);
    store.save_recovery_record(&task.id, &recovery)?;
    store.save(&task)?;
    let reopened = Store::open_unobserved(Path::new(&root))?;
    let record = reopened.task_record(&task.id)?;
    ensure!(record.recoveries == [recovery], "recovery history changed");
    ensure!(
        parse_task_record(&render_task_record(&record)?, &task.path)? == record,
        "envelope did not round trip"
    );
    ensure!(
        std::fs::read(store.config_path())? == config,
        "config changed"
    );
    ensure!(
        std::fs::read(&other.path)? == unrelated,
        "unrelated task changed"
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "fixture_evidence_only": true,
            "schema": 3,
            "backup": backup,
            "configuration_unchanged": true,
            "unrelated_task_unchanged": true,
            "record": record
        }))?
    );
    Ok(())
}
