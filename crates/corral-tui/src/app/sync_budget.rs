//! The hot-sync work budget (plan T-253 sections 4 and 5), run by `scripts/work-budget.sh`.
//!
//! One board sync step (`sync_step::run`: the sync pass, then the usage refresh) is measured
//! on the base, grown (10x terminal tasks and 10x history on every active task) and nested
//! (10x nested history inside every current container) stores, whose open tasks own live panes
//! on the fake engine. Warm runs measure a settled tick and then a tick that contains one real
//! current-run mutation (sync writing back a stale `agent_state`); cold runs measure each of
//! those steps in a fresh process on the settled store. Between the base and each larger variant every
//! unattributed cell is unchanged, the unattributed hydration cell is 0, every inclusive total
//! is the sum of its cells, FU-3 reads stay within v2 header-only bounds on that variant's rows,
//! and the usage refresh's own-record cells equal the refreshed runs' own rows exactly. The
//! mutation commits as one task change on the change feed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use corral_core::Status;
use corral_core::backend::HeadQuery;
use corral_core::work::{self, Counter, HotScope};

use super::hot_budget::{Cells, Measured, compare, decode, encode, flatten, report};
use super::hot_budget_checks::{expected, preconditions};
use super::hot_budget_fixture::{BASE, Fixture, GROWN, NESTED, Shape, fixture_with, reopen};

pub(super) const PATH: &str = "sync_tick";
const STEPS: [&str; 2] = ["steady", "mutation"];
const COLD_VAR: &str = "CORRAL_SYNC_BUDGET_ROOT";
const RUNNING_VAR: &str = "CORRAL_SYNC_BUDGET_RUNNING";
const STEP_VAR: &str = "CORRAL_SYNC_BUDGET_STEP";

/// Ticks that bring a fresh fixture to its steady state: first sightings, usage notes and
/// pane observations are written once.
const SETTLE: usize = 3;

/// `sync_step::run` on the fixture's pinned clock: crossings of wall-clock seconds must
/// not introduce different maintenance work in base and grown passes.
fn tick(fx: &Fixture) -> String {
    let now = fx.ctx.store.now();
    corral_ops::lesson_rows::prepare_evaluation(&fx.ctx).unwrap();
    let synced = corral_ops::sync_at(&fx.ctx, now);
    corral_ops::refresh_active_usage(&fx.ctx.store);
    format!("{synced:?}")
}

/// A stale `agent_state` on the Running task, which the next tick writes back from herdr.
fn stale_agent_state(fx: &Fixture) {
    fx.ctx
        .store
        .update_head(&fx.running, |head| {
            if let Some(run) = head.current_run_mut() {
                run.agent_state = Some("idle".into());
            }
            Ok(())
        })
        .unwrap();
}

/// The own-record work the usage refresh owes: each refreshed run's rows, read once.
fn expected_own(fx: &Fixture) -> (u64, u64) {
    let (mut rows, mut bytes) = (0, 0);
    for head in fx
        .ctx
        .store
        .heads(&HeadQuery::Open { project: None })
        .unwrap()
    {
        let refreshed = matches!(
            head.status,
            Status::Running | Status::NeedsMe | Status::Review
        );
        let Some(run) = head.current_run.as_ref().filter(|_| refreshed) else {
            continue;
        };
        let _hot = HotScope::enter();
        let before = work::snapshot();
        {
            let _own = work::own_records("budget", &head.id, &run.id);
            head.current_run_record().unwrap();
        }
        let delta = work::snapshot().since(&before);
        rows += delta.own_record_rows();
        bytes += delta.own_record_bytes();
    }
    (rows, bytes)
}

fn own_record(cells: &Cells) -> (u64, u64) {
    let rows = Counter::DECODES
        .iter()
        .map(|c| {
            cells
                .get(&format!("{}.own_record", c.name()))
                .copied()
                .unwrap_or(0)
        })
        .sum();
    let bytes = cells
        .get("decoded_doc_bytes.own_record")
        .copied()
        .unwrap_or(0);
    (rows, bytes)
}

fn task_changes(fx: &Fixture, after: corral_core::backend::ChangeSeq) -> Vec<String> {
    let page = fx.ctx.store.changes_since(after, 1000).unwrap();
    let mut tasks = Vec::new();
    for entry in page.entries {
        for scope in entry.scopes {
            if let corral_core::backend::ChangeScope::Task { num, .. } = scope {
                tasks.push(format!("T-{:03}", num.0));
            }
        }
    }
    tasks
}

fn measure_tick(fx: &Fixture, step: &str, m: &mut Measured) {
    let exp = expected(fx);
    let own = expected_own(fx);
    m.expected.insert(step.to_owned(), exp.clone());
    fx.probe.take();
    let seq = fx.ctx.store.view().unwrap().seq();
    let (herdr, git) = (
        corral_herdr::work::invocations(),
        corral_git::work::invocations(),
    );
    let before = work::snapshot();
    let output = tick(fx);
    let delta = work::snapshot().since(&before);
    let samples = fx.probe.take();
    let key = format!("{step}/{PATH}");
    let mut exp = exp;
    if step == "mutation" {
        // Walks before the tick's own write read the old bytes, walks after it the new.
        let after = expected(fx);
        if (after.tasks, after.walk_rows) != (exp.tasks, exp.walk_rows) {
            m.failures
                .push(format!("{key}: the tick's write changed the walked rows"));
        }
        exp.walk_bytes_after = Some(after.walk_bytes);
        m.expected.insert(step.to_owned(), exp.clone());
    }
    if let Err(error) = delta.check_identity() {
        m.failures.push(format!("{key}: {error}"));
    }
    m.failures.extend(preconditions(
        fx, &exp, &key, PATH, &delta, &samples, &output,
    ));
    for site in delta.sites.iter().filter(|site| site.followup.is_none()) {
        m.failures.push(format!(
            "{key}: counted hydration at {}:{}",
            site.location.file(),
            site.location.line()
        ));
    }
    let cells = flatten(
        &delta,
        corral_herdr::work::invocations() - herdr,
        corral_git::work::invocations() - git,
    );
    if own_record(&cells) != own {
        m.failures.push(format!(
            "{key}: own-record (rows, bytes) = {:?}, expected {own:?} (the refreshed runs' rows)",
            own_record(&cells)
        ));
    }
    let changed = task_changes(fx, seq);
    let want: Vec<String> = match step {
        "mutation" => vec![fx.running.clone()],
        _ => Vec::new(),
    };
    if changed != want {
        m.failures.push(format!(
            "{key}: task changes {changed:?}, expected {want:?}"
        ));
    }
    m.cells.insert(key.clone(), cells);
    m.outputs.insert(key, output);
}

fn empty() -> Measured {
    Measured {
        cells: BTreeMap::new(),
        outputs: BTreeMap::new(),
        expected: BTreeMap::new(),
        failures: Vec::new(),
    }
}

fn settle(fx: &Fixture) {
    for _ in 0..SETTLE {
        tick(fx);
    }
}

fn warm(shape: Shape) -> Measured {
    let fx = fixture_with(shape, true);
    settle(&fx);
    let mut m = empty();
    for step in STEPS {
        if step == "mutation" {
            stale_agent_state(&fx);
        }
        measure_tick(&fx, step, &mut m);
    }
    m
}

/// Each step in its own fresh process on a store this process settled: the steady tick, then
/// the tick that writes back a stale `agent_state`. A child must exit successfully.
fn cold(shape: Shape) -> Measured {
    let fx = fixture_with(shape, true);
    settle(&fx);
    let root: PathBuf = fx.ctx.store.root().to_path_buf();
    let mut m = empty();
    for step in STEPS {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::sync_budget::cold_variant",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(COLD_VAR, &root)
            .env(RUNNING_VAR, &fx.running)
            .env(STEP_VAR, step)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "cold {} {step} exited {:?}:\n{text}\n{}",
            shape.name,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        let line = text
            .lines()
            .find_map(|line| line.split_once("SYNCBUDGET ").map(|(_, json)| json))
            .unwrap_or_else(|| panic!("cold {} {step} printed no result:\n{text}", shape.name));
        let child = decode(line);
        m.cells.extend(child.cells);
        m.outputs.extend(child.outputs);
        m.expected.extend(child.expected);
        m.failures.extend(child.failures);
    }
    m
}

#[test]
#[ignore = "spawned by sync_tick_work_stays_independent_of_history for one cold step"]
fn cold_variant() {
    let (Some(root), Ok(running), Ok(step)) = (
        std::env::var_os(COLD_VAR),
        std::env::var(RUNNING_VAR),
        std::env::var(STEP_VAR),
    ) else {
        return;
    };
    let fx = reopen(Path::new(&root), &running);
    let mut m = empty();
    if step == "mutation" {
        stale_agent_state(&fx);
    }
    measure_tick(&fx, &step, &mut m);
    println!("SYNCBUDGET {}", encode(&m));
}

#[test]
#[ignore = "run by scripts/work-budget.sh (the gate)"]
fn sync_tick_work_stays_independent_of_history() {
    let mut failures = Vec::new();
    let mut text = String::new();
    for (mode, measured) in [
        ("warm", [BASE, GROWN, NESTED].map(warm)),
        ("cold", [BASE, GROWN, NESTED].map(cold)),
    ] {
        let [base, grown, nested] = &measured;
        for (m, name) in [(base, "base"), (grown, "grown"), (nested, "nested")] {
            failures.extend(m.failures.iter().map(|f| format!("{mode}/{name}: {f}")));
            text.push_str(&format!("{mode} {name}:\n{}", report(m)));
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
        "hot-sync work budget exceeded:\n  {}",
        failures.join("\n  ")
    );
}
