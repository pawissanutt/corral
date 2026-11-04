//! The database schema (plan T-253 section 11) and its initialization.
//!
//! [`SECTION_11`] is the plan's section 11 block verbatim, with every table `STRICT` as the
//! plan's file comment requires and the gate-attempt key as TEXT (cross-plan correction F3), so
//! request-derived `ag-...` attempts are kept unchanged. [`BACKEND_TABLES`] adds what the
//! backend-neutral contract needs beyond it: the contract's history rows, stored as the facade
//! hands them over until store-tasks-db lays each family into its typed table (correction F6),
//! and the append-only triggers the section names.

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

#[cfg(test)]
#[path = "schema_index_tests.rs"]
mod index_tests;

/// The `meta.schema_version` this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

pub const SECTION_11: &str = r#"-- P8: every time column holds fixed-width UTC text (YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ, one encoder), so text order is time order; documents keep their own serialization
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;  -- schema_version, workspace_id, change_seq, origin(init|migrate), migration_digest
CREATE TABLE changes(seq INTEGER PRIMARY KEY, at TEXT NOT NULL, op TEXT NOT NULL) STRICT;
CREATE TABLE change_scopes(seq INTEGER NOT NULL REFERENCES changes(seq), kind TEXT NOT NULL, key TEXT NOT NULL, PRIMARY KEY(seq, kind, key)) STRICT;
CREATE INDEX change_scopes_kind ON change_scopes(kind, seq);
-- task head: header columns + spec/state/body documents + the history summary (§4)
CREATE TABLE tasks(num INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, rev INTEGER NOT NULL, title TEXT NOT NULL,
  status TEXT NOT NULL, project TEXT, kind TEXT, size TEXT, priority TEXT NOT NULL, parent TEXT, tracker INTEGER NOT NULL,
  design INTEGER NOT NULL, queued INTEGER NOT NULL, created TEXT NOT NULL, updated TEXT NOT NULL, spec_rev TEXT NOT NULL,
  spec TEXT NOT NULL, state TEXT NOT NULL, body TEXT NOT NULL, history TEXT NOT NULL, doc_version INTEGER NOT NULL,
  dashboard_due TEXT) STRICT;  -- P8: declared, nullable (see tasks_dashboard_due)
CREATE INDEX tasks_open ON tasks(status, project) WHERE status NOT IN ('done','dropped');
CREATE INDEX tasks_status_updated ON tasks(status, updated);
CREATE INDEX tasks_project_created ON tasks(project, created, id);  -- P5: capped new-task page of the operating policy
-- P7/P8: dashboard_due is the unpaused schedule's next_due (NULL when absent or paused), derived in the same UpdateTask and by migrate's import
CREATE INDEX tasks_dashboard_due ON tasks(dashboard_due) WHERE dashboard_due IS NOT NULL;
CREATE TABLE task_edges(task INTEGER NOT NULL REFERENCES tasks(num), kind TEXT NOT NULL, other TEXT NOT NULL, PRIMARY KEY(task, kind, other)) STRICT;
CREATE INDEX task_edges_other ON task_edges(other, kind);
-- history and current rows; `current = 1` marks the row a TaskHead carries
CREATE TABLE runs(task INTEGER NOT NULL REFERENCES tasks(num), seq INTEGER NOT NULL, run_id TEXT NOT NULL,
  family TEXT NOT NULL CHECK(family IN ('coding','planning')), current INTEGER NOT NULL, outcome TEXT, started TEXT NOT NULL,
  ended TEXT, worktree TEXT, branch TEXT, doc TEXT NOT NULL, doc_version INTEGER NOT NULL,
  facts TEXT NOT NULL,  -- P6: ContainerFacts (§4); doc excludes every nested history collection
  PRIMARY KEY(task, family, seq)) STRICT;
CREATE UNIQUE INDEX runs_id ON runs(task, family, run_id);
CREATE UNIQUE INDEX runs_current ON runs(task, family) WHERE current = 1;
-- P6 nested rows (§4). run_seq is a coding run's seq; child_seq 0 is the run itself, n > 0 its review child n.
-- Plan runs carry no nested history except their inbox, which is keyed by the recipient's owner key.
CREATE TABLE review_runs(task INTEGER NOT NULL, run_seq INTEGER NOT NULL, seq INTEGER NOT NULL, child_id TEXT NOT NULL,
  current INTEGER NOT NULL, ended TEXT, outcome TEXT, review_at TEXT, review_sha TEXT, stronger INTEGER NOT NULL DEFAULT 0,
  doc TEXT NOT NULL, facts TEXT NOT NULL, PRIMARY KEY(task, run_seq, seq)) STRICT;
CREATE UNIQUE INDEX review_runs_id ON review_runs(task, run_seq, child_id);
CREATE UNIQUE INDEX review_runs_current ON review_runs(task, run_seq) WHERE current = 1;
CREATE INDEX review_runs_open ON review_runs(task, run_seq, seq) WHERE ended IS NULL;
CREATE INDEX review_runs_at ON review_runs(task, run_seq, review_at);
CREATE INDEX review_runs_stronger ON review_runs(task, run_seq, review_sha) WHERE stronger = 1 AND outcome = 'superseded';
CREATE TABLE run_usage(task INTEGER NOT NULL, run_seq INTEGER NOT NULL, child_seq INTEGER NOT NULL, seq INTEGER NOT NULL, doc TEXT NOT NULL,
  PRIMARY KEY(task, run_seq, child_seq, seq)) STRICT;
-- P7: one CheckpointMeta row per checkpoint (constant size); the payload is the canonical Checkpoint document, stored once per digest
CREATE TABLE run_checkpoints(task INTEGER NOT NULL, run_seq INTEGER NOT NULL, child_seq INTEGER NOT NULL, seq INTEGER NOT NULL,
  artifact TEXT NOT NULL, revision INTEGER NOT NULL, captured TEXT NOT NULL, input_task TEXT NOT NULL, input_run TEXT NOT NULL,
  spec_revision TEXT NOT NULL, progress_revision INTEGER NOT NULL, head TEXT NOT NULL, base TEXT NOT NULL, tree TEXT NOT NULL,
  payload_digest TEXT NOT NULL REFERENCES checkpoint_payloads(digest), PRIMARY KEY(task, run_seq, child_seq, seq)) STRICT;
CREATE TABLE checkpoint_payloads(digest TEXT PRIMARY KEY, doc TEXT NOT NULL) STRICT;  -- refcounted by run_checkpoints rows; deleted with the last reference
CREATE INDEX run_checkpoints_payload ON run_checkpoints(payload_digest);  -- the UpdateTask that drops the last reference deletes the payload
CREATE INDEX run_checkpoints_artifact ON run_checkpoints(task, run_seq, artifact);
CREATE TABLE run_switches(task INTEGER NOT NULL, run_seq INTEGER NOT NULL, child_seq INTEGER NOT NULL, seq INTEGER NOT NULL, doc TEXT NOT NULL,
  PRIMARY KEY(task, run_seq, child_seq, seq)) STRICT;
CREATE TABLE run_obligations(task INTEGER NOT NULL, run_seq INTEGER NOT NULL, child_seq INTEGER NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('wait','decision')), seq INTEGER NOT NULL, request_id TEXT NOT NULL, receipt_id TEXT,
  open INTEGER NOT NULL, doc TEXT NOT NULL, PRIMARY KEY(task, run_seq, child_seq, kind, seq)) STRICT;
CREATE UNIQUE INDEX run_obligations_id ON run_obligations(task, run_seq, child_seq, request_id);
CREATE INDEX run_obligations_receipt ON run_obligations(task, run_seq, child_seq, receipt_id) WHERE receipt_id IS NOT NULL;
CREATE INDEX run_obligations_open ON run_obligations(task, run_seq, child_seq, seq) WHERE open = 1;
CREATE TABLE inbox_effects(task INTEGER NOT NULL, owner TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY(task, owner, seq)) STRICT;
CREATE TABLE observe_cursors(task INTEGER NOT NULL, owner TEXT NOT NULL, adapter TEXT NOT NULL, binding TEXT NOT NULL, doc TEXT NOT NULL,
  PRIMARY KEY(task, owner, adapter, binding)) STRICT;
CREATE TABLE plans(task INTEGER NOT NULL, seq INTEGER NOT NULL, plan_id TEXT NOT NULL, current INTEGER NOT NULL, snapshot_blob TEXT, doc TEXT NOT NULL,
  facts TEXT NOT NULL, PRIMARY KEY(task, seq)) STRICT;
CREATE UNIQUE INDEX plans_id ON plans(task, plan_id);  -- P5: keyed plan(id)
CREATE TABLE plan_reviews(task INTEGER NOT NULL, plan_seq INTEGER NOT NULL, seq INTEGER NOT NULL, review_id TEXT NOT NULL,
  current INTEGER NOT NULL, pending INTEGER NOT NULL, reviewer TEXT NOT NULL, verdict TEXT, doc TEXT NOT NULL, PRIMARY KEY(task, plan_seq, seq)) STRICT;
CREATE UNIQUE INDEX plan_reviews_id ON plan_reviews(task, plan_seq, review_id);
CREATE INDEX plan_reviews_pending ON plan_reviews(task, plan_seq, seq) WHERE pending = 1;
CREATE INDEX plan_reviews_by ON plan_reviews(task, plan_seq, reviewer, verdict);
CREATE TABLE aggregate_attempts(task INTEGER NOT NULL, seq INTEGER NOT NULL, current INTEGER NOT NULL, doc TEXT NOT NULL, facts TEXT NOT NULL,
  PRIMARY KEY(task, seq)) STRICT;
CREATE TABLE aggregate_reviews(task INTEGER NOT NULL, attempt_seq INTEGER NOT NULL, seq INTEGER NOT NULL, review_id TEXT NOT NULL,
  current INTEGER NOT NULL, pending INTEGER NOT NULL, doc TEXT NOT NULL, PRIMARY KEY(task, attempt_seq, seq)) STRICT;
CREATE UNIQUE INDEX aggregate_reviews_id ON aggregate_reviews(task, attempt_seq, review_id);
CREATE INDEX aggregate_reviews_pending ON aggregate_reviews(task, attempt_seq, seq) WHERE pending = 1;
-- P6: pane references, written in the operation that records the pane; released once cleanup confirms it closed or absent
CREATE TABLE pane_refs(task INTEGER NOT NULL, owner TEXT NOT NULL, kind TEXT NOT NULL, socket TEXT NOT NULL, pane TEXT NOT NULL,
  workspace TEXT, released INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(task, owner, kind, pane)) STRICT;
CREATE INDEX pane_refs_open ON pane_refs(task, owner) WHERE released = 0;
CREATE TABLE comments(task INTEGER NOT NULL REFERENCES tasks(num), seq INTEGER NOT NULL, doc TEXT NOT NULL, PRIMARY KEY(task, seq)) STRICT;
CREATE TABLE scope_revisions(task INTEGER NOT NULL, seq INTEGER NOT NULL, doc TEXT NOT NULL, PRIMARY KEY(task, seq)) STRICT;
CREATE TABLE recoveries(task INTEGER NOT NULL, id TEXT NOT NULL, intent TEXT NOT NULL, created TEXT NOT NULL, PRIMARY KEY(task, id)) STRICT;
CREATE TABLE recovery_outcomes(task INTEGER NOT NULL, recovery TEXT NOT NULL, seq INTEGER NOT NULL, doc TEXT NOT NULL, PRIMARY KEY(task, recovery, seq)) STRICT;
-- triggers reject UPDATE/DELETE on recoveries and recovery_outcomes (T-137 append-only)
CREATE TABLE events(seq INTEGER PRIMARY KEY, ts TEXT NOT NULL, task TEXT, kind TEXT NOT NULL, actor TEXT, via TEXT,
  run TEXT,  -- P7: detail.run copied by the Rust append and import paths, never by SQL JSON functions
  detail TEXT NOT NULL, malformed INTEGER NOT NULL DEFAULT 0) STRICT;
CREATE INDEX events_task ON events(task, seq);
CREATE INDEX events_kind ON events(kind, seq);
CREATE INDEX events_task_kind ON events(task, kind, seq);  -- P6: events_of(task, kinds, after)
CREATE INDEX events_task_kind_run ON events(task, kind, run, seq);  -- P7: events_of(task, kinds, run, after, limit)
-- §9: one row per condition run, durable after it closes; `conditions` only points at the open run
CREATE TABLE condition_runs(opened_seq INTEGER PRIMARY KEY REFERENCES events(seq), task TEXT NOT NULL, kind TEXT NOT NULL,
  detail_digest TEXT NOT NULL, count INTEGER NOT NULL, first_at TEXT NOT NULL, last_at TEXT NOT NULL,
  first_actor TEXT, first_via TEXT, last_actor TEXT, last_via TEXT,
  closed_seq INTEGER REFERENCES events(seq), close_reason TEXT CHECK(close_reason IN ('changed','cleared')),
  CHECK((closed_seq IS NULL) = (close_reason IS NULL))) STRICT;
CREATE INDEX condition_runs_task ON condition_runs(task, kind, opened_seq);
CREATE INDEX condition_runs_closed ON condition_runs(closed_seq);
CREATE TABLE conditions(task TEXT NOT NULL, kind TEXT NOT NULL, open_run INTEGER NOT NULL UNIQUE REFERENCES condition_runs(opened_seq), PRIMARY KEY(task, kind)) STRICT;
CREATE TABLE records(area TEXT NOT NULL, key TEXT NOT NULL, version INTEGER NOT NULL, doc TEXT NOT NULL, updated TEXT NOT NULL, PRIMARY KEY(area, key)) STRICT;
CREATE TABLE claims(name TEXT PRIMARY KEY, owner TEXT NOT NULL, acquired TEXT NOT NULL, doc TEXT NOT NULL) STRICT;
CREATE TABLE authorizations(id TEXT PRIMARY KEY, task TEXT NOT NULL, disposition TEXT NOT NULL, doc TEXT NOT NULL) STRICT;
CREATE TABLE artifacts(task INTEGER NOT NULL, seq INTEGER NOT NULL, run TEXT NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL,
  name TEXT NOT NULL, sha256 TEXT NOT NULL, bytes INTEGER NOT NULL, blob TEXT, tier_override TEXT, pinned INTEGER NOT NULL DEFAULT 0,
  added TEXT NOT NULL, added_seq INTEGER NOT NULL, purged_at TEXT, PRIMARY KEY(task, seq)) STRICT;
CREATE INDEX artifacts_run ON artifacts(task, run, seq);  -- P5: artifacts_of(run)
-- P5: derived from runs/review_runs and plans in the same UpdateTask; one row per non-approve review with notes (emitted_findings)
CREATE TABLE findings(task INTEGER NOT NULL REFERENCES tasks(num), source TEXT NOT NULL CHECK(source IN ('run','plan')),
  owner TEXT NOT NULL, attempt TEXT NOT NULL, seq INTEGER NOT NULL, doc TEXT NOT NULL, PRIMARY KEY(task, source, seq)) STRICT;
CREATE TABLE prompts(task INTEGER NOT NULL, run TEXT NOT NULL, blob TEXT NOT NULL, PRIMARY KEY(task, run)) STRICT;
CREATE TABLE gate_attempts(task INTEGER NOT NULL, run TEXT NOT NULL, attempt TEXT NOT NULL, scope TEXT NOT NULL, sha TEXT NOT NULL,
  tier TEXT, identity_blob TEXT NOT NULL, commands_blob TEXT NOT NULL, context_blob TEXT, result TEXT, failed_cmd TEXT,
  log_blob TEXT, result_blob TEXT, started TEXT NOT NULL, ended TEXT, PRIMARY KEY(task, run, attempt)) STRICT;
CREATE TABLE review_evidence(task INTEGER NOT NULL, subject TEXT NOT NULL, attempt TEXT NOT NULL, artifact_seq INTEGER NOT NULL,
  PRIMARY KEY(task, subject, attempt, artifact_seq)) STRICT;
CREATE TABLE blobs(hash TEXT PRIMARY KEY, raw_bytes INTEGER NOT NULL, stored_bytes INTEGER NOT NULL, codec TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('writing','live','deleting')), refs INTEGER NOT NULL, unreferenced_since TEXT, created TEXT NOT NULL) STRICT;
CREATE INDEX blobs_gc ON blobs(state, refs, unreferenced_since);
CREATE TABLE blob_refs(owner_kind TEXT NOT NULL, owner_key TEXT NOT NULL, blob TEXT NOT NULL REFERENCES blobs(hash), PRIMARY KEY(owner_kind, owner_key, blob)) STRICT;
CREATE INDEX blob_refs_blob ON blob_refs(blob);
CREATE TABLE reservations(hash TEXT NOT NULL, token TEXT NOT NULL, owner TEXT NOT NULL, expires TEXT NOT NULL, PRIMARY KEY(hash, token)) STRICT;
CREATE TABLE leases(id TEXT PRIMARY KEY, holder TEXT NOT NULL, started TEXT NOT NULL, expires TEXT NOT NULL) STRICT;
CREATE TABLE scratch_dirs(id TEXT PRIMARY KEY, kind TEXT NOT NULL, task TEXT, subject TEXT, attempt TEXT, path TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL, reason TEXT, next_check_at TEXT, owner TEXT, warned TEXT, evidence_bytes INTEGER NOT NULL DEFAULT 0,
  created TEXT NOT NULL, closed TEXT, reclaimed TEXT) STRICT;
CREATE INDEX scratch_due ON scratch_dirs(state, next_check_at);
CREATE TABLE lifecycle(task TEXT NOT NULL, generation TEXT NOT NULL, target TEXT NOT NULL, policy_digest TEXT NOT NULL,
  outcome TEXT NOT NULL CHECK(outcome IN ('reclaimed','retained','deferred','failed')), reason TEXT, bytes INTEGER,
  next_check_at TEXT, at TEXT NOT NULL, PRIMARY KEY(task, generation, target)) STRICT;
CREATE INDEX lifecycle_due ON lifecycle(outcome, next_check_at);
CREATE INDEX lifecycle_policy ON lifecycle(policy_digest);
CREATE TABLE status_counts(status TEXT NOT NULL, project TEXT NOT NULL, n INTEGER NOT NULL, PRIMARY KEY(status, project)) STRICT;
CREATE TABLE usage_totals(scope TEXT NOT NULL, key TEXT NOT NULL, doc TEXT NOT NULL, PRIMARY KEY(scope, key)) STRICT;
CREATE TABLE pane_owners(socket TEXT NOT NULL, n INTEGER NOT NULL, PRIMARY KEY(socket)) STRICT;  -- '' = legacy socket-less
CREATE TABLE legacy_files(path TEXT PRIMARY KEY, kind TEXT NOT NULL, bytes INTEGER NOT NULL, sha256 TEXT, blob TEXT, disposition TEXT NOT NULL) STRICT;
"#;

pub const BACKEND_TABLES: &str = r#"
CREATE INDEX IF NOT EXISTS tasks_project_updated ON tasks(project, updated DESC, id DESC);
-- The contract's history rows (backend::HistoryRow) keyed by family and key; `part` is the
-- family's snake_case name. store-tasks-db moves each family into its typed table above.
CREATE TABLE task_rows(task INTEGER NOT NULL REFERENCES tasks(num), part TEXT NOT NULL, key TEXT NOT NULL,
  current INTEGER NOT NULL, open INTEGER NOT NULL, doc TEXT NOT NULL, idx TEXT NOT NULL DEFAULT '[]',
  PRIMARY KEY(task, part, key)) STRICT;
CREATE INDEX task_rows_current ON task_rows(task, part, key) WHERE current = 1;
CREATE INDEX task_rows_open ON task_rows(task, part, key) WHERE open = 1;
-- Last active children across older containers: neither settled current rows nor replaced
-- pending children belong to this output-sized set (attention's pending plan reviewers).
CREATE INDEX task_rows_current_open ON task_rows(task, part, key) WHERE current = 1 AND open = 1;
CREATE INDEX IF NOT EXISTS task_rows_worktrees ON task_rows(
    current, task, key, json_extract(doc, '$.id'), json_extract(doc, '$.worktree')
) WHERE part = 'run';
-- Secondary lookup tokens of each row (keyed child reads by an indexed attribute).
CREATE TABLE task_row_index(task INTEGER NOT NULL, part TEXT NOT NULL, token TEXT NOT NULL, key TEXT NOT NULL,
  PRIMARY KEY(task, part, token, key)) STRICT;
-- Cross-task token lookups (the sync sweep's pane owners).
CREATE INDEX task_row_index_token ON task_row_index(part, token, task);
CREATE TRIGGER recoveries_no_update BEFORE UPDATE ON recoveries BEGIN SELECT RAISE(ABORT, 'recoveries are append-only'); END;
CREATE TRIGGER recoveries_no_delete BEFORE DELETE ON recoveries BEGIN SELECT RAISE(ABORT, 'recoveries are append-only'); END;
CREATE TRIGGER recovery_outcomes_no_update BEFORE UPDATE ON recovery_outcomes BEGIN SELECT RAISE(ABORT, 'recovery outcomes are append-only'); END;
CREATE TRIGGER recovery_outcomes_no_delete BEFORE DELETE ON recovery_outcomes BEGIN SELECT RAISE(ABORT, 'recovery outcomes are append-only'); END;
"#;

/// Creates the schema in a fresh database, or checks the version of an existing one. Safe to
/// run from many processes at once: the check and the creation share one write transaction.
pub(super) fn init(conn: &mut Connection, workspace_id: &str) -> Result<()> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin schema transaction")?;
    let found: Option<String> = tx
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("look up the meta table")?;
    if found.is_none() {
        tx.execute_batch(SECTION_11)
            .context("create the section 11 schema")?;
        tx.execute_batch(BACKEND_TABLES)
            .context("create the backend tables")?;
        let mut insert = tx.prepare("INSERT INTO meta(key, value) VALUES (?1, ?2)")?;
        let version = SCHEMA_VERSION.to_string();
        for (key, value) in [
            ("schema_version", version.as_str()),
            ("workspace_id", workspace_id),
            ("change_seq", "0"),
            ("feed_floor", "0"),
            ("origin", "init"),
        ] {
            insert
                .execute(params![key, value])
                .with_context(|| format!("record meta {key}"))?;
        }
        drop(insert);
    } else {
        let version: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("read schema_version")?;
        if version.as_deref() != Some(SCHEMA_VERSION.to_string().as_str()) {
            bail!(
                "unsupported corral database schema_version {version:?}; this build reads {SCHEMA_VERSION}"
            );
        }
    }
    // Additive lookup indexes apply to existing v1 databases as well as fresh ones.
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS tasks_project_updated ON tasks(project, updated DESC, id DESC);",
    )
    .context("create project updated index")?;
    tx.execute_batch(super::run_worktrees::INDEX)
        .context("create run worktree projection index")?;
    let backfilled: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'question_event_outbox_v1')",
        [],
        |row| row.get(0),
    )?;
    if !backfilled {
        super::tx::backfill_question_events(&tx)
            .map_err(anyhow::Error::new)
            .context("backfill question event outbox")?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('question_event_outbox_v1', '1')",
            [],
        )?;
    }
    tx.commit().context("commit schema transaction")
}
