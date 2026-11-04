//! The storage conformance suite against FakeBackend + FakeBlobStore, the SQL confinement
//! guard and the blob reference scanner (plan T-253 sections 1-3, 5 and 7-8).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::backend::conformance::{cases, fake_fixture, run_case};
use corral_core::backend::{GateAttempt, NewEvent, OpKind, StoredRecord};
use corral_core::blob_ref::{self, BlobHash, BlobRef};
use regex::Regex;
use serde_json::json;

macro_rules! conformance {
    ($($name:ident),* $(,)?) => {
        const RUN_HERE: &[&str] = &[$(stringify!($name)),*];
        $(
            #[test]
            fn $name() -> Result<()> {
                run_case(stringify!($name), &fake_fixture)
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
fn every_case_runs_here_and_every_operation_is_covered() -> Result<()> {
    let all = cases();
    let names: BTreeSet<&str> = all.iter().map(|case| case.name).collect();
    ensure!(
        names == RUN_HERE.iter().copied().collect(),
        "cases {names:?} vs tests {RUN_HERE:?}"
    );
    let covered: BTreeSet<OpKind> = all
        .iter()
        .flat_map(|case| case.covers.iter().copied())
        .collect();
    let missing: Vec<OpKind> = OpKind::ALL
        .into_iter()
        .filter(|kind| !covered.contains(kind))
        .collect();
    ensure!(
        missing.is_empty(),
        "operations without a conformance case: {missing:?}"
    );
    let atomicity = all
        .iter()
        .find(|case| case.name == "atomicity_under_injected_failures");
    ensure!(
        atomicity.is_some_and(|case| case.covers == OpKind::ALL),
        "atomicity covers every operation"
    );
    Ok(())
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Owner requirements 3-4: SQL and database mechanisms appear only under `corral-core/src/sqlite`
/// and the SQLite backend's own tests.
#[test]
fn sql_stays_inside_the_sqlite_module() -> Result<()> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let sqlite_owned = [
        "corral-core/src/sqlite/",
        "corral-core/tests/backend_conformance_sqlite.rs",
        "corral-core/tests/sqlite_stress.rs",
        "corral-core/tests/sqlite_support/",
        "corral-core/tests/backend_conformance_fake.rs",
    ];
    let mechanism = Regex::new(r"(?i)\b(rusqlite|data_version|rowid|json_extract|pragma)\b")?;
    let keyword = Regex::new(
        r"\b(SELECT\s+[\w*(]|INSERT\s+(OR\s+\w+\s+)?INTO|DELETE\s+FROM|UPDATE\s+\w+\s+SET|CREATE\s+(TABLE|(UNIQUE\s+)?INDEX|TRIGGER|VIEW)|DROP\s+(TABLE|INDEX)|ALTER\s+TABLE|BEGIN\s+(IMMEDIATE|DEFERRED|EXCLUSIVE)|COMMIT\s*;|VACUUM\b|ATTACH\s+DATABASE)",
    )?;
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates)? {
        let krate = entry?.path();
        for sub in ["src", "tests"] {
            if krate.join(sub).is_dir() {
                rust_files(&krate.join(sub), &mut files)?;
            }
        }
    }
    ensure!(
        files.iter().any(|f| f.ends_with("backend/fake/apply.rs")),
        "the scan reaches the backend sources"
    );
    let mut hits = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&crates)?
            .to_string_lossy()
            .replace('\\', "/");
        if sqlite_owned.iter().any(|owned| relative.starts_with(owned)) {
            continue;
        }
        let text = std::fs::read_to_string(file)?;
        for (n, line) in text.lines().enumerate() {
            if mechanism.is_match(line) || keyword.is_match(line) {
                hits.push(format!("{relative}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    ensure!(
        hits.is_empty(),
        "SQL outside crates/corral-core/src/sqlite:\n{}",
        hits.join("\n")
    );
    Ok(())
}

#[test]
fn blob_references_are_found_at_any_depth() -> Result<()> {
    let hashes: Vec<BlobHash> = (0..6)
        .map(|n| BlobHash::of(format!("evidence {n}").as_bytes()))
        .collect();
    let task_part = json!({"spec": {"acceptance": [{"evidence": [hashes[0].to_ref()]}]}});
    let run_doc = json!({"run": "R1", "review": {"children": [{"notes": hashes[1].to_ref()}]}});
    let event = NewEvent {
        ts: jiff::Timestamp::UNIX_EPOCH,
        task: Some("T-001".to_owned()),
        kind: "verdict".to_owned(),
        actor: None,
        via: None,
        run: None,
        detail: json!({"notes": {"blob": hashes[2].to_ref()}}),
    };
    let record = StoredRecord {
        area: "t251".to_owned(),
        key: "receipt".to_owned(),
        version: 1,
        doc: json!([[{"deep": [[hashes[3].to_ref()]]}]]),
        updated: jiff::Timestamp::UNIX_EPOCH,
    };
    let gate = GateAttempt {
        run: "R1".to_owned(),
        attempt: "ag-1".to_owned(),
        scope: "task".to_owned(),
        sha: "abc".to_owned(),
        tier: Some("quick".to_owned()),
        identity: hashes[4].to_ref(),
        commands: hashes[5].to_ref(),
        context: None,
        launched: None,
        result: None,
        failed_cmd: None,
        log: None,
        result_doc: None,
        started: jiff::Timestamp::UNIX_EPOCH,
        ended: None,
    };
    let expect = |doc: BTreeSet<BlobHash>, want: &[&BlobHash], what: &str| -> Result<()> {
        let want: BTreeSet<BlobHash> = want.iter().map(|h| (*h).clone()).collect();
        ensure!(doc == want, "{what}: found {doc:?}");
        Ok(())
    };
    expect(blob_ref::scan(&task_part), &[&hashes[0]], "task part")?;
    expect(blob_ref::scan(&run_doc), &[&hashes[1]], "run doc")?;
    expect(blob_ref::scan_doc(&event)?, &[&hashes[2]], "event detail")?;
    expect(blob_ref::scan_doc(&record)?, &[&hashes[3]], "record doc")?;
    expect(
        blob_ref::scan_doc(&gate)?,
        &[&hashes[4], &hashes[5]],
        "gate attempt row",
    )?;
    let serialized = serde_json::to_value(hashes[0].to_ref())?;
    ensure!(
        serialized == json!(format!("blob:sha256:{}", hashes[0])),
        "BlobRef serialization {serialized}"
    );
    let round: BlobRef = serde_json::from_value(serialized)?;
    ensure!(round == hashes[0].to_ref(), "BlobRef round trip");
    Ok(())
}

#[test]
fn a_bare_or_malformed_digest_is_not_a_reference() -> Result<()> {
    let hash = BlobHash::of(b"x");
    let hex = hash.to_string();
    let doc = json!({
        "bare": hex,
        "upper": format!("blob:sha256:{}", hex.to_uppercase()),
        "short": format!("blob:sha256:{}", &hex[..63]),
        "long": format!("blob:sha256:{hex}0"),
        "embedded": format!("see blob:sha256:{hex}"),
        "other_algo": format!("blob:sha1:{hex}"),
        "key_only": {format!("blob:sha256:{hex}"): 1},
    });
    ensure!(
        blob_ref::scan(&doc).is_empty(),
        "false references: {:?}",
        blob_ref::scan(&doc)
    );
    ensure!(
        format!("blob:sha256:{}", hex.to_uppercase())
            .parse::<BlobRef>()
            .is_err(),
        "uppercase parses"
    );
    ensure!(BlobHash::parse(&hex[..63]).is_err(), "short digest parses");
    Ok(())
}
