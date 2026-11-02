//! The hot-reads work budget (plan T-253 sections 4 and 5), run by `scripts/work-budget.sh`.
//!
//! `pm next`, `automation status` and a board frame are measured through their production
//! scopes: the CLI evaluation (snapshot, derivation preparation, then `HotScope`) and the
//! board's own frame scope, so preparation and recovery stay outside the measured work. Each
//! runs on the base store, the grown store (2 vs 5 terminal tasks/history rows per active
//! task, artifacts included) and the nested store (1 vs 3 nested rows inside every current
//! container), warm in this process and cold in a fresh one, and again after a real
//! current-run mutation, an unrelated record write and deletion of the cache root. Between the
//! base and each larger variant every unattributed cell is unchanged, the unattributed
//! hydration cell is 0, every inclusive total is the sum of its cells, and FU-3 reads stay
//! within v2 header-only bounds on that variant's rows. The measured pass's own resource
//! samples and branch are checked before any counter is compared.

use std::collections::BTreeMap;

use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_ops::Ctx;

use super::hot_budget_checks::{Expected, expected, followup_failures, preconditions};
use super::hot_budget_fixture::{BASE, Fixture, GROWN, NESTED, Shape, fixture, shape};

pub(super) type Cells = BTreeMap<String, u64>;

pub(super) const PATHS: [&str; 3] = ["pm_next", "automation_status", "board_frame"];
const STEPS: [&str; 4] = ["steady", "mutation", "unrelated_record", "cache_loss"];
const SHAPE_VAR: &str = "CORRAL_HOT_BUDGET_SHAPE";

/// `pm_cmd::evaluate`: one snapshot, the production preparation boundary, then the scope.
fn evaluate(ctx: &Ctx, pass: impl FnOnce(&Ctx) -> String) -> String {
    let snapshot = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    corral_ops::lesson_rows::prepare_evaluation(&snapshot).unwrap();
    let _hot = HotScope::enter();
    pass(&snapshot)
}

/// Each path's production reads; returns the output that later steps must reproduce.
fn run_path(ctx: &Ctx, path: &str) -> String {
    let json = |value: serde_json::Value| value.to_string();
    match path {
        // `pm next --json` (pm_cmd::readback).
        "pm_next" => evaluate(ctx, |s| {
            let workset = corral_ops::pm_workset(s).unwrap();
            let summary = corral_ops::automation_summary(s).unwrap();
            json(serde_json::json!({"workset": workset, "summary": summary}))
        }),
        // `automation status` (mode_cmd::status and cmds::dispatch_status::print).
        "automation_status" => evaluate(ctx, |s| {
            let workset = corral_ops::pm_workset(s).unwrap();
            let effective = s.store.effective_operating_policy().unwrap();
            let config = s.store.config_layers().unwrap().config().unwrap();
            let approval = s.store.operating_state().unwrap().approval;
            let usage = corral_ops::slot_usage(s).unwrap();
            let automatic = corral_ops::automatic_dispatch_status(s).unwrap();
            json(serde_json::json!({
                "workset": workset, "effective": effective, "policy": config.automation,
                "approval": approval, "slots": format!("{usage:?}"),
                "free_slots": automatic.free_slots, "promotions": automatic.promotions_today,
            }))
        }),
        // One board frame: the board's own snapshot, preparation and frame scope.
        "board_frame" => {
            let frame = crate::board::BoardSnapshot::load(ctx).unwrap();
            format!("{}", frame.resources.line())
        }
        other => panic!("unknown path {other}"),
    }
}

pub(super) fn flatten(delta: &WorkSnapshot, herdr: u64, git: u64) -> Cells {
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

/// One measured variant: cells and output per (step, path), and the expected FU-3 work.
pub(super) struct Measured {
    pub(super) cells: BTreeMap<String, Cells>,
    pub(super) outputs: BTreeMap<String, String>,
    pub(super) expected: BTreeMap<String, Expected>,
    pub(super) failures: Vec<String>,
}

fn measure_path(fx: &Fixture, exp: &Expected, label: &str, path: &str, m: &mut Measured) {
    fx.probe.take();
    let (herdr, git) = (
        corral_herdr::work::invocations(),
        corral_git::work::invocations(),
    );
    let before = work::snapshot();
    let output = run_path(&fx.ctx, path);
    let delta = work::snapshot().since(&before);
    let samples = fx.probe.take();
    let key = format!("{label}/{path}");
    if let Err(error) = delta.check_identity() {
        m.failures.push(format!("{key}: {error}"));
    }
    m.failures.extend(preconditions(
        fx, exp, &key, path, &delta, &samples, &output,
    ));
    m.cells.insert(
        key.clone(),
        flatten(
            &delta,
            corral_herdr::work::invocations() - herdr,
            corral_git::work::invocations() - git,
        ),
    );
    m.outputs.insert(key, output);
}

fn step(fx: &Fixture, name: &str) {
    match name {
        "steady" => {}
        "mutation" => {
            let running = fx.running.clone();
            fx.ctx
                .store
                .update_head(&running, |head| {
                    if let Some(run) = head.current_run_mut() {
                        run.agent_state = Some("working".into());
                    }
                    Ok(())
                })
                .unwrap();
        }
        "unrelated_record" => {
            fx.ctx
                .store
                .save_record(
                    "hot-budget-unrelated",
                    "count",
                    &serde_json::json!({"n": 1}),
                )
                .unwrap();
        }
        "cache_loss" => {
            std::fs::remove_dir_all(fx.ctx.store.cache_root().unwrap()).unwrap();
        }
        other => panic!("unknown step {other}"),
    }
}

/// Steps in order, each measured twice so the second (warm) pass is the one kept; cold runs
/// measure the steady step once in a fresh process.
fn measure_shape(shape: Shape, warm: bool) -> Measured {
    let fx: Fixture = fixture(shape);
    let mut m = Measured {
        cells: BTreeMap::new(),
        outputs: BTreeMap::new(),
        expected: BTreeMap::new(),
        failures: Vec::new(),
    };
    let steps: &[&str] = if warm { &STEPS } else { &STEPS[..1] };
    for name in steps {
        step(&fx, name);
        let exp = expected(&fx);
        m.expected.insert((*name).to_owned(), exp.clone());
        for path in PATHS {
            if warm {
                measure_path(&fx, &exp, "warmup", path, &mut m);
            }
            measure_path(&fx, &exp, name, path, &mut m);
        }
    }
    m.cells.retain(|key, _| !key.starts_with("warmup/"));
    m.outputs.retain(|key, _| !key.starts_with("warmup/"));
    m
}

fn unattributed(cells: &Cells) -> impl Iterator<Item = (&String, &u64)> {
    cells
        .iter()
        .filter(|(key, _)| key.ends_with(".unattributed") || key.starts_with("calls."))
}

pub(super) fn compare(base: &Measured, larger: &Measured, name: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for (key, b) in &base.cells {
        let Some(l) = larger.cells.get(key) else {
            failures.push(format!("{key}: not measured at {name}"));
            continue;
        };
        let step = key.split('/').next().unwrap_or_default();
        failures.extend(followup_failures("base", key, b, &base.expected[step]));
        failures.extend(followup_failures(name, key, l, &larger.expected[step]));
        for (cell, x) in unattributed(b) {
            let y = l.get(cell).copied().unwrap_or(0);
            if *x != y {
                failures.push(format!("{key}: {cell} changes: {x} at base, {y} at {name}"));
            }
        }
        for (cells, variant) in [(b, "base"), (l, name)] {
            let hydrations = cells["full_task_hydrations.unattributed"];
            if hydrations != 0 {
                failures.push(format!(
                    "{variant}/{key}: unattributed hydration cell = {hydrations}"
                ));
            }
        }
    }
    failures
}

fn in_variant(m: &Measured, name: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for path in PATHS {
        for (before, step) in [
            ("mutation", "unrelated_record"),
            ("unrelated_record", "cache_loss"),
        ] {
            let Some(steady) = m.cells.get(&format!("{before}/{path}")) else {
                continue;
            };
            let key = format!("{step}/{path}");
            let Some(cells) = m.cells.get(&key) else {
                continue;
            };
            for (cell, x) in unattributed(steady) {
                let y = cells.get(cell).copied().unwrap_or(0);
                if *x != y {
                    failures.push(format!("{name}/{key}: {cell} = {y}, steady {x}"));
                }
            }
        }
        let output = |step: &str| m.outputs.get(&format!("{step}/{path}"));
        if path != "board_frame"
            && output("cache_loss").is_some()
            && output("cache_loss") != output("unrelated_record")
        {
            failures.push(format!("{name}/{path}: output changed after cache loss"));
        }
    }
    failures
}

pub(super) fn report(m: &Measured) -> String {
    let mut out = String::new();
    for (key, cells) in &m.cells {
        let line: Vec<String> = cells
            .iter()
            .filter(|(_, n)| **n != 0)
            .map(|(k, n)| format!("{k}={n}"))
            .collect();
        out.push_str(&format!("  {key}: {}\n", line.join(" ")));
    }
    out
}

pub(super) fn encode(m: &Measured) -> String {
    serde_json::json!({
        "cells": m.cells, "outputs": m.outputs, "expected": m.expected.iter().map(|(k, e)| (k.clone(), e.encode())).collect::<BTreeMap<_, _>>(),
        "failures": m.failures,
    })
    .to_string()
}

pub(super) fn decode(line: &str) -> Measured {
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    Measured {
        cells: serde_json::from_value(v["cells"].clone()).unwrap(),
        outputs: serde_json::from_value(v["outputs"].clone()).unwrap(),
        expected: v["expected"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, e)| (k.clone(), Expected::decode(e)))
            .collect(),
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
    println!("HOTBUDGET {}", encode(&measure_shape(shape(&name), false)));
}

#[test]
#[ignore = "run by scripts/work-budget.sh (the gate)"]
fn hot_reads_work_stays_independent_of_history() {
    let mut failures = Vec::new();
    let mut text = String::new();
    for (mode, measured) in [
        (
            "warm",
            [BASE, GROWN, NESTED].map(|s| measure_shape(s, true)),
        ),
        ("cold", [BASE, GROWN, NESTED].map(cold)),
    ] {
        let [base, grown, nested] = &measured;
        for (m, name) in [(base, "base"), (grown, "grown"), (nested, "nested")] {
            failures.extend(m.failures.iter().map(|f| format!("{mode}/{name}: {f}")));
            failures.extend(
                in_variant(m, name)
                    .into_iter()
                    .map(|f| format!("{mode}: {f}")),
            );
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
        "hot-reads work budget exceeded:\n  {}",
        failures.join("\n  ")
    );
}
