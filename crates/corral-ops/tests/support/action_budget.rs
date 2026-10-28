//! The actions-launch action budget (plan T-253 section 4, "Action budget"), run by
//! `scripts/work-budget.sh`. Worker launch, planner launch, plan review through a pane and the
//! PM, and automatic plan acceptance with one scope and with a decomposition are taken by a
//! real sync tick; repair with and without an inbox and aggregate acceptance with and without
//! code run through their real entries (Kind::ticked). Each kind is measured warm in this
//! process and cold in a fresh one, on its base, grown and emitted stores. Base and grown give
//! identical unattributed cells; base and emitted too, with the emitted cells grown by exactly
//! the added rows' stored bytes the prompt shows; no counted hydration; every inclusive total
//! is the sum of its cells; each FU-3 cell is its expected walk; outputs equal but for ids.
use std::collections::BTreeMap;

use corral_core::work::{self, Counter, HotScope, WorkSnapshot};

use super::action_budget_checks::{
    Cells, Walk, followup_failures, preconditions, section_cells, sections, submitted, walk,
};
use super::action_budget_fixture::{Bench, KINDS, Kind, Variant, build, reopen, settle};
use super::fixture;

const COLD_ROOT: &str = "CORRAL_ACTION_BUDGET_ROOT";
const COLD_KIND: &str = "CORRAL_ACTION_BUDGET_KIND";
const COLD_ID: &str = "CORRAL_ACTION_BUDGET_ID";
const COLD_ATTEMPT: &str = "CORRAL_ACTION_BUDGET_ATTEMPT";

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

/// Ordinal-derived ids and stamps differ by size; the rest of an output must not.
fn normalize(output: &str) -> String {
    let mut out = String::with_capacity(output.len());
    let mut digits = false;
    for c in output.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Measured {
    pub(super) cells: Cells,
    pub(super) output: String,
    before: Option<Walk>,
    after: Option<Walk>,
    /// The emitted (rows, bytes) each prompt section reads on this store.
    sections: BTreeMap<String, (u64, u64)>,
    pub(super) failures: Vec<String>,
}

/// The action's real entry, its inputs read before counting starts.
enum Entry {
    Tick,
    Repair(crate::integration::RepairKey),
    Aggregate,
}

fn entry(kind: Kind, bench: &Bench) -> Entry {
    match kind {
        Kind::RepairLegacy | Kind::RepairInbox => {
            let task = bench.ctx.store.get(&bench.id).unwrap();
            let run = task.current_run().unwrap();
            Entry::Repair(crate::integration::RepairKey::new(&task, run, "regression").unwrap())
        }
        Kind::AggregateCode | Kind::AggregateNoCode => Entry::Aggregate,
        _ => Entry::Tick,
    }
}

fn run(entry: &Entry, bench: &Bench) -> String {
    match entry {
        Entry::Tick => format!("{:?}", crate::sync_at(&bench.ctx, fixture::now()).unwrap()),
        Entry::Repair(key) => {
            let _hot = HotScope::enter();
            crate::dispatch::auto_request_changes(
                &bench.ctx,
                &bench.id,
                "repair regression",
                1,
                corral_core::AutopilotState {
                    stage: corral_core::AutoStage::Gating,
                    reason: None,
                    sha: None,
                    at: fixture::now(),
                },
                key,
            )
            .map(|task| task.status.label().to_owned())
            .unwrap_or_else(|e| format!("error: {e:#}"))
        }
        Entry::Aggregate => {
            let _hot = HotScope::enter();
            crate::plan_accept(&bench.ctx, &bench.id, &bench.attempt, true)
                .map(|acceptance| format!("{:?}", acceptance.state))
                .unwrap_or_else(|e| format!("error: {e:#}"))
        }
    }
}

pub(super) fn measure(bench: &Bench, kind: Kind) -> Measured {
    corral_core::clock::pin_thread(Some(fixture::now()));
    let mut m = Measured::default();
    for (section, _) in sections(kind) {
        m.sections
            .insert((*section).to_owned(), section_cells(bench, section));
    }
    let entry = entry(kind, bench);
    let (before, volumes) = walk(bench);
    let effects = submitted(bench, kind);
    bench.probe.take();
    let (herdr, git) = (
        corral_herdr::work::invocations(),
        corral_git::work::invocations(),
    );
    let start = work::snapshot();
    let output = run(&entry, bench);
    let delta = work::snapshot().since(&start);
    let (herdr, git) = (
        corral_herdr::work::invocations() - herdr,
        corral_git::work::invocations() - git,
    );
    let samples = bench.probe.take();
    let (after, after_volumes) = walk(bench);
    let effects = submitted(bench, kind) - effects;
    if let Err(error) = delta.check_identity() {
        m.failures.push(format!("identity: {error}"));
    }
    m.failures.extend(preconditions(
        bench,
        kind,
        (&volumes, &after_volumes),
        &delta,
        &samples,
        &output,
        effects,
    ));
    for site in delta.sites.iter().filter(|site| site.followup.is_none()) {
        m.failures.push(format!(
            "counted hydration at {}:{}",
            site.location.file(),
            site.location.line()
        ));
    }
    m.cells = flatten(&delta, herdr, git);
    let walks: Vec<Walk> = samples.iter().map(|(_, walk)| *walk).collect();
    m.failures
        .extend(followup_failures(&m.cells, &before, &after, &walks));
    m.output = normalize(&output);
    m.before = Some(before);
    m.after = Some(after);
    m
}

/// A store built and settled here, then measured in a fresh process: a cold cache and store.
fn cold(kind: Kind, variant: Variant) -> Measured {
    let bench = build(kind, variant);
    settle(&bench);
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "submission_paths_tests::action_budget::action_budget_cold_variant",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(COLD_ROOT, &bench.root)
        .env(COLD_KIND, kind.name())
        .env(COLD_ID, &bench.id)
        .env(COLD_ATTEMPT, &bench.attempt)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "cold {kind:?} {} exited {:?}:\n{text}\n{}",
        variant.name(),
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let line = text
        .lines()
        .find_map(|line| line.split_once("ACTIONBUDGET ").map(|(_, json)| json))
        .unwrap_or_else(|| panic!("cold {kind:?} printed no result:\n{text}"));
    serde_json::from_str(line).unwrap()
}

#[test]
#[ignore = "spawned by action_budget_actions_launch_kinds_do_not_follow_history"]
fn action_budget_cold_variant() {
    let (Some(root), Ok(kind), Ok(id), Ok(attempt)) = (
        std::env::var_os(COLD_ROOT),
        std::env::var(COLD_KIND),
        std::env::var(COLD_ID),
        std::env::var(COLD_ATTEMPT),
    ) else {
        return;
    };
    let bench = reopen(std::path::Path::new(&root), &id, &attempt);
    let m = measure(&bench, Kind::parse(&kind));
    println!("ACTIONBUDGET {}", serde_json::to_string(&m).unwrap());
}

fn warm(kind: Kind, variant: Variant) -> Measured {
    let bench = build(kind, variant);
    settle(&bench);
    measure(&bench, kind)
}

pub(super) fn unattributed(cells: &Cells) -> impl Iterator<Item = (&String, &u64)> {
    cells
        .iter()
        .filter(|(key, _)| key.ends_with(".unattributed") || key.starts_with("calls."))
}

fn get(cells: &Cells, key: &str) -> u64 {
    cells.get(key).copied().unwrap_or(0)
}

pub(super) fn emitted(cells: &Cells) -> (u64, u64) {
    let rows = Counter::DECODES
        .iter()
        .map(|c| get(cells, &format!("{}.emitted", c.name())))
        .sum();
    (rows, get(cells, "decoded_doc_bytes.emitted"))
}

pub(super) fn own(cells: &Cells) -> (u64, u64) {
    let rows = Counter::DECODES
        .iter()
        .map(|c| get(cells, &format!("{}.own_record", c.name())))
        .sum();
    (rows, get(cells, "decoded_doc_bytes.own_record"))
}

/// Base against one larger variant: every unattributed cell and output equal; the emitted
/// cells equal on grown and grown by exactly the shown sections' added rows and bytes on
/// emitted; own-record cells equal.
fn compare(kind: Kind, base: &Measured, other: &Measured, variant: Variant) -> Vec<String> {
    let name = variant.name();
    let mut failures = Vec::new();
    for (cell, x) in unattributed(&base.cells) {
        let y = get(&other.cells, cell);
        if *x != y {
            failures.push(format!("{cell}: {x} at base, {y} at {name}"));
        }
    }
    for (cell, y) in unattributed(&other.cells) {
        if !base.cells.contains_key(cell) && *y != 0 {
            failures.push(format!("{cell}: absent at base, {y} at {name}"));
        }
    }
    if base.output != other.output {
        failures.push(format!(
            "output differs:\n    base:  {}\n    {name}: {}",
            base.output, other.output
        ));
    }
    let (base_emitted, other_emitted) = (emitted(&base.cells), emitted(&other.cells));
    let mut want = base_emitted;
    if variant == Variant::Emitted {
        for (section, times) in sections(kind) {
            let (b, o) = (base.sections[*section], other.sections[*section]);
            want.0 += times * (o.0 - b.0);
            want.1 += times * (o.1 - b.1);
        }
    }
    if other_emitted != want {
        failures.push(format!(
            "emitted (rows, bytes) = {other_emitted:?} at {name}, expected {want:?} (base {base_emitted:?})"
        ));
    }
    if own(&base.cells) != own(&other.cells) {
        failures.push(format!(
            "own-record (rows, bytes) = {:?} at {name}, {:?} at base",
            own(&other.cells),
            own(&base.cells)
        ));
    }
    failures
}

fn report(m: &Measured) -> String {
    let line: Vec<String> = m
        .cells
        .iter()
        .filter(|(_, n)| **n != 0)
        .map(|(k, n)| format!("{k}={n}"))
        .collect();
    line.join(" ")
}

#[test]
fn action_budget_actions_launch_kinds_do_not_follow_history() {
    let only = std::env::var("ACTION_BUDGET_KIND").ok();
    let mut failures = Vec::new();
    let mut text = String::new();
    for kind in KINDS
        .into_iter()
        .filter(|k| only.as_deref().is_none_or(|o| o == k.name()))
    {
        for (mode, measure) in [
            ("warm", warm as fn(Kind, Variant) -> Measured),
            ("cold", cold),
        ] {
            let [base, grown, emitted] =
                [Variant::Base, Variant::Grown, Variant::Emitted].map(|v| measure(kind, v));
            for (m, variant) in [(&base, "base"), (&grown, "grown"), (&emitted, "emitted")] {
                failures.extend(
                    m.failures
                        .iter()
                        .map(|f| format!("{kind:?} {mode}/{variant}: {f}")),
                );
                text.push_str(&format!("{kind:?} {mode}/{variant}: {}\n", report(m)));
            }
            for (other, variant) in [(&grown, Variant::Grown), (&emitted, Variant::Emitted)] {
                failures.extend(
                    compare(kind, &base, other, variant)
                        .into_iter()
                        .map(|f| format!("{kind:?} {mode}: {f}")),
                );
            }
        }
    }
    if std::env::var_os("WORK_BUDGET_REPORT").is_some() || !failures.is_empty() {
        println!("{text}");
    }
    assert!(
        failures.is_empty(),
        "action budget exceeded:\n  {}",
        failures.join("\n  ")
    );
}
