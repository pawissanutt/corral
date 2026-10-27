//! The hot-reads work budget (plan T-253 sections 4 and 5), run by `scripts/work-budget.sh`.
//!
//! `pm next`, `automation status` and a board frame are measured inside a `HotScope` on the
//! base store, the grown store (10x terminal tasks and 10x history on every active task) and
//! the nested store (10x nested history inside every current container), warm in this process
//! and cold in a fresh one. Between the base and each larger variant every unattributed cell is
//! unchanged, the unattributed hydration cell is 0, every inclusive total is the sum of its
//! cells, and each FU-3 follow-up cell equals the work its walk is expected to do on that
//! variant's own rows. Preconditions are checked before any counter is read.

use std::collections::BTreeMap;

use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_ops::Ctx;

use super::hot_budget_fixture::{BASE, Fixture, GROWN, NESTED, Shape, fixture, shape};

type Cells = BTreeMap<String, u64>;

const PATHS: [&str; 3] = ["pm_next", "automation_status", "board_frame"];
const SHAPE_VAR: &str = "CORRAL_HOT_BUDGET_SHAPE";

fn snapshot_ctx(ctx: &Ctx) -> Ctx {
    Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    }
}

fn run_path(ctx: &Ctx, path: &str) {
    match path {
        "pm_next" => {
            let ctx = snapshot_ctx(ctx);
            corral_ops::pm_workset(&ctx).unwrap();
            corral_ops::automation_summary(&ctx).unwrap();
        }
        "automation_status" => {
            let ctx = snapshot_ctx(ctx);
            corral_ops::automation_summary(&ctx).unwrap();
        }
        "board_frame" => {
            crate::board::BoardSnapshot::load(ctx).unwrap();
        }
        other => panic!("unknown path {other}"),
    }
}

fn flatten(delta: &WorkSnapshot, herdr: u64, git: u64) -> Cells {
    let mut cells = Cells::new();
    for counter in Counter::ALL {
        let c = delta.cells(counter);
        let name = counter.name();
        cells.insert(format!("{name}.total"), c.total);
        cells.insert(format!("{name}.unattributed"), c.unattributed);
        cells.insert(format!("{name}.own_record"), c.own_record);
        cells.insert(format!("{name}.emitted"), c.emitted);
        for ((id, site), n) in &c.followup {
            cells.insert(format!("{name}.followup.{id}.{site}"), *n);
        }
    }
    for ((id, site), n) in &delta.followup_calls {
        cells.insert(format!("calls.{id}.{site}"), *n);
    }
    cells.insert("herdr_calls.unattributed".into(), herdr);
    cells.insert("git_processes.unattributed".into(), git);
    cells
}

/// The work of one isolated full read of every task, the expected cost of one FU-3 walk.
fn listing_cost(ctx: &Ctx) -> (u64, u64, u64) {
    let ids: Vec<String> = ctx
        .store
        .list()
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    let (mut rows, mut bytes) = (0, 0);
    for id in &ids {
        let _hot = HotScope::enter();
        let before = work::snapshot();
        ctx.store.get(id).unwrap();
        let delta = work::snapshot().since(&before);
        rows += Counter::DECODES
            .iter()
            .map(|c| delta.total(*c))
            .sum::<u64>();
        bytes += delta.decoded_doc_bytes();
    }
    (ids.len() as u64, rows, bytes)
}

/// One measured variant: each path's cells and the expected FU-3 walk cost.
struct Measured {
    paths: BTreeMap<String, Cells>,
    tasks: u64,
    walk_rows: u64,
    walk_bytes: u64,
    coding_runs: u64,
    failures: Vec<String>,
}

fn coding_runs(ctx: &Ctx) -> u64 {
    ctx.store
        .list()
        .unwrap()
        .iter()
        .map(|task| task.runs.len() as u64)
        .sum()
}

fn measure_shape(shape: Shape, rounds: usize) -> Measured {
    let fx: Fixture = fixture(shape);
    let (tasks, walk_rows, walk_bytes) = listing_cost(&fx.ctx);
    let coding_runs = coding_runs(&fx.ctx);
    let mut failures = Vec::new();
    let mut paths = BTreeMap::new();
    for path in PATHS {
        let mut last = None;
        for _ in 0..rounds {
            let samples = fx.probe.samples();
            let (herdr, git) = (
                corral_herdr::work::invocations(),
                corral_git::work::invocations(),
            );
            let _hot = HotScope::enter();
            let before = work::snapshot();
            run_path(&fx.ctx, path);
            let delta = work::snapshot().since(&before);
            drop(_hot);
            if let Err(error) = delta.check_identity() {
                failures.push(format!("{}/{path}: {error}", shape.name));
            }
            let sampled = fx.probe.samples() - samples;
            let calls = delta.followup_calls("FU-3", "sample");
            if sampled != calls {
                failures.push(format!(
                    "{}/{path}: precondition: probe sampled {sampled} times, FU-3 sample calls {calls}",
                    shape.name
                ));
            }
            last = Some(flatten(
                &delta,
                corral_herdr::work::invocations() - herdr,
                corral_git::work::invocations() - git,
            ));
        }
        paths.insert(path.to_owned(), last.unwrap());
    }
    failures.extend(crate::app::hot_budget_checks::preconditions(&fx));
    Measured {
        paths,
        tasks,
        walk_rows,
        walk_bytes,
        coding_runs,
        failures,
    }
}

/// Every FU-3 cell of one path on one variant against its expected work.
fn followup_failures(variant: &str, path: &str, cells: &Cells, m: &Measured) -> Vec<String> {
    let mut failures = Vec::new();
    let get = |key: &str| cells.get(key).copied().unwrap_or(0);
    for site in ["likely_files", "sample"] {
        let calls = get(&format!("calls.FU-3.{site}"));
        let cell = |counter: &str| get(&format!("{counter}.followup.FU-3.{site}"));
        let rows: u64 = Counter::DECODES.iter().map(|c| cell(c.name())).sum();
        let mut expect = |what: &str, got: u64, want: u64| {
            if got != want {
                failures.push(format!(
                    "{variant}/{path}: FU-3 {site} {what} = {got}, expected {want} ({calls} calls)"
                ));
            }
        };
        expect("hydrations", cell("full_task_hydrations"), calls * m.tasks);
        expect("rows", rows, calls * m.walk_rows);
        expect("doc bytes", cell("decoded_doc_bytes"), calls * m.walk_bytes);
        expect("processes", cell("processes"), 0);
        let stats = if site == "sample" {
            calls * m.coding_runs
        } else {
            0
        };
        expect("stats", cell("file_stats"), stats);
        let views = cell("views_opened");
        if views != 0 && views != calls {
            failures.push(format!(
                "{variant}/{path}: FU-3 {site} views = {views}, expected 0 or {calls}"
            ));
        }
    }
    failures
}

fn compare(base: &Measured, larger: &Measured, name: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for path in PATHS {
        let (b, l) = (&base.paths[path], &larger.paths[path]);
        failures.extend(followup_failures("base", path, b, base));
        failures.extend(followup_failures(name, path, l, larger));
        let keys: std::collections::BTreeSet<&String> = b.keys().chain(l.keys()).collect();
        for key in keys {
            let (x, y) = (
                b.get(key).copied().unwrap_or(0),
                l.get(key).copied().unwrap_or(0),
            );
            if key.ends_with(".unattributed") && x != y {
                failures.push(format!("{path}: {key} changes: {x} at base, {y} at {name}"));
            }
            if key.starts_with("calls.") && x != y {
                failures.push(format!("{path}: {key} changes: {x} at base, {y} at {name}"));
            }
        }
        for (cells, variant) in [(b, "base"), (l, name)] {
            let hydrations = cells
                .get("full_task_hydrations.unattributed")
                .copied()
                .unwrap_or(0);
            if hydrations != 0 {
                failures.push(format!(
                    "{variant}/{path}: unattributed hydration cell = {hydrations}"
                ));
            }
        }
    }
    failures
}

fn report(m: &Measured) -> String {
    let mut out = String::new();
    for (path, cells) in &m.paths {
        let line: Vec<String> = cells
            .iter()
            .filter(|(_, n)| **n != 0)
            .map(|(k, n)| format!("{k}={n}"))
            .collect();
        out.push_str(&format!("  {path}: {}\n", line.join(" ")));
    }
    out
}

fn encode(m: &Measured) -> String {
    serde_json::json!({
        "paths": m.paths, "tasks": m.tasks, "walk_rows": m.walk_rows,
        "walk_bytes": m.walk_bytes, "coding_runs": m.coding_runs, "failures": m.failures,
    })
    .to_string()
}

fn decode(line: &str) -> Measured {
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    Measured {
        paths: serde_json::from_value(v["paths"].clone()).unwrap(),
        tasks: v["tasks"].as_u64().unwrap(),
        walk_rows: v["walk_rows"].as_u64().unwrap(),
        walk_bytes: v["walk_bytes"].as_u64().unwrap(),
        coding_runs: v["coding_runs"].as_u64().unwrap(),
        failures: serde_json::from_value(v["failures"].clone()).unwrap(),
    }
}

/// One variant measured in a fresh test process: a cold cache and a cold store.
fn cold(shape: Shape) -> Measured {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "app::hot_budget::cold_variant",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SHAPE_VAR, shape.name)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find_map(|line| line.split_once("HOTBUDGET ").map(|(_, json)| json))
        .unwrap_or_else(|| panic!("cold {} run printed no result:\n{text}", shape.name));
    decode(line)
}

#[test]
#[ignore = "spawned by hot_reads_work_stays_independent_of_history for one cold variant"]
fn cold_variant() {
    let Ok(name) = std::env::var(SHAPE_VAR) else {
        return;
    };
    println!("HOTBUDGET {}", encode(&measure_shape(shape(&name), 1)));
}

#[test]
#[ignore = "run by scripts/work-budget.sh (the gate)"]
fn hot_reads_work_stays_independent_of_history() {
    let mut failures = Vec::new();
    let mut text = String::new();
    for (mode, measured) in [
        ("warm", [BASE, GROWN, NESTED].map(|s| measure_shape(s, 2))),
        ("cold", [BASE, GROWN, NESTED].map(cold)),
    ] {
        let [base, grown, nested] = &measured;
        for (m, name) in [(base, "base"), (grown, "grown"), (nested, "nested")] {
            failures.extend(m.failures.iter().map(|f| format!("{mode}: {f}")));
            text.push_str(&format!(
                "{mode} {name} ({} tasks):\n{}",
                m.tasks,
                report(m)
            ));
        }
        for (larger, name) in [(grown, "grown"), (nested, "nested")] {
            failures.extend(
                compare(base, larger, name)
                    .into_iter()
                    .map(|f| format!("{mode}: {f}")),
            );
        }
    }
    if std::env::var_os("WORK_BUDGET_REPORT").is_some() || !failures.is_empty() {
        println!("{text}");
    }
    assert!(
        failures.is_empty(),
        "hot-reads work budget exceeded:\n  {}",
        failures.join("\n  ")
    );
}
