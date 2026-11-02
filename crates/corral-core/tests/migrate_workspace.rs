//! Post-rehearsal Pi, assistant and PM files must survive cutover with usable v2 records.
#[path = "migrate_support/mod.rs"]
mod support;
use corral_core::Store;
use std::fs;
use support::Legacy;

const EVIDENCE: &[&str] = &[
    "state/role-sessions/T-009/R1/nonce/session.jsonl",
    "state/role-sessions/T-009/R1-review1/nonce/session-artifacts/eval.log",
    "state/role-sessions/T-009/R1-review2/nonce/kernels/globals.json",
    "state/role-sessions/T-009/R1-review3/nonce/image.blob",
    "state/role-sessions/T-009/R1-review4/nonce/image.b64",
    "state/role-sessions/T-009/R1/nonce/session.index",
    "state/launch-exits/T-009-R1.log",
    "artifacts/assistant/A-1/report.md",
    "artifacts/assistant/A-1/qa/probe.ts",
    "artifacts/assistant/A-1/qa/screen.png",
    "artifacts/assistant/A-1/bin/corral-pi-host",
    "pm/handoff.md",
    "pm/cutover-held.txt",
    "pm/patches/repair.patch",
    "pm/cutover-2026-11-02/dryrun.json",
];
const TMP: &[&str] = &[
    "state/role-sessions/T-009/R1/nonce/session.lease",
    "state/launch-exits/T-009-R1.owner",
    "state/launch-exits/T-009-R1.exited",
];

#[test]
fn pi_and_workspace_evidence_is_classified_preserved_and_not_a_catch_all() {
    let legacy = Legacy::new();
    for path in EVIDENCE.iter().chain(TMP) {
        legacy.put(path, path.as_bytes());
    }
    let dry = legacy.migrate().dry_run().unwrap();
    assert!(dry.ok, "{dry:?}");
    for path in TMP {
        assert!(dry.tmp.iter().any(|p| p == path), "{path}");
    }
    let before = legacy.legacy_bytes();
    legacy.migrate().apply().unwrap();
    let backend =
        corral_core::sqlite::SqliteBackend::open(&legacy.corral, Default::default()).unwrap();
    let rows = backend.legacy_file_rows().unwrap();
    let blobs = corral_core::blob::FsBlobStore::open(&legacy.corral).unwrap();
    use corral_core::backend::BlobStore;
    for path in EVIDENCE {
        let row = rows.iter().find(|r| r.path == *path).unwrap();
        let mut bytes = Vec::new();
        use std::io::Read;
        blobs
            .get(row.blob.as_ref().unwrap())
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, path.as_bytes(), "{path}");
    }
    for path in TMP {
        assert!(
            rows.iter()
                .find(|r| r.path == *path)
                .unwrap()
                .blob
                .is_none()
        );
    }
    assert_eq!(legacy.legacy_bytes(), before);
    drop(backend);
    legacy.migrate().finalize().unwrap();
    // Arbitrary future state and malformed family owners must still refuse a new migration.
    let unknown = Legacy::new();
    for path in [
        "state/future/new.json",
        "state/role-sessions/not-task/R1/session.log",
        "state/role-sessions/T-009/not-run/session.log",
        "state/launch-exits/a.future",
        "state/agent-assistant/not-assistant.json",
        "assistants/not-assistant.md",
        "pm-inbox/not-item.md",
        "artifacts/assistant/not-assistant/report.md",
    ] {
        unknown.put(path, b"unknown");
    }
    assert_eq!(unknown.migrate().dry_run().unwrap().unclassified.len(), 8);
}

#[test]
fn runtime_copies_resume_and_never_overwrite_owner_files_or_follow_directory_links() {
    let rel = "state/role-sessions/T-001/R1/nonce/session.jsonl";
    let destination = "agent/sessions/legacy-role/T-001/R1/nonce/session.jsonl";
    let legacy = Legacy::new();
    legacy.put(rel, b"session");
    let (barrier, _) = support::kill_at("B3");
    support::killed(legacy.migrate().barrier(barrier).apply(), "B3");
    assert_eq!(
        fs::read(legacy.corral.join(destination)).unwrap(),
        b"session"
    );
    legacy.migrate().apply().unwrap();
    let conflict = Legacy::new();
    conflict.put(rel, b"session");
    conflict.put(destination, b"owner data");
    assert!(
        format!("{:#}", conflict.migrate().apply().unwrap_err()).contains("runtime copy differs")
    );
    assert_eq!(
        fs::read(conflict.corral.join(destination)).unwrap(),
        b"owner data"
    );
    assert_eq!(fs::read(conflict.corral.join(rel)).unwrap(), b"session");
    #[cfg(unix)]
    {
        let linked = Legacy::new();
        linked.put(rel, b"session");
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), linked.corral.join("agent")).unwrap();
        assert!(linked.migrate().apply().is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}

#[test]
fn role_session_paths_follow_retained_runtime_bytes_after_finalize() {
    let legacy = Legacy::new();
    let rel = "state/role-sessions/T-001/R1/nonce/session.jsonl";
    legacy.put(rel, b"{\"type\":\"session\",\"id\":\"fixture\"}\n");
    let path = legacy.corral.join("tasks/T-001-finished-work.md");
    let mut task = corral_core::parse_task_record(&fs::read_to_string(&path).unwrap(), &path)
        .unwrap()
        .task;
    task.runs[0].session = Some(
        serde_json::from_value(serde_json::json!({
            "policy":"reuse", "id":"fixture", "transcript":legacy.corral.join(rel),
            "directory":legacy.corral.join(rel).parent(), "harness":"pi", "action":"new"
        }))
        .unwrap(),
    );
    fs::write(&path, corral_core::render_task(&task)).unwrap();
    let before = legacy.legacy_bytes();
    legacy.migrate().apply().unwrap();
    assert_eq!(legacy.legacy_bytes(), before);
    legacy.migrate().finalize().unwrap();
    let store = Store::open_unobserved(&legacy.root).unwrap();
    let task = store.get("T-001").unwrap();
    let session = task.runs[0].session.as_ref().unwrap();
    let transcript = session.transcript.as_ref().unwrap();
    assert!(transcript.starts_with(corral_core::agent::paths::sessions_dir(&store)));
    assert_eq!(transcript.parent(), Some(session.directory.as_path()));
    assert_eq!(
        fs::read(transcript).unwrap(),
        b"{\"type\":\"session\",\"id\":\"fixture\"}\n"
    );
    assert!(!legacy.corral.join(rel).exists());
}

#[test]
fn assistant_and_pm_records_and_artifacts_are_usable_after_finalize() {
    let legacy = Legacy::new();
    let sha = corral_core::agent::paths::sha256_hex(b"snapshot");
    let artifact = serde_json::json!({"run":"", "kind":"report", "title":"Kept",
        "path":"artifacts/assistant/A-1/report.md", "source":"/tmp/report.md", "sha256":sha,
        "bytes":8,"added":"2026-10-30T00:00:00Z"});
    let session = serde_json::json!({"version":1,"id":"A-1","created":"2026-10-30T00:00:00Z",
        "ended":null,"focus":"workspace","profile":"pi", "artifacts":[artifact]});
    legacy.put(
        "assistants/A-1.md",
        format!(
            "---\n{}---\nAssistant body\n",
            serde_yaml_ng::to_string(&session).unwrap()
        )
        .as_bytes(),
    );
    legacy.put("artifacts/assistant/A-1/report.md", b"snapshot");
    // A complete container, just as assistant runtime saves it.
    let state = corral_core::agent::AgentRunState::new(corral_core::agent::AgentBinding {
        role: corral_core::agent::AgentRole::Assistant,
        store: "fixture".into(),
        project: "".into(),
        generation: 0,
        health: Default::default(),
        callback: None,
        closing: false,
    });
    let container = serde_json::json!({"version":1,"workspace":legacy.root,"state":state});
    legacy.put(
        "state/agent-assistant/A-1.json",
        &serde_json::to_vec(&container).unwrap(),
    );
    legacy.put("state/assistant-counter", b"7\n");
    legacy.put(
        "state/assistant-inbox/A-1.json",
        br#"{"inbox":{"next_seq":4}}"#,
    );
    legacy.put("state/agent-pm.json", br#"{"fixture":"pm"}"#);
    legacy.put("state/queue-refusals.json", b"{}");
    legacy.put("state/pm-inbox-counter", b"12\n");
    legacy.put("pm-inbox/PMI-1.md", b"---\nversion: 1\nid: PMI-1\nrevision: 0\nfrom: owner\ncreated: 2026-10-30T00:00:00Z\nkind: question\nsubject: Question\nstate: open\n---\nPM body\n");
    let dry = legacy.migrate().dry_run().unwrap();
    assert!(dry.ok, "{dry:?}");
    legacy.migrate().apply().unwrap();
    legacy.migrate().finalize().unwrap();
    let store = Store::open_unobserved(&legacy.root).unwrap();
    let assistant = store.get_assistant("A-1").unwrap();
    assert_eq!(assistant.body, "Assistant body\n");
    assert_eq!(assistant.artifacts[0].sha256, sha);
    assert_eq!(store.active_assistant_ids().unwrap(), ["A-1"]);
    assert_eq!(store.get_assistant_container("A-1").unwrap().state, state);
    assert_eq!(
        store.record::<u64>("assistant-counter", "current").unwrap(),
        Some(7)
    );
    assert_eq!(
        store.record::<u64>("pm-inbox-counter", "current").unwrap(),
        Some(12)
    );
    assert_eq!(
        store
            .record::<serde_json::Value>("agent-pm", "current")
            .unwrap()
            .unwrap()["fixture"],
        "pm"
    );
    assert!(
        store
            .record::<serde_json::Value>("assistant-inbox", "A-1")
            .unwrap()
            .is_some()
    );
    assert_eq!(store.pending_pm_inbox_items().unwrap()[0].body, "PM body\n");
    assert_eq!(store.owed_pm_inbox_items().unwrap()[0].id, "PMI-1");
    let blob = store
        .record::<serde_json::Value>("artifact-blobs", &format!("A-1/{sha}"))
        .unwrap()
        .unwrap();
    assert_eq!(blob["blob"], format!("blob:sha256:{sha}"));
    assert!(!legacy.corral.join("assistants/A-1.md").exists());
    assert!(
        !legacy
            .corral
            .join("state/agent-assistant/A-1.json")
            .exists()
    );
    drop(store);
    assert!(fs::metadata(legacy.corral.join("blobs")).unwrap().is_dir());
}
