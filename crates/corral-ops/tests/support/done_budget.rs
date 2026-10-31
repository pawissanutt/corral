//! Fresh-process approve budget; the warm variant lives in launch_resume.
use super::fixture;
use corral_core::work::{self, Counter, HotScope};
use std::collections::BTreeMap;

fn measure(ctx: &crate::Ctx, id: &str) -> (String, BTreeMap<String, u64>) {
    corral_core::clock::pin_thread(Some(fixture::now()));
    let _hot = HotScope::enter();
    let before = work::snapshot();
    let git = corral_git::work::invocations();
    let herdr = corral_herdr::work::invocations();
    let result = crate::done(ctx, id, "finished the change")
        .map(|t| t.status.label().to_owned())
        .unwrap_or_else(|e| format!("error: {e:#}"));
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    assert_eq!(
        delta.total(Counter::FullTaskHydrations),
        0,
        "{:?}",
        delta.sites
    );
    let mut cells = BTreeMap::new();
    for c in Counter::ALL {
        let cell = delta.cells(c);
        cells.insert(format!("{}.unattributed", c.name()), cell.unattributed);
    }
    cells.insert("own.rows".into(), delta.own_record_rows());
    cells.insert("own.bytes".into(), delta.own_record_bytes());
    cells.insert("calls.git".into(), corral_git::work::invocations() - git);
    cells.insert(
        "calls.herdr".into(),
        corral_herdr::work::invocations() - herdr,
    );
    (result, cells)
}

#[test]
#[ignore = "fresh-process child of done_budget_is_identical_warm_and_cold"]
fn done_budget_cold_child() {
    let Ok(root) = std::env::var("CORRAL_DONE_BUDGET_ROOT") else {
        return;
    };
    let id = std::env::var("CORRAL_DONE_BUDGET_ID").unwrap();
    let root = std::path::Path::new(&root);
    let ctx = crate::Ctx {
        store: corral_core::Store::open(root).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(root.join("engine/herdr"), Some("corral-test".into())),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    println!(
        "DONEBUDGET {}",
        serde_json::to_string(&measure(&ctx, &id)).unwrap()
    );
}

#[test]
fn done_budget_is_identical_warm_and_cold() {
    for cold in [false, true] {
        for stale in [false, true] {
            let mut samples = Vec::new();
            for n in [1, 10] {
                let (fx, id) = super::launch_resume::obligated(n, false);
                if stale {
                    super::launch_resume::stale(&fx, &id);
                }
                let task = fx.ctx.store.get(&id).unwrap();
                let run = task.current_run().unwrap();
                assert!(run.checkpoint_required);
                assert_eq!(run.obligations.waits.len(), n);
                assert_eq!(run.obligations.decisions.len(), n);
                assert_eq!(run.checkpoints.len(), 1);
                let reference =
                    crate::resume::require_checkpoint(&task, run).map_err(|e| format!("{e:#}"));
                assert_eq!(reference.is_err(), stale);
                let sample: (String, BTreeMap<String, u64>) = if cold {
                    let out = std::process::Command::new(std::env::current_exe().unwrap())
                        .args([
                            "--exact",
                            "submission_paths_tests::done_budget::done_budget_cold_child",
                            "--ignored",
                            "--nocapture",
                            "--test-threads=1",
                        ])
                        .env("CORRAL_DONE_BUDGET_ROOT", fx.temp.path())
                        .env("CORRAL_DONE_BUDGET_ID", &id)
                        .stdin(std::process::Stdio::null())
                        .output()
                        .unwrap();
                    assert!(
                        out.status.success(),
                        "{}\n{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                    let text = String::from_utf8(out.stdout).unwrap();
                    let line = text
                        .lines()
                        .find_map(|l| l.split_once("DONEBUDGET ").map(|(_, json)| json))
                        .unwrap();
                    serde_json::from_str(line).unwrap()
                } else {
                    measure(&fx.ctx, &id)
                };
                if let Err(error) = reference {
                    assert!(sample.0.contains(&error), "{}", sample.0);
                } else {
                    assert_eq!(sample.0, "Review");
                    let after = fx.ctx.store.get(&id).unwrap();
                    assert_eq!(after.status, corral_core::Status::Review);
                    assert_eq!(after.current_run().unwrap().obligations, run.obligations);
                    assert_eq!(after.current_run().unwrap().checkpoints, run.checkpoints);
                }
                println!("done cold={cold} stale={stale} x{n}: {:?}", sample.1);
                samples.push(sample);
            }
            let [(a_out, a), (b_out, b)] = samples.try_into().unwrap();
            assert_eq!(a_out, b_out);
            for (key, value) in &a {
                if !key.starts_with("own.") {
                    assert_eq!(Some(value), b.get(key), "cold={cold} stale={stale}: {key}");
                }
            }
            assert!(b["own.rows"] > a["own.rows"]);
            assert!(b["own.bytes"] > a["own.bytes"]);
        }
    }
}
