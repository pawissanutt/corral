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
use std::path::PathBuf;

use corral_core::work::{self, Counter, HotScope, WorkSnapshot};

use super::action_budget_checks::{
    Cells, Walk, effect_failures, followup_failures, preconditions, section_cells, sections,
    submitted, walk,
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

/// Ordinal-derived identifiers (task, run, plan, attempt and child numbers) and content
/// digests differ by size; every other character of an output, digits included, must not.
fn normalize(output: &str) -> String {
    let output = &mask_times(output);
    let mut out = String::with_capacity(output.len());
    let mut chars = output.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !c.is_ascii_alphanumeric() {
            out.push(c);
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(i, next)) = chars.peek() {
            if !next.is_ascii_alphanumeric() {
                break;
            }
            end = i + next.len_utf8();
            chars.next();
        }
        let word = &output[start..end];
        let letters = word.trim_end_matches(|c: char| c.is_ascii_digit());
        let digits = &word[letters.len()..];
        let ordinal_prefix = ["T-", "-r", "-h", "review-", "reviewer:", "-H"]
            .iter()
            .any(|prefix| out.ends_with(prefix));
        if word.len() >= 40 && word.chars().all(|c| c.is_ascii_hexdigit()) {
            out.push_str("<digest>");
        } else if letters.is_empty() && ordinal_prefix {
            out.push('#');
        } else if !digits.is_empty()
            && (1..=3).contains(&letters.len())
            && letters.chars().all(|c| c.is_ascii_uppercase())
        {
            out.push_str(letters);
            out.push('#');
        } else if !digits.is_empty() && matches!(letters, "r" | "h") && out.ends_with('-') {
            // A worktree branch's run suffix (`-r11`) or a historic child (`-h001`).
            out.push_str(letters);
            out.push('#');
        } else if !digits.is_empty() && letters.len() == 1 && ordinal_prefix {
            out.push_str(letters);
            out.push('#');
        } else {
            out.push_str(word);
        }
    }
    out
}

/// RFC 3339 stamps: the fixture's are pinned, but a gate's commands are stamped by the gate
/// script's own process clock.
fn mask_times(output: &str) -> String {
    let bytes = output.as_bytes();
    let shape = b"dddd-dd-ddTdd:dd:dd";
    let mut out = String::with_capacity(output.len());
    let mut i = 0;
    while i < bytes.len() {
        let fits = bytes.len() - i >= shape.len()
            && shape.iter().zip(&bytes[i..]).all(|(s, b)| match s {
                b'd' => b.is_ascii_digit(),
                s => s == b,
            });
        if fits {
            let mut end = i + shape.len();
            if bytes.get(end) == Some(&b'.') {
                end += 1;
                while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                }
            }
            if bytes.get(end) == Some(&b'Z') {
                out.push_str("<time>");
                i = end + 1;
                continue;
            }
        }
        let c = output[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Every text file of the store's directory, by path: what a pass writes is compared.
fn store_files(bench: &Bench) -> BTreeMap<PathBuf, String> {
    let root = bench.ctx.store.dir().to_path_buf();
    let mut files = BTreeMap::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path)
                && !path.to_string_lossy().contains(".db")
            {
                files.insert(path.strip_prefix(&root).unwrap().to_path_buf(), text);
            }
        }
    }
    files
}

/// The pass's durable output: the acted task's head fields, the events it logged and the
/// text files it wrote or changed.
fn durable_output(bench: &Bench, events: usize, files: &BTreeMap<PathBuf, String>) -> String {
    let mut out = Vec::new();
    let shell = bench
        .ctx
        .store
        .head(&bench.id)
        .unwrap()
        .to_shell_task()
        .unwrap();
    let mut shell = serde_json::to_value(shell).unwrap();
    shell["updated"] = serde_json::Value::Null;
    out.push(format!("task {shell}"));
    for event in bench.ctx.store.events().unwrap().into_iter().skip(events) {
        out.push(format!("event {} {}", event.kind, event.detail));
    }
    for (path, text) in store_files(bench) {
        if files.get(&path) != Some(&text) {
            out.push(format!("file {}:\n{text}", path.display()));
        }
    }
    // Each store has its own temporary root and cache directory.
    let cache = bench.ctx.store.cache_root().unwrap();
    let per_store = cache
        .parent()
        .unwrap_or(&cache)
        .to_string_lossy()
        .into_owned();
    out.join("\n")
        .replace(&per_store, "<store-cache>")
        .replace(&bench.root.to_string_lossy().into_owned(), "<root>")
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
    crate::resources::pin_psi(Some(super::action_budget_fixture::PSI));
    let mut m = Measured::default();
    for (section, _) in sections(kind) {
        m.sections
            .insert((*section).to_owned(), section_cells(bench, section));
    }
    let entry = entry(kind, bench);
    let (before, volumes) = walk(bench);
    let effects = submitted(bench, kind);
    let effects_before = super::action_budget_checks::effects(bench);
    bench.probe.take();
    bench.probe.take_live();
    let (herdr, git) = (
        corral_herdr::work::invocations(),
        corral_git::work::invocations(),
    );
    let files = store_files(bench);
    let events = bench.ctx.store.events().unwrap().len();
    let start = work::snapshot();
    let output = run(&entry, bench);
    let delta = work::snapshot().since(&start);
    let (herdr, git) = (
        corral_herdr::work::invocations() - herdr,
        corral_git::work::invocations() - git,
    );
    let samples = bench.probe.take();
    let live = bench.probe.take_live();
    let (after, after_volumes) = walk(bench);
    let effects = submitted(bench, kind) - effects;
    m.failures.extend(effect_failures(
        kind,
        &effects_before,
        &super::action_budget_checks::effects(bench),
    ));
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
        .extend(followup_failures(&m.cells, &before, &after, &walks, live));
    m.output = normalize(&format!(
        "{output}\n{}",
        durable_output(bench, events, &files)
    ));
    m.before = Some(before);
    m.after = Some(after);
    m
}

/// A store built and settled here, then measured in a fresh process: a cold cache and store.
fn cold(kind: Kind, variant: Variant) -> Measured {
    let bench = build(kind, variant);
    settle(&bench);
    measure_cold(&bench, kind)
}

/// [`measure`] of `bench` in a fresh process: a cold cache and store.
pub(super) fn measure_cold(bench: &Bench, kind: Kind) -> Measured {
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
        "cold {kind:?} exited {:?}:\n{text}\n{}",
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
        let (b, o) = base
            .output
            .lines()
            .zip(other.output.lines())
            .find(|(b, o)| b != o)
            .unwrap_or(("<end>", "<end>"));
        let (bc, oc): (Vec<char>, Vec<char>) = (b.chars().collect(), o.chars().collect());
        let first = bc
            .iter()
            .zip(&oc)
            .position(|(x, y)| x != y)
            .unwrap_or(bc.len().min(oc.len()));
        let from = first.saturating_sub(80);
        let at = |line: &[char]| line.iter().skip(from).take(240).collect::<String>();
        failures.push(format!(
            "output differs:\n    base:  {}\n    {name}: {}",
            at(&bc),
            at(&oc)
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
    // A send-back reads none of the repaired run's obligations or frozen payload, which the
    // emitted repair store adds: no kind's own records grow.
    let want_own = own(&base.cells);
    if own(&other.cells) != want_own {
        failures.push(format!(
            "own-record (rows, bytes) = {:?} at {name}, expected {want_own:?} (base {:?})",
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
