//! Failure selections and integrity issues are maintained at ingestion, not per evaluation.
//! A run's checkpoint is bound to its accepted-proof artifact revision, which changes on every
//! corral-mediated artifact edit, so any edit outside publication forces one rebuild. Each
//! rebuild starts a new epoch; live failure pointers from older epochs read as absent, which
//! retires selections of reports that no longer match without scanning them. Legacy backfills
//! commit once under the artifact storage lock. Immutable backfill keys include the input task
//! revision: an older snapshot never consumes a newer generation. Only the selected report's
//! bytes are reverified on every observation, together with its full selection predicate.
use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use corral_core::backend::{PageReq, RecordWrite, RowQuery, TaskPart};
use corral_core::{Artifact, Store, Task, TaskHead};
use serde::{Deserialize, Serialize};

use super::LoadCheck;
use crate::{Ctx, PmObservation, verification};

pub(super) const TITLE: &str = "PM isolated test result";
pub(super) const REPORTS: &str = "pm-load-reports-v2";
pub(super) const FAILURES: &str = "pm-load-failures-v2";
const RUNS: &str = "pm-load-runs-v2";
const GENERATIONS: &str = "pm-load-backfills-v2";
const GENERATION_FAILURES: &str = "pm-load-backfill-failures-v2";
const PREPARED: &str = "pm-load-checks-v2";

#[derive(Clone, Deserialize, Serialize)]
struct Selection {
    row: String,
    /// The rebuild epoch this pointer was written in, fixed-width.
    #[serde(default)]
    epoch: String,
}

#[derive(Deserialize, Serialize)]
struct Report {
    sha256: String,
    run: String,
    failure: Option<String>,
    issue: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Issue {
    row: String,
    message: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct RunState {
    /// The run's accepted-proof artifact revision this checkpoint describes.
    #[serde(default)]
    revision: String,
    /// Incremented by every rebuild, fixed-width for work-byte equality.
    #[serde(default)]
    epoch: String,
    issue: Option<Issue>,
}

struct PreparedRun {
    state: RunState,
    /// A backfill written after this snapshot opened is read only by its immutable key.
    generation: Option<(Store, String)>,
}

type Prepared = BTreeMap<(String, String), PreparedRun>;

fn key(parts: &impl Serialize) -> Result<String> {
    Ok(verification::digest(&serde_json::to_vec(parts)?))
}

fn failure_key(task: &str, run: &str, head: &str) -> Result<String> {
    key(&(task, run, head))
}

fn run_key(task: &str, run: &str) -> Result<String> {
    key(&(task, run))
}

fn report_key(task: &str, row: &str) -> String {
    format!("{task}/{row}")
}

fn generation_failure(generation: &str, head: &str) -> Result<String> {
    Ok(format!("{generation}/{}", key(&head)?))
}

fn epoch(n: u64) -> String {
    format!("{n:020}")
}

fn next_epoch(previous: Option<&RunState>) -> String {
    epoch(
        previous
            .and_then(|s| s.epoch.parse::<u64>().ok())
            .map_or(1, |n| n + 1),
    )
}

/// The head summary's accepted-proof revision for `run`, folded the same way.
fn revision<'a>(artifacts: impl Iterator<Item = &'a Artifact>, run: &str) -> String {
    let mut proof = corral_core::accepted_proof::AcceptedProof::default();
    for artifact in artifacts.filter(|a| a.run == run) {
        proof.fold(artifact);
    }
    proof.revision
}

/// db316644's selection predicate for one registration and head.
fn matches(artifact: &Artifact, check: &LoadCheck, task: &str, run: &str, head: &str) -> bool {
    artifact.title == TITLE
        && artifact.run == run
        && check.version == 1
        && check.task == task
        && check.run == run
        && check.head == head
        && check.under_load_exit != 0
}

fn read_check(ctx: &Ctx, artifact: &Artifact) -> Result<LoadCheck> {
    let path = ctx.store.dir().join(&artifact.path);
    let bytes = crate::artifact_read(ctx, artifact)?;
    if verification::digest(&bytes) != artifact.sha256 {
        bail!("test observation artifact hash changed: {}", path.display());
    }
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn classify(ctx: &Ctx, task: &str, artifact: &Artifact) -> Report {
    let mut report = Report {
        sha256: artifact.sha256.clone(),
        run: artifact.run.clone(),
        failure: None,
        issue: None,
    };
    match read_check(ctx, artifact) {
        Ok(check) => {
            if check.version == 1
                && check.task == task
                && check.run == artifact.run
                && check.under_load_exit != 0
            {
                report.failure = Some(check.head);
            }
        }
        Err(error) => report.issue = Some(format!("{error:#}")),
    }
    report
}

fn ingested(ctx: &Ctx, task: &str, row: &str, artifact: &Artifact) -> Result<Report> {
    if let Some(report) = ctx
        .store
        .record::<Report>(REPORTS, &report_key(task, row))?
        && report.sha256 == artifact.sha256
        && report.run == artifact.run
    {
        return Ok(report);
    }
    Ok(classify(ctx, task, artifact))
}

fn fold(
    state: &mut RunState,
    failures: &mut BTreeMap<String, Selection>,
    row: &str,
    report: &Report,
) {
    if let Some(head) = &report.failure {
        let selected = failures.entry(head.clone()).or_insert_with(|| Selection {
            row: row.to_owned(),
            epoch: state.epoch.clone(),
        });
        if selected.row.as_str() < row {
            selected.row = row.to_owned();
        }
    }
    if let Some(message) = &report.issue
        && state
            .issue
            .as_ref()
            .is_none_or(|issue| issue.row.as_str() <= row)
    {
        state.issue = Some(Issue {
            row: row.to_owned(),
            message: message.clone(),
        });
    }
}

/// Called under the publisher's storage lock and task transaction. Every attachment advances
/// its run's checkpoint, including unrelated titles, so a normal evaluation never scans. When
/// the stored checkpoint does not describe the run as it was before this publication (any edit
/// outside publication, a revival or a superseded Diff), the run is rebuilt in a new epoch.
pub(crate) fn published(ctx: &Ctx, task: &Task, index: usize, added: bool) -> Result<()> {
    let artifact = &task.artifacts[index];
    let after = revision(task.artifacts.iter(), &artifact.run);
    let before = if added {
        revision(task.artifacts[..index].iter(), &artifact.run)
    } else {
        after.clone()
    };
    let name = run_key(&task.id, &artifact.run)?;
    let previous: Option<RunState> = ctx.store.record(RUNS, &name)?;
    let row = corral_core::store::codec::ordinal_key(index + 1);
    let current = (artifact.title == TITLE).then(|| classify(ctx, &task.id, artifact));
    if let Some(report) = &current {
        ctx.store
            .save_record(REPORTS, &report_key(&task.id, &row), report)?;
    }
    let repair_issue = previous
        .as_ref()
        .and_then(|s| s.issue.as_ref())
        .is_some_and(|issue| issue.row == row)
        && current
            .as_ref()
            .is_some_and(|report| report.issue.is_none());
    let unchanged = previous.as_ref().is_some_and(|s| s.revision == before);
    let fresh_run = previous.is_none() && before == revision([].iter(), &artifact.run);
    let mut failures = BTreeMap::new();
    let mut state = if (unchanged || fresh_run) && !repair_issue {
        let mut state = previous.unwrap_or_default();
        if state.epoch.is_empty() {
            state.epoch = epoch(1);
        }
        if let Some(report) = &current {
            fold(&mut state, &mut failures, &row, report);
        }
        state
    } else {
        let mut state = RunState {
            epoch: next_epoch(previous.as_ref()),
            ..RunState::default()
        };
        for (index, a) in task
            .artifacts
            .iter()
            .enumerate()
            .filter(|(_, a)| a.run == artifact.run && a.title == TITLE)
        {
            let row = corral_core::store::codec::ordinal_key(index + 1);
            let report = ingested(ctx, &task.id, &row, a)?;
            ctx.store
                .save_record(REPORTS, &report_key(&task.id, &row), &report)?;
            fold(&mut state, &mut failures, &row, &report);
        }
        state
    };
    for (head, selected) in failures {
        let name = failure_key(&task.id, &artifact.run, &head)?;
        let old: Option<Selection> = ctx.store.record(FAILURES, &name)?;
        if old.is_none_or(|old| old.epoch != state.epoch || old.row <= selected.row) {
            ctx.store.save_record(FAILURES, &name, &selected)?;
        }
    }
    state.revision = after;
    ctx.store.save_record(RUNS, &name, &state)
}

fn stage(
    store: &Store,
    writes: &mut Vec<RecordWrite>,
    area: &str,
    key: String,
    value: &impl Serialize,
) -> Result<()> {
    let before = store.record_row(area, &key)?;
    let doc = serde_json::to_value(value)?;
    if before.as_ref().is_some_and(|row| row.doc == doc) {
        return Ok(());
    }
    writes.push(RecordWrite {
        area: area.to_owned(),
        key,
        expected: before.map(|row| row.version),
        doc: Some(doc),
        updated: corral_core::clock::now(),
    });
    Ok(())
}

fn backfill(ctx: &Ctx, head: &TaskHead, run: &str, revision: &str) -> Result<PreparedRun> {
    // Publishers hold this same lock before opening their task transaction. No publication
    // can race checkpoint seeding; generic task writes cannot make an old revision current.
    let _lock = crate::artifact_storage::storage_lock(ctx)?;
    let writer = ctx.store.reopen_unobserved()?;
    let generation = key(&(&head.id, run, revision, head.rev))?;
    if let Some(state) = writer.record(GENERATIONS, &generation)? {
        return Ok(PreparedRun {
            state,
            generation: Some((writer, generation)),
        });
    }
    let live: Option<RunState> = writer.record(RUNS, &run_key(&head.id, run)?)?;
    let mut state = RunState {
        revision: revision.to_owned(),
        epoch: next_epoch(live.as_ref()),
        issue: None,
    };
    let mut failures = BTreeMap::new();
    let mut reports = Vec::new();
    let query = RowQuery::part(TaskPart::Artifact).with_index(format!("run-title:{run}:{TITLE}"));
    let view = ctx.store.view()?;
    let mut page = PageReq::first(256);
    loop {
        let found = view.rows(head.num, &query, page.clone())?;
        for row in found.items {
            corral_core::work::decoded(
                corral_core::work::Counter::HistoryRowDecodes,
                u64::try_from(serde_json::to_vec(&row.doc)?.len())?,
                corral_core::work::Subject::Plain,
            );
            let artifact: Artifact = serde_json::from_value(row.doc)?;
            let report = ingested(ctx, &head.id, &row.key, &artifact)?;
            fold(&mut state, &mut failures, &row.key, &report);
            reports.push((row.key, report));
        }
        match found.next {
            Some(after) => page.after = Some(after),
            None => break,
        }
    }
    let current = writer.head(&head.id)?;
    let seed_current = current.rev == head.rev
        && current.current_run.as_ref().is_some_and(|r| r.id == run)
        && current.summary.current_artifacts.accepted_proof.revision == revision;
    let mut writes = Vec::new();
    for (candidate, selected) in failures {
        stage(
            &writer,
            &mut writes,
            GENERATION_FAILURES,
            generation_failure(&generation, &candidate)?,
            &selected,
        )?;
        if seed_current {
            stage(
                &writer,
                &mut writes,
                FAILURES,
                failure_key(&head.id, run, &candidate)?,
                &selected,
            )?;
        }
    }
    stage(
        &writer,
        &mut writes,
        GENERATIONS,
        generation.clone(),
        &state,
    )?;
    if seed_current {
        for (row, report) in reports {
            stage(
                &writer,
                &mut writes,
                REPORTS,
                report_key(&head.id, &row),
                &report,
            )?;
        }
        stage(&writer, &mut writes, RUNS, run_key(&head.id, run)?, &state)?;
    }
    // The immutable generation, its pointers and the live checkpoint commit together.
    writer.put_records(writes)?;
    Ok(PreparedRun {
        state,
        generation: Some((writer, generation)),
    })
}

/// Normal preparation reads one constant-size checkpoint per awaiting run, without opening
/// report history or report bytes. Only a missing/stale legacy checkpoint enters recovery.
pub(crate) fn prepare(ctx: &Ctx) -> Result<()> {
    ctx.store.pass_derived(PREPARED, || {
        let mut prepared = Prepared::new();
        for head in super::super::heads::inputs(ctx)?.iter() {
            let task = super::super::heads::shell(head)?;
            if !task.awaiting_owner() || matches!(task.status, corral_core::Status::Done) {
                continue;
            }
            if let Some(run) = &head.current_run {
                let revision = &head.summary.current_artifacts.accepted_proof.revision;
                let state: Option<RunState> =
                    ctx.store.record(RUNS, &run_key(&head.id, &run.id)?)?;
                let checked = match state {
                    Some(state) if &state.revision == revision => PreparedRun {
                        state,
                        generation: None,
                    },
                    _ => backfill(ctx, head, &run.id, revision)?,
                };
                prepared.insert((head.id.clone(), run.id.clone()), checked);
            }
        }
        Ok(prepared)
    })?;
    Ok(())
}

fn prepared(ctx: &Ctx) -> Result<Arc<Prepared>> {
    ctx.store.pass_derived(PREPARED, || {
        bail!("PM load checks were not prepared before the hot evaluation")
    })
}

pub(super) fn load(ctx: &Ctx, task: &str, run: &str, head: &str) -> Result<Option<PmObservation>> {
    let prepared = prepared(ctx)?;
    let checked = prepared
        .get(&(task.to_owned(), run.to_owned()))
        .context("PM load check run was not prepared")?;
    let selected: Option<Selection> = match &checked.generation {
        Some((writer, generation)) => {
            writer.record(GENERATION_FAILURES, &generation_failure(generation, head)?)?
        }
        None => ctx
            .store
            .record::<Selection>(FAILURES, &failure_key(task, run, head)?)?
            .filter(|selected| selected.epoch == checked.state.epoch),
    };
    if let Some(issue) = &checked.state.issue
        && selected
            .as_ref()
            .is_none_or(|selected| issue.row >= selected.row)
    {
        bail!("{}", issue.message);
    }
    let Some(selected) = selected else {
        return Ok(None);
    };
    let artifact = ctx
        .store
        .artifact_row(task, &selected.row)?
        .context("selected PM load report is missing")?;
    let check = read_check(ctx, &artifact)?;
    if !matches(&artifact, &check, task, run, head) {
        bail!(
            "selected PM load report {} no longer matches its selection",
            artifact.path
        );
    }
    let path = ctx.store.dir().join(&artifact.path);
    Ok(Some(PmObservation::LoadFailure {
        task: task.to_owned(),
        command: check.command,
        alone_passed: check.alone_exit.map(|exit| exit == 0),
        evidence: format!(
            "{}: load exit {}; alone exit {:?}; log {}",
            path.display(),
            check.under_load_exit,
            check.alone_exit,
            check.log
        ),
    }))
}
