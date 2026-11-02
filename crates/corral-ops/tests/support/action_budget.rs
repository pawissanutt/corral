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
use super::action_budget_fixture::{Bench, Kind, Variant, build, reopen, settle};
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
pub(super) fn normalize(output: &str) -> String {
    let output = &mask_counter(&mask_times(output), "\"next_seq\":");
    // A delivered message's sequence number and id suffix: ordinals of the inbox's counter.
    let output = &mask_counter(output, "\"seq\":");
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
        let hex = word.chars().all(|c| c.is_ascii_hexdigit());
        if word.len() >= 40 && hex {
            out.push_str("<digest>");
        } else if word.len() == 8 && hex && output[end..].starts_with('-') {
            // A snapshot's content prefix: each store's repository has its own commits.
            out.push_str("<sha8>");
        } else if word.len() == 7 && hex && (out.ends_with(" at ") || out.ends_with(" at `")) {
            // A landed commit's short sha.
            out.push_str("<short>");
        } else if letters == "review" && !digits.is_empty() {
            out.push_str("review#");
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

/// The value after `field`: an inbox's sequence counter, an ordinal of the effects it applied.
fn mask_counter(output: &str, field: &str) -> String {
    let mut out = String::with_capacity(output.len());
    let mut rest = output;
    while let Some(at) = rest.find(field) {
        let (head, tail) = rest.split_at(at + field.len());
        out.push_str(head);
        let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 {
            out.push('#');
        }
        rest = &tail[digits..];
    }
    out.push_str(rest);
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
    files_under(bench.ctx.store.dir().to_path_buf())
}

/// Every generated prompt under the store's scratch root, by path: the worker's registered
/// prompt copy, the planner's `prompt.md` and both plan reviews' attempt prompts.
fn prompt_files(bench: &Bench) -> BTreeMap<PathBuf, String> {
    let layout = bench.ctx.store.storage_layout().unwrap();
    let mut files = files_under(layout.scratch_root().to_path_buf());
    files.retain(|path, _| path.extension().is_some_and(|e| e == "md"));
    files
}

fn files_under(root: PathBuf) -> BTreeMap<PathBuf, String> {
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
fn durable_output(
    bench: &Bench,
    events: usize,
    files: &BTreeMap<PathBuf, String>,
    prompts: &BTreeMap<PathBuf, String>,
) -> String {
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
    strip_grown(&mut shell);
    out.push(format!("task {shell}"));
    for event in bench.ctx.store.events().unwrap().into_iter().skip(events) {
        out.push(format!("event {} {}", event.kind, event.detail));
    }
    // Snapshot names carry a content prefix of each store's own commits: order them as
    // normalized, so the order is the same on every store.
    let mut changed: Vec<String> = store_files(bench)
        .into_iter()
        .filter(|(path, text)| files.get(path) != Some(text))
        .map(|(path, text)| format!("file {}:\n{text}", path.display()))
        .collect();
    changed.sort_by_cached_key(|entry| normalize(entry));
    out.extend(changed);
    for (path, text) in prompt_files(bench) {
        if prompts.get(&path) != Some(&text) {
            out.push(format!("prompt {}:\n{text}", path.display()));
        }
    }
    // Each store has its own temporary root and cache directory.
    let cache = bench.ctx.store.cache_root().unwrap();
    let per_store = cache
        .parent()
        .unwrap_or(&cache)
        .to_string_lossy()
        .into_owned();
    let scratch = bench.ctx.store.storage_layout().unwrap();
    let scratch = scratch.scratch_root().to_string_lossy().into_owned();
    mask_clock_ids(
        &out.join("\n")
            .replace(&scratch, "<scratch>")
            .replace(&per_store, "<store-cache>")
            .replace(&bench.root.to_string_lossy().into_owned(), "<root>"),
    )
}

/// Drops the rows the fixture grew into the shell's runs: what history a store holds is the
/// variant, not the pass's output. The pass's own rows (its new run, delivered effect, current
/// child) stay, and the inbox's applied set, an ordinal record of grown effects, is masked.
fn strip_grown(shell: &mut serde_json::Value) {
    let grown = |v: &serde_json::Value, field: &str, prefix: &str| {
        v[field].as_str().is_some_and(|s| s.contains(prefix))
    };
    for run in shell["runs"].as_array_mut().into_iter().flatten() {
        let mut retain = |field: &str, key: &str, prefix: &str| {
            if let Some(rows) = run[field].as_array_mut() {
                rows.retain(|row| !grown(row, key, prefix));
            }
        };
        retain("review_runs", "id", "-review-h");
        retain("switches", "from", "historic-");
        retain("checkpoints", "artifact", "artifacts/history/");
        if let Some(entries) = run["usage"]["entries"].as_array_mut() {
            entries.retain(|e| !grown(e, "model", "historic-"));
        }
        for family in ["waits", "decisions"] {
            if let Some(rows) = run["obligations"][family].as_array_mut() {
                rows.retain(|r| {
                    !r["request"]["id"]
                        .as_str()
                        .is_some_and(|s| s.contains("-build-") || s.contains("-branch-"))
                });
            }
        }
        if let Some(cursors) = run["inbox"]["observe_cursors"].as_array_mut() {
            cursors.retain(|c| c["adapter"] != "historic");
        }
        if run["inbox"]["effects_applied"].is_array() {
            run["inbox"]["effects_applied"] = "<applied>".into();
        }
    }
}

/// Runs of twelve or more digits: real-clock nanoseconds in scratch registration ids.
pub(super) fn mask_clock_ids(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut digits = String::new();
    // Only standalone runs: a digest's hex digits stay for its own normalization.
    let mut joined = false;
    for c in text.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            if digits.is_empty() {
                joined = out
                    .chars()
                    .last()
                    .is_some_and(|p| p.is_ascii_alphanumeric());
            }
            digits.push(c);
            continue;
        }
        if digits.len() >= 12 && !joined && !c.is_ascii_alphanumeric() {
            // A gate attempt's `<pid>-<nanos>`: each cold store runs in its own process.
            if let Some(stem) = out.strip_suffix('-') {
                let pid = stem.len() - stem.trim_end_matches(|d: char| d.is_ascii_digit()).len();
                if pid > 0 {
                    out.truncate(stem.len() - pid);
                    out.push_str("<pid>-");
                }
            }
            out.push_str("<nanos>");
        } else {
            out.push_str(&digits);
        }
        digits.clear();
        if c != '\0' {
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
    /// Each emitted section's expected rendering, from the full task's stored rows.
    references: BTreeMap<String, String>,
    pub(super) failures: Vec<String>,
    #[serde(default)]
    inputs: super::action_budget_trace::Inputs,
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
            Entry::Repair(
                crate::integration::RepairKey::new(&task, &run_head(run), "regression").unwrap(),
            )
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
    let _entropy = crate::fixture_role_entropy(2);
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
    let prompts = prompt_files(bench);
    let events = bench.ctx.store.events().unwrap().len();
    let events_before = events;
    let capture = super::action_budget_trace::Capture::start();
    let start = work::snapshot();
    let output = run(&entry, bench);
    let delta = work::snapshot().since(&start);
    m.inputs = capture.finish();
    let (herdr, git) = (
        corral_herdr::work::invocations() - herdr,
        corral_git::work::invocations() - git,
    );
    let samples = bench.probe.take();
    let live = bench.probe.take_live();
    let (after, after_volumes) = walk(bench);
    m.references = super::action_budget_prompts::references(bench, kind);
    let effects = submitted(bench, kind) - effects;
    let effect = effect_failures(
        kind,
        &effects_before,
        &super::action_budget_checks::effects(bench),
    );
    if !effect.is_empty() {
        // Why the pass did not act: its error events and reports.
        let events = bench.ctx.store.events().unwrap();
        for event in events
            .iter()
            .skip(events_before)
            .filter(|e| e.kind.contains("error"))
        {
            m.failures
                .push(format!("effect: {} {}", event.kind, event.detail));
        }
    }
    m.failures.extend(effect);
    if matches!(
        kind,
        Kind::RepairLegacy | Kind::RepairInbox | Kind::RepairTickLegacy | Kind::RepairTickInbox
    ) {
        let head = bench.ctx.store.head(&bench.id).unwrap();
        if let Some(round) = head
            .current_run
            .as_deref()
            .and_then(|run| run.changes_round.as_ref())
            && round.at != bench.ctx.store.now()
        {
            m.failures.push(format!(
                "changes-round timestamp is not the store clock: {}",
                round.at
            ));
        }
    }
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
    // A message id's sequence suffix (`T-#_R##10` once normalized).
    m.output = mask_counter(
        &normalize(&format!(
            "{output}\n{}",
            durable_output(bench, events, &files, &prompts)
        )),
        "##",
    );
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

const PACKET: &str = "## Portable resume contract";

/// The output without its resume-contract sections, and each section's JSON packet.
pub(super) fn cut_packets(output: &str) -> (String, Vec<serde_json::Value>) {
    let (mut rest, mut packets, mut inside, mut block) = (Vec::new(), Vec::new(), false, None);
    for line in output.lines() {
        if line == PACKET {
            inside = true;
            continue;
        }
        if inside && (line.starts_with("## ") || line.starts_with("prompt ")) {
            inside = false;
        }
        if !inside {
            rest.push(line);
            continue;
        }
        match (&mut block, line.trim()) {
            (None, "```json" | "<corral-resume>") => block = Some(String::new()),
            (Some(text), "```" | "</corral-resume>") => {
                packets.push(serde_json::from_str(text).unwrap_or(serde_json::Value::Null));
                block = None;
            }
            (Some(text), _) => {
                text.push_str(line);
                text.push('\n');
            }
            (None, _) => {}
        }
    }
    (rest.join("\n"), packets)
}

/// The paths at which two JSON values differ.
fn json_diff(a: &serde_json::Value, b: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for key in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                let (l, r) = (
                    x.get(key).unwrap_or(&Value::Null),
                    y.get(key).unwrap_or(&Value::Null),
                );
                json_diff(l, r, &format!("{path}/{key}"), out);
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (l, r)) in x.iter().zip(y).enumerate() {
                json_diff(l, r, &format!("{path}/{i}"), out);
            }
        }
        _ if a != b => out.push(path.to_owned()),
        _ => {}
    }
}

/// The emitted packet adds the emitted obligations and changes nothing else.
pub(super) fn packet_failures(
    base: &[serde_json::Value],
    emitted: &[serde_json::Value],
) -> Vec<String> {
    if base.is_empty() || base.len() != emitted.len() || base.iter().any(|p| p.is_null()) {
        return vec![format!(
            "resume packets: {} at base, {} emitted (or unparsed)",
            base.len(),
            emitted.len()
        )];
    }
    let mut paths = Vec::new();
    for (b, e) in base.iter().zip(emitted) {
        json_diff(b, e, "", &mut paths);
    }
    let obligation = |p: &str| {
        p.split('/')
            .any(|s| matches!(s, "waits" | "decisions") || s.ends_with("obligations"))
    };
    let mut failures: Vec<String> = paths
        .iter()
        .filter(|p| !obligation(p))
        .map(|p| format!("emitted resume packet changed {p}"))
        .collect();
    if !paths.iter().any(|p| obligation(p)) {
        failures.push("emitted resume packet lacks the emitted obligations".into());
    }
    failures
}

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
    if variant == Variant::Emitted {
        // The sections whose emitted rows this store really added.
        // The sections whose emitted rows this store really added.
        let grew: Vec<&str> = sections(kind)
            .iter()
            .map(|(s, _)| *s)
            .filter(|s| other.sections[*s].0 > base.sections[*s].0)
            .collect();
        failures.extend(super::action_budget_prompts::emitted_output_failures(
            kind,
            &grew,
            (&base.output, &base.references),
            (&other.output, &other.references),
        ));
    } else if base.output != other.output {
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

// Separate cases let nextest schedule bounded work, instead of two multi-minute tests.
#[path = "action_budget_cases.rs"]
mod cases;

fn budget(kind: Kind, cold_mode: bool) {
    let mut failures = Vec::new();
    let mut text = String::new();
    let mode = if cold_mode { "cold" } else { "warm" };
    let measure = if cold_mode { cold } else { warm };
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
    let failed_before = failures.len();
    for (other, variant) in [(&grown, Variant::Grown), (&emitted, Variant::Emitted)] {
        failures.extend(
            compare(kind, &base, other, variant)
                .into_iter()
                .map(|f| format!("{kind:?} {mode}: {f}")),
        );
    }
    if failures.len() != failed_before
        || [&base, &grown, &emitted]
            .iter()
            .any(|m| !m.failures.is_empty())
    {
        for (m, variant) in [(&base, "base"), (&grown, "grown"), (&emitted, "emitted")] {
            m.inputs.print(&format!("{kind:?} {mode}/{variant}"));
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

/// The run projection a converted helper takes.
fn run_head(run: &corral_core::Run) -> corral_core::RunHead {
    corral_core::RunHead::of(run).unwrap()
}
