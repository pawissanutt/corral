//! The db-core conformance suite against SqliteBackend, plus the SQLite-only acceptance checks
//! of plan T-253 sqlite-backend: concurrent first opens, cross-handle conflicts, backups while a
//! writer commits, filesystem refusal, the write-transaction guard and WAL release after a view
//! held by another process is dropped.

mod backend {
    pub use corral_core::backend::*;
}
mod blob_ref {
    pub use corral_core::blob_ref::*;
}
#[path = "sqlite_support/artifact_facts.rs"]
mod artifact_facts;
mod sqlite_support;

use std::io::Write;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result, anyhow, bail, ensure};
use corral_core::backend::conformance::{cases, dump, run_case, snapshot};
use corral_core::backend::*;
use corral_core::blob_ref::BlobHash;
use corral_core::sqlite::{
    BACKEND_TABLES, Barrier, SECTION_11, SqliteBackend, SqliteOptions, guard, statfs_probe,
};
use rusqlite::Connection;
use serde_json::json;
use sqlite_support::{
    CHILD_TIMEOUT, ChildLines, base_task, change, child_command, child_role, event, fixture_at,
    gate_attempt, hear, marked, record, row, sqlite_fixture, tell,
};
use tempfile::TempDir;

macro_rules! conformance {
    ($($name:ident),* $(,)?) => {
        const RUN_HERE: &[&str] = &[$(stringify!($name)),*];
        $(
            #[test]
            fn $name() -> Result<()> {
                run_case(stringify!($name), &sqlite_fixture)
            }
        )*
    };
}

conformance!(
    atomicity_under_injected_failures,
    stale_preconditions_conflict,
    claim_race_has_one_winner,
    change_feed_one_entry_per_commit,
    change_feed_gap_after_pruning,
    head_form_keeps_history,
    authorization_transitions,
    task_operation_preconditions,
    review_completion,
    documents_keep_their_blobs,
    blob_transitions,
    blob_store_contract,
    scratch_transitions,
    lifecycle_and_leases,
    conditions_claims_and_binding,
    task_change_commits_its_conditions,
    view_isolation,
    view_expiry,
    head_queries,
    row_queries,
    head_form_rows,
    head_documents_and_columns,
    lesson_artifact_projection,
    lesson_approval_projection,
    question_event_outbox,
);

#[test]
fn every_conformance_case_runs_against_sqlite() -> Result<()> {
    let names: Vec<&str> = cases().iter().map(|case| case.name).collect();
    let mut here = RUN_HERE.to_vec();
    here.sort_unstable();
    let mut all = names.clone();
    all.sort_unstable();
    ensure!(all == here, "cases {names:?} vs tests {RUN_HERE:?}");
    Ok(())
}

const ENTRY: &str = "zz_child_process_role";

/// The entry child processes run; a no-op in a normal test run.
#[test]
fn zz_child_process_role() -> Result<()> {
    let Some((role, dir)) = child_role() else {
        return Ok(());
    };
    match role.as_str() {
        "open" => {
            ensure!(hear()? == "go", "open child: no go signal");
            let backend = SqliteBackend::open(&dir, SqliteOptions::default())?;
            let p = backend.pragmas()?;
            tell(&format!(
                "OPENED {} {} {} {}",
                p.journal_mode, p.synchronous, p.foreign_keys, p.schema_version
            ))
        }
        "open-file" => {
            ensure!(hear()? == "go", "open-file child: no go signal");
            let backend = SqliteBackend::open_file(&dir, SqliteOptions::default())?;
            let p = backend.pragmas()?;
            tell(&format!(
                "OPENED {} {} {} {}",
                p.journal_mode, p.synchronous, p.foreign_keys, p.schema_version
            ))
        }
        "view" => {
            let backend = SqliteBackend::open(&dir, SqliteOptions::default())?;
            let view = backend.view()?;
            tell(&format!("VIEW {} {}", view.seq().0, digest(view.as_ref())?))?;
            ensure!(hear()? == "check", "view child: expected check");
            tell(&format!(
                "CHECK {} {}",
                view.seq().0,
                digest(view.as_ref())?
            ))?;
            ensure!(hear()? == "drop", "view child: expected drop");
            drop(view);
            tell("DROPPED")?;
            ensure!(hear()? == "exit", "view child: expected exit");
            Ok(())
        }
        other => bail!("unknown child role {other}"),
    }
}

fn digest(view: &dyn ReadView) -> Result<String> {
    Ok(BlobHash::of(format!("{:?}", dump(view)?).as_bytes()).to_string())
}

fn schema_of(conn: &Connection) -> Result<Vec<(String, String, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn expected_schema() -> Result<Vec<(String, String, Option<String>)>> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SECTION_11)?;
    conn.execute_batch(BACKEND_TABLES)?;
    conn.execute_batch(corral_core::sqlite::ARTIFACT_REVIEW_TABLES)?;
    conn.execute_batch(corral_core::sqlite::LESSON_APPROVAL_TABLES)?;
    conn.execute_batch(corral_core::sqlite::LESSON_ARTIFACT_TABLES)?;
    schema_of(&conn)
}

#[test]
fn section_11_executes_on_an_empty_database() -> Result<()> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    conn.execute_batch(SECTION_11)?;
    let column = |table: &str, name: &str| -> Result<(String, bool)> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let cols = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        cols.into_iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, ty, notnull)| (ty, notnull != 0))
            .ok_or_else(|| anyhow!("{table}.{name} is not declared"))
    };
    ensure!(
        column("tasks", "dashboard_due")? == ("TEXT".to_owned(), false),
        "tasks.dashboard_due is nullable TEXT"
    );
    ensure!(
        column("gate_attempts", "attempt")? == ("TEXT".to_owned(), true),
        "gate attempt keys are TEXT (F3)"
    );
    let index: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE name = 'tasks_dashboard_due'",
        [],
        |row| row.get(0),
    )?;
    ensure!(index.contains("WHERE dashboard_due IS NOT NULL"), "{index}");
    let tables: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
        [],
        |row| row.get(0),
    )?;
    let strict: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_list WHERE schema = 'main' AND type = 'table' \
         AND strict = 1",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        tables == 44 && strict == 44,
        "{tables} tables, {strict} strict"
    );
    println!("section 11: {tables} STRICT tables; dashboard_due nullable; attempt TEXT");
    Ok(())
}

fn spawn_opener(corral: &Path) -> Result<Child> {
    child_command(ENTRY, "open", corral)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn an opener")
}

#[test]
fn eight_processes_open_a_fresh_database_ten_times() -> Result<()> {
    let expected = expected_schema()?;
    let mut opened = 0;
    for trial in 0..10 {
        let dir = TempDir::new()?;
        let corral = dir.path().join(".corral");
        let mut children = (0..8)
            .map(|_| spawn_opener(&corral))
            .collect::<Result<Vec<_>>>()?;
        for child in &mut children {
            let stdin = child.stdin.as_mut().context("child stdin")?;
            writeln!(stdin, "go")?;
        }
        for child in children {
            let output = child.wait_with_output()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            ensure!(
                output.status.success(),
                "trial {trial}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let line = stdout
                .lines()
                .find_map(marked)
                .ok_or_else(|| anyhow!("trial {trial}: no report in {stdout}"))?;
            ensure!(line == "OPENED wal 1 true 1", "trial {trial}: {line}");
            opened += 1;
        }
        let path = SqliteBackend::db_path(&corral);
        ensure!(path.ends_with(".corral/db/corral.db") && path.is_file());
        let conn = Connection::open(&path)?;
        ensure!(
            schema_of(&conn)? == expected,
            "trial {trial}: schema differs"
        );
        let meta: Vec<(String, String)> = conn
            .prepare("SELECT key, value FROM meta ORDER BY key")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let value = |key: &str| meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        ensure!(
            value("schema_version") == Some("1")
                && value("origin") == Some("init")
                && value("change_seq") == Some("0")
                && value("workspace_id").is_some_and(|id| id.len() == 32),
            "trial {trial}: meta {meta:?}"
        );
    }
    ensure!(opened == 80, "{opened}/80 opens");
    println!("{opened}/80 opens: WAL, synchronous NORMAL (1), foreign keys on, schema_version 1");
    Ok(())
}

/// Review F1: a fresh relative corral directory or database path, resolved against the working
/// directory, opens like an absolute one. Each case runs in a child whose working directory is
/// a new empty directory.
#[test]
fn fresh_relative_paths_open_against_the_working_directory() -> Result<()> {
    let cases = [
        ("open", ".corral", ".corral/db/corral.db"),
        ("open", "new/.corral", "new/.corral/db/corral.db"),
        ("open-file", "corral.db", "corral.db"),
        ("open-file", "sub/dir/x.db", "sub/dir/x.db"),
    ];
    for (role, target, created) in cases {
        let cwd = TempDir::new()?;
        let mut child = child_command(ENTRY, role, Path::new(target))?
            .current_dir(cwd.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        writeln!(child.stdin.as_mut().context("child stdin")?, "go")?;
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        ensure!(
            output.status.success(),
            "{role} {target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let line = stdout.lines().find_map(marked);
        ensure!(
            line == Some("OPENED wal 1 true 1"),
            "{role} {target}: {line:?}"
        );
        ensure!(
            cwd.path().join(created).is_file(),
            "{role} {target}: {created} was not created"
        );
    }
    Ok(())
}

#[test]
fn a_stale_handle_conflicts_and_applies_nothing() -> Result<()> {
    let fx = sqlite_fixture();
    let num = base_task(&fx)?;
    let a = &fx.backend;
    let b = &fx.second;
    let read = a.view()?.head(num)?.context("head")?;
    let r = read.header.rev;
    let mut theirs = change(&fx, num)?;
    theirs.fields.title = "B's edit".to_owned();
    let committed = b.apply(Operation::UpdateTask(UpdateTask {
        change: theirs,
        form: UpdateForm::Head,
    }))?;
    ensure!(committed.outcome == Outcome::Task { num, rev: r + 1 });
    let after_b = snapshot(a.as_ref())?;
    let mine = TaskChange {
        num,
        expected_rev: r,
        fields: TaskFields {
            title: "A's edit".to_owned(),
            ..read.header.fields.clone()
        },
        doc: json!({"body": "A"}),
        summary: read.summary.clone(),
        rows: vec![RowWrite::Put(row(
            TaskPart::Run,
            "R2",
            true,
            true,
            json!({}),
        ))],
        events: vec![event(&fx, Some("T-001"), "edited", json!({"by": "A"}))],
        records: vec![record("pm", "a", None, Some(json!({})), fx.clock.now())],
        aggregates: vec![AggregateWrite::PaneOwner {
            socket: "a".to_owned(),
            delta: 1,
        }],
        reservations: Vec::new(),
        conditions: Vec::new(),
    };
    match a.apply(Operation::UpdateTask(UpdateTask {
        change: mine,
        form: UpdateForm::Head,
    })) {
        Err(ApplyError::Conflict(conflict)) => {
            ensure!(conflict.expected == r.to_string() && conflict.found == (r + 1).to_string());
        }
        other => bail!("A applied with a stale revision: {other:?}"),
    }
    ensure!(
        snapshot(a.as_ref())? == after_b,
        "A's stale write left something behind"
    );
    Ok(())
}

#[test]
fn backups_taken_while_a_writer_commits_are_consistent() -> Result<()> {
    let dir = TempDir::new()?;
    let corral = dir.path().join(".corral");
    let paused = Arc::new(AtomicBool::new(false));
    let (at_commit, reached) = sync_channel::<()>(0);
    let (resume, resumed) = sync_channel::<()>(0);
    let resumed = Mutex::new(resumed);
    let barrier: Barrier = {
        let paused = Arc::clone(&paused);
        let at_commit = Mutex::new(at_commit);
        Arc::new(move |point| {
            if point.what == "commit" && paused.swap(false, Ordering::SeqCst) {
                let signalled = at_commit
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send(());
                if signalled.is_ok() {
                    let _released = resumed
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .recv();
                }
            }
        })
    };
    let fx = fixture_at(&corral, None, Some(barrier))?;
    let num = base_task(&fx)?;
    let copies = std::thread::scope(|scope| -> Result<Vec<(std::path::PathBuf, u64)>> {
        let (reached, resume) = (reached, resume);
        let backend = fx.backend.as_ref();
        let clock = fx.clock.clone();
        let writer = scope.spawn(move || -> Result<()> {
            for i in 0..40 {
                if i % 10 == 5 {
                    paused.store(true, Ordering::SeqCst);
                }
                let head = backend.view()?.head(num)?.context("head")?;
                let edit = TaskChange {
                    num,
                    expected_rev: head.header.rev,
                    fields: TaskFields {
                        title: format!("edit {i}"),
                        ..head.header.fields
                    },
                    doc: head.doc,
                    summary: head.summary,
                    rows: Vec::new(),
                    events: vec![NewEvent {
                        ts: clock.now(),
                        task: Some("T-001".to_owned()),
                        kind: "edited".to_owned(),
                        actor: None,
                        via: None,
                        run: None,
                        detail: json!({"i": i}),
                    }],
                    records: Vec::new(),
                    aggregates: Vec::new(),
                    reservations: Vec::new(),
                    conditions: Vec::new(),
                };
                backend
                    .apply(Operation::UpdateTask(UpdateTask {
                        change: edit,
                        form: UpdateForm::Head,
                    }))
                    .map_err(|e| anyhow!("edit {i}: {e}"))?;
            }
            Ok(())
        });
        let mut copies = Vec::new();
        for trial in 0..4 {
            reached
                .recv_timeout(CHILD_TIMEOUT)
                .context("writer paused inside its transaction")?;
            let committed = fx.backend.view()?.seq().0;
            let dest = dir.path().join(format!("backup-{trial}.db"));
            fx.backend.backup_database(&dest)?;
            resume.send(()).context("resume the writer")?;
            copies.push((dest, committed));
        }
        writer.join().map_err(|_| anyhow!("writer panicked"))??;
        Ok(copies)
    })?;
    for (dest, committed) in copies {
        let copy = SqliteBackend::open_file(&dest, SqliteOptions::default())?;
        let report = copy.check()?;
        ensure!(report.is_ok(), "{}: {:?}", dest.display(), report.problems);
        let view = copy.view()?;
        ensure!(
            view.seq().0 == committed,
            "{}: copy at seq {} but {committed} operations had committed",
            dest.display(),
            view.seq().0
        );
        let head = view.head(num)?.context("task in the copy")?;
        let edits = conformance_events(view.as_ref(), "edited")?;
        ensure!(
            head.header.rev == 1 + edits,
            "{}: revision {} with {edits} edit events",
            dest.display(),
            head.header.rev
        );
        let changes = view.changes(ChangeSeq(0), 1000)?;
        let seqs: Vec<u64> = changes.entries.iter().map(|e| e.seq.0).collect();
        ensure!(
            seqs == (1..=committed).collect::<Vec<_>>(),
            "{}: change feed {seqs:?}",
            dest.display()
        );
        println!(
            "{}: integrity ok, seq {committed}, rev {} = 1 + {edits} edits",
            dest.display(),
            head.header.rev
        );
    }
    Ok(())
}

fn conformance_events(view: &dyn ReadView, kind: &str) -> Result<u64> {
    let events = corral_core::backend::conformance::all_pages(|page| {
        view.events(
            &EventQuery {
                kinds: vec![kind.to_owned()],
                ..EventQuery::default()
            },
            page,
        )
    })?;
    Ok(events.len() as u64)
}

macro_rules! probes {
    ($($fn_name:ident => $fs:literal),* $(,)?) => {
        $(fn $fn_name(_: &Path) -> Result<String> { Ok($fs.to_owned()) })*
        const PROBES: &[(&str, corral_core::sqlite::FilesystemProbe)] = &[$(($fs, $fn_name)),*];
    };
}

probes!(
    probe_9p => "9p",
    probe_v9fs => "v9fs",
    probe_drvfs => "drvfs",
    probe_nfs => "nfs",
    probe_smb => "smb3",
    probe_cifs => "cifs",
    probe_fuse => "fuse",
    probe_sshfs => "fuse.sshfs",
);

#[test]
fn network_and_fuse_filesystems_are_refused_before_any_file_exists() -> Result<()> {
    for (name, probe) in PROBES {
        let dir = TempDir::new()?;
        let corral = dir.path().join("work").join(".corral");
        let options = SqliteOptions {
            filesystem: *probe,
            ..SqliteOptions::default()
        };
        let error = match SqliteBackend::open(&corral, options) {
            Ok(_) => bail!("{name}: opened"),
            Err(error) => format!("{error:#}"),
        };
        ensure!(
            error.contains(&format!("on a {name} filesystem"))
                && error.contains("the workspace must live on a local filesystem"),
            "{name}: {error}"
        );
        ensure!(
            std::fs::read_dir(dir.path())?.next().is_none(),
            "{name}: a file was created"
        );
    }
    let local = TempDir::new()?;
    let kind = statfs_probe(local.path())?;
    ensure!(
        !corral_core::sqlite::is_refused_filesystem(&kind),
        "the test directory is on {kind}"
    );
    SqliteBackend::open(&local.path().join(".corral"), SqliteOptions::default())?;
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn an_external_call_inside_a_write_transaction_panics() -> Result<()> {
    let dir = TempDir::new()?;
    let barrier: Barrier = Arc::new(|point| {
        if point.what == "event" {
            guard::external_call("git worktree add");
        }
        if point.what == "committed" {
            guard::external_call("herdr pane run");
        }
    });
    let fx = fixture_at(&dir.path().join(".corral"), None, Some(barrier))?;
    let before = snapshot(fx.backend.as_ref())?;
    let op = Operation::AppendEvents(AppendEvents {
        events: vec![event(&fx, None, "note", json!({}))],
        reservations: Vec::new(),
    });
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.backend.apply(op)));
    let message = match outcome {
        Ok(result) => bail!("no panic: {result:?}"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default(),
    };
    ensure!(
        message.contains("`git worktree add`") && message.contains("write transaction"),
        "panic message {message:?}"
    );
    ensure!(
        !guard::in_write_transaction(),
        "the flag outlived the transaction"
    );
    ensure!(
        snapshot(fx.backend.as_ref())? == before,
        "the interrupted transaction left state behind"
    );
    let claim = Operation::Claim(Claim {
        name: "after".to_owned(),
        owner: "t".to_owned(),
        acquired: fx.clock.now(),
        doc: json!({}),
        reservations: Vec::new(),
    });
    fx.backend
        .apply(claim)
        .map_err(|e| anyhow!("apply after the panic: {e}"))?;
    Ok(())
}

#[test]
fn a_view_in_another_process_blocks_no_writer_and_its_drop_lets_the_wal_truncate() -> Result<()> {
    let dir = TempDir::new()?;
    let corral = dir.path().join(".corral");
    let fx = fixture_at(&corral, None, None)?;
    base_task(&fx)?;
    let mut child = child_command(ENTRY, "view", &corral)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut stdin = child.stdin.take().context("stdin")?;
    let lines = ChildLines::new(child.stdout.take().context("stdout")?);
    let opened = lines.wait_for("VIEW")?;
    let pinned = opened.trim_start_matches("VIEW ").to_owned();
    let waits = SqliteBackend::busy_waits();
    for n in 0..1000 {
        fx.backend
            .apply(Operation::AppendEvents(AppendEvents {
                events: vec![event(&fx, None, "note", json!({"n": n}))],
                reservations: Vec::new(),
            }))
            .map_err(|e| anyhow!("commit {n}: {e}"))?;
    }
    ensure!(
        SqliteBackend::busy_waits() == waits,
        "a writer waited while the view was open"
    );
    let wal = corral.join("db").join("corral.db-wal");
    let wal_before = std::fs::metadata(&wal)?.len();
    ensure!(wal_before > 0, "the WAL is empty while pinned");
    writeln!(stdin, "check")?;
    let checked = lines.wait_for("CHECK")?;
    ensure!(
        checked.trim_start_matches("CHECK ") == pinned,
        "the view moved: {pinned} then {checked}"
    );
    let seq: u64 = pinned.split(' ').next().unwrap_or_default().parse()?;
    ensure!(fx.backend.view()?.seq().0 == seq + 1000, "1,000 commits");
    writeln!(stdin, "drop")?;
    lines.wait_for("DROPPED")?;
    let checkpoint =
        SqliteBackend::open(&corral, SqliteOptions::default())?.checkpoint_truncate()?;
    ensure!(
        !checkpoint.busy && checkpoint.wal_frames == checkpoint.checkpointed,
        "checkpoint {checkpoint:?}"
    );
    let wal_after = std::fs::metadata(&wal)?.len();
    ensure!(wal_after == 0, "WAL holds {wal_after} bytes after TRUNCATE");
    writeln!(stdin, "exit")?;
    ensure!(child.wait()?.success(), "the view child failed");
    println!(
        "view pinned at {pinned}; 1000 commits with 0 busy waits; WAL {wal_before} -> {wal_after} bytes"
    );
    Ok(())
}

#[test]
fn gate_attempt_keys_stay_text() -> Result<()> {
    let fx = sqlite_fixture();
    let num = base_task(&fx)?;
    let identity = BlobHash::of(b"id");
    let commands = BlobHash::of(b"cmds");
    for (token, hash) in [("t1", &identity), ("t2", &commands)] {
        fx.backend
            .apply(Operation::Blob(BlobOp::Reserve {
                hash: hash.clone(),
                raw_bytes: 2,
                stored_bytes: 2,
                codec: "identity".to_owned(),
                token: token.to_owned(),
                owner: "t".to_owned(),
                ttl: std::time::Duration::from_secs(3600),
            }))
            .map_err(|e| anyhow!("{e}"))?;
    }
    let mut first = true;
    for attempt in ["ag-7f3a9c", "12", "0012"] {
        let mut gate = gate_attempt(&fx, &identity, &commands);
        gate.attempt = attempt.to_owned();
        let mut start = change(&fx, num)?;
        start.events = vec![event(
            &fx,
            Some("T-001"),
            "gate_started",
            json!({"attempt": attempt}),
        )];
        if first {
            start.reservations = vec![
                ReservationUse {
                    hash: identity.clone(),
                    token: "t1".to_owned(),
                },
                ReservationUse {
                    hash: commands.clone(),
                    token: "t2".to_owned(),
                },
            ];
            first = false;
        }
        fx.backend
            .apply(Operation::GateStart(GateStart {
                form: corral_core::backend::UpdateForm::Head,
                change: start,
                attempt: gate,
            }))
            .map_err(|e| anyhow!("{attempt}: {e}"))?;
    }
    let task = fx
        .backend
        .view()?
        .task(num, &TaskParts::Only(vec![TaskPart::GateAttempt]))?
        .context("task")?;
    let keys: Vec<(String, String)> = task
        .rows
        .iter()
        .map(|row| {
            let gate: GateAttempt = serde_json::from_value(row.doc.clone())?;
            Ok((row.key.clone(), gate.attempt))
        })
        .collect::<Result<_>>()?;
    ensure!(
        keys == vec![
            ("R1/0012".to_owned(), "0012".to_owned()),
            ("R1/12".to_owned(), "12".to_owned()),
            ("R1/ag-7f3a9c".to_owned(), "ag-7f3a9c".to_owned()),
        ],
        "attempt keys {keys:?}"
    );
    Ok(())
}
