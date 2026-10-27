//! Cold-process coverage of the production consumers, not just the underlying walks.
use super::{fixture::assert_growth, likely, sample};
use corral_core::work::{Counter, WorkSnapshot};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};

const VARIANT: &str = "CORRAL_FU3_VARIANT";

fn compare_fresh(name: &str) {
    for variant in [false, true] {
        let snapshots = [1, 10].map(|scale| {
            // Given a fresh test process, which will build its own cold fixture.
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", name, "--ignored", "--nocapture"])
                .env(VARIANT, serde_json::to_string(&(scale, variant)).unwrap())
                .stdin(Stdio::null())
                .output()
                .unwrap();
            // When the selected real caller completes, all its per-variant checks ran.
            assert!(
                output.status.success(),
                "{name}, scale={scale}, variant={variant}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            let lines: Vec<_> = stdout
                .lines()
                .filter(|line| line.starts_with('{'))
                .collect();
            assert_eq!(lines.len(), 1, "one flattened work snapshot: {stdout}");
            let flat: BTreeMap<String, u64> = serde_json::from_str(lines[0]).unwrap();
            restore(&flat)
        });
        // Then hot work is unchanged and every inclusive delta is fully attributed.
        assert_growth(&snapshots[0], &snapshots[1]);
    }
}

fn emit(caller: fn(usize, bool) -> WorkSnapshot) {
    let (scale, variant): (usize, bool) =
        serde_json::from_str(&std::env::var(VARIANT).unwrap()).unwrap();
    let snapshot = caller(scale, variant);
    snapshot.check_identity().unwrap();
    assert_eq!(snapshot.unattributed_hydrations(), 0);
    let mut flat = BTreeMap::new();
    for counter in Counter::ALL {
        let cells = snapshot.cells(counter);
        let name = counter.name();
        for (bucket, count) in [
            ("total", cells.total),
            ("unattributed", snapshot.get(counter)),
            ("own_record", cells.own_record),
            ("emitted", cells.emitted),
        ] {
            flat.insert(format!("{name}/{bucket}"), count);
        }
        for ((id, site), count) in cells.followup {
            flat.insert(format!("{name}/followup/{id}/{site}"), count);
        }
    }
    for ((id, site), calls) in snapshot.followup_calls {
        flat.insert(format!("calls/{id}/{site}"), calls);
    }
    println!("\n{}", serde_json::to_string(&flat).unwrap());
}

fn restore(flat: &BTreeMap<String, u64>) -> WorkSnapshot {
    let mut snapshot = WorkSnapshot::default();
    for counter in Counter::ALL {
        let name = counter.name();
        let prefix = format!("{name}/followup/");
        snapshot.cells.insert(
            counter,
            corral_core::work::Cells {
                total: flat[&format!("{name}/total")],
                unattributed: flat[&format!("{name}/unattributed")],
                own_record: flat[&format!("{name}/own_record")],
                emitted: flat[&format!("{name}/emitted")],
                followup: flat
                    .iter()
                    .filter_map(|(key, count)| {
                        let (id, site) = key.strip_prefix(&prefix)?.split_once('/')?;
                        Some(((id.into(), site.into()), *count))
                    })
                    .collect(),
            },
        );
    }
    snapshot.followup_calls = flat
        .iter()
        .filter_map(|(key, count)| {
            let (id, site) = key.strip_prefix("calls/")?.split_once('/')?;
            Some(((id.into(), site.into()), *count))
        })
        .collect();
    snapshot.check_identity().unwrap();
    snapshot
}

#[test]
fn pm_shortlist_has_identical_hot_work_in_fresh_processes() {
    compare_fresh("fu3::fresh::pm_shortlist_child");
}

#[test]
fn worker_prompt_has_identical_hot_work_in_fresh_processes() {
    compare_fresh("fu3::fresh::worker_prompt_child");
}

#[test]
fn scoped_workset_has_identical_hot_work_in_fresh_processes() {
    compare_fresh("fu3::fresh::scoped_workset_child");
}

#[test]
fn sync_observation_has_identical_hot_work_in_fresh_processes() {
    compare_fresh("fu3::fresh::sync_observation_child");
}

#[test]
#[ignore = "re-executed with a scale and variant by the fresh-process parent"]
fn pm_shortlist_child() {
    emit(likely::shortlist_variant);
}

#[test]
#[ignore = "re-executed with a scale and variant by the fresh-process parent"]
fn worker_prompt_child() {
    emit(likely::worker_variant);
}

#[test]
#[ignore = "re-executed with a scale and variant by the fresh-process parent"]
fn scoped_workset_child() {
    emit(sample::workset_variant);
}

#[test]
#[ignore = "re-executed with a scale and variant by the fresh-process parent"]
fn sync_observation_child() {
    emit(|scale, existing| sample::resource_variant(scale, existing, false));
}
