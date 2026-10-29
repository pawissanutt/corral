#![allow(dead_code)]
//! A small legacy (schema 2) board on disk for the migration tests: task files, events,
//! artifacts, prompts, state records, legacy review files with a CACHEDIR.TAG build tree, and
//! the helpers the barrier tests assert with.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use corral_core::migrate::{Barrier, Migrate, directory_problems};
use sha2::{Digest, Sha256};

pub const FENCE: &str = "2\n";

pub struct Legacy {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub corral: PathBuf,
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// An artifact holding `content`, whose snapshot (when given) is `stored`: gzip bytes when
/// `compressed`.
pub struct NewArtifact<'a> {
    pub task_file: &'a str,
    pub kind: &'a str,
    pub file: &'a str,
    pub content: &'a [u8],
    pub stored: Option<&'a [u8]>,
    pub compressed: bool,
    pub purged: bool,
}

pub const REPORT: &[u8] = b"# Report\n\nThe work is done.\n";

pub fn done_task(id: &str, artifact_path: &str) -> String {
    format!(
        "---\nschema_version: 2\ntask:\n  id: {id}\n  title: Finished work {id}\n  status: done\n  priority: P2\n  created: 2026-10-01T00:00:00Z\n  updated: 2026-10-02T00:00:00Z\n  runs:\n  - id: R1\n    agent: claude\n    worktree: /tmp/corral-migrate-fixture/{id}\n    branch: corral/{id}\n    base: main\n    base_sha: '0000000000000000000000000000000000000000'\n    started: 2026-10-01T01:00:00Z\n    ended: 2026-10-01T02:00:00Z\n    outcome: done\n    herdr:\n      workspace_id: w1\n      pane_id: w1:p1\n    gate:\n      result: running\n      process_lost: true\n      at: 2026-10-01T01:30:00Z\n      sha: '1111111111111111111111111111111111111111'\n      pane: w1:p2\n  artifacts:\n  - run: R1\n    kind: report\n    title: Report\n    path: {artifact_path}\n    source: /tmp/report.md\n    sha256: {}\n    bytes: {}\n    added: 2026-10-01T01:45:00Z\n---\nThe body of {id}.\n",
        sha(REPORT),
        REPORT.len()
    )
}

pub fn simple_task(id: &str, status: &str) -> String {
    format!(
        "---\nschema_version: 2\ntask:\n  id: {id}\n  title: Task {id}\n  status: {status}\n  priority: P2\n  created: 2026-10-03T00:00:00Z\n  updated: 2026-10-03T00:00:00Z\n---\nBody of {id}.\n"
    )
}

pub fn flat_task(id: &str) -> String {
    format!(
        "---\nid: {id}\ntitle: Flat {id}\nstatus: idea\npriority: P3\ncreated: 2026-09-01T00:00:00Z\nupdated: 2026-09-01T00:00:00Z\n---\nA flat legacy task.\n"
    )
}

impl Legacy {
    pub fn new() -> Legacy {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let corral = root.join(".corral");
        fs::create_dir_all(&corral).unwrap();
        let config = corral_core::Config::default_for(&root).to_toml();
        write(&corral.join("corral.toml"), config.as_bytes());
        write(
            &corral.join(".gitignore"),
            b"artifacts/\nprompts/\nstate/\nlock/\n",
        );
        write(&corral.join("state/schema-version"), FENCE.as_bytes());
        let artifact = format!("artifacts/T-001/R1/{}-report.md", &sha(REPORT)[..8]);
        write(&corral.join(&artifact), REPORT);
        write(
            &corral.join("tasks/T-001-finished-work.md"),
            done_task("T-001", &artifact).as_bytes(),
        );
        write(
            &corral.join("tasks/T-002-backlog.md"),
            simple_task("T-002", "backlog").as_bytes(),
        );
        write(
            &corral.join("tasks/T-003-flat.md"),
            flat_task("T-003").as_bytes(),
        );
        let events = [
            r#"{"ts":"2026-10-01T00:00:00Z","task":"T-001","kind":"created","detail":{}}"#,
            r#"{"ts":"2026-10-01T01:00:00Z","task":"T-001","kind":"dispatched","actor":"owner","via":"cli","detail":{"run":"R1"}}"#,
            r#"{"ts":"2026-10-01T01:30:00Z","task":"T-001","kind":"gate_started","detail":{"run":"R1","pane":"w1:p3","socket":"/tmp/h.sock"}}"#,
            r#"{"ts":"2026-10-01T01:31:00Z","task":"T-001","kind":"run_workspace_created","detail":{"run":"R1","pane":"w2:p1","workspace":"w2"}}"#,
            r#"{"ts":"2026-10-03T00:00:00Z","task":"T-002","kind":"created","detail":{}}"#,
        ];
        write(
            &corral.join("events.jsonl"),
            (events.join("\n") + "\n").as_bytes(),
        );
        write(&corral.join("prompts/T-001-R1.md"), b"Implement T-001.\n");
        write(&corral.join("state/pm.json"), b"{\"session\":null}");
        write(
            &corral.join("state/reviews/T-001-R1-notes.md"),
            b"Looks good.\n",
        );
        write(
            &corral.join("state/reviews/T-001-R1-probe/target/CACHEDIR.TAG"),
            b"Signature: 8a477f597d28d172789f06886806bc55\n",
        );
        write(
            &corral.join("state/reviews/T-001-R1-probe/target/debug/out.bin"),
            &[0u8; 512],
        );
        write(&corral.join("lock/dispatch.lock"), b"");
        Legacy {
            _dir: dir,
            root,
            corral,
        }
    }

    pub fn migrate(&self) -> Migrate {
        Migrate::new(&self.root).wait(std::time::Duration::ZERO)
    }

    pub fn fence(&self) -> Option<String> {
        fs::read_to_string(self.corral.join("state/schema-version")).ok()
    }

    pub fn journal(&self) -> Option<serde_json::Value> {
        fs::read_to_string(self.corral.join("migration.json"))
            .ok()
            .map(|text| serde_json::from_str(&text).unwrap())
    }

    pub fn exists(&self, rel: &str) -> bool {
        fs::symlink_metadata(self.corral.join(rel)).is_ok()
    }

    /// Every legacy file's bytes outside CACHEDIR.TAG trees and the fence.
    pub fn legacy_bytes(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        for top in ["tasks", "events.jsonl", "artifacts", "prompts", "state"] {
            collect(&self.corral, &self.corral.join(top), &mut out);
        }
        out.remove("state/schema-version");
        out
    }

    /// Appends an owner comment to a task file as a 0.3.x writer would.
    pub fn legacy_comment(&self, file: &str, text: &str) {
        let path = self.corral.join("tasks").join(file);
        let raw = fs::read_to_string(&path).unwrap();
        let mut record = corral_core::parse_task_record(&raw, &path).unwrap();
        record.task.comments.push(
            serde_json::from_value(serde_json::json!({
                "at": "2026-10-05T00:00:00Z", "author": "owner", "text": text
            }))
            .unwrap(),
        );
        let rendered = corral_core::render_task(&record.task);
        fs::write(&path, rendered).unwrap();
    }

    /// Registers `a` on its task's run R1 and writes its snapshot when it has one. Returns the
    /// snapshot path.
    pub fn register_artifact(&self, a: NewArtifact<'_>) -> String {
        let NewArtifact {
            task_file,
            kind,
            file,
            content,
            stored,
            compressed,
            purged,
        } = a;
        let task_path = self.corral.join("tasks").join(task_file);
        let raw = fs::read_to_string(&task_path).unwrap();
        let mut record = corral_core::parse_task_record(&raw, &task_path).unwrap();
        let digest = sha(content);
        let path = format!("artifacts/{}/R1/{}-{file}", record.task.id, &digest[..8]);
        let mut doc = serde_json::json!({
            "run": "R1", "kind": kind, "title": file, "path": path,
            "source": format!("/tmp/{file}"), "sha256": digest, "bytes": content.len(),
            "added": "2026-10-01T01:40:00Z"
        });
        if compressed {
            doc["compressed_bytes"] = serde_json::json!(stored.map_or(0, <[u8]>::len));
        }
        if purged {
            doc["purged_at"] = serde_json::json!("2026-10-02T00:00:00Z");
        }
        record
            .task
            .artifacts
            .push(serde_json::from_value(doc).unwrap());
        fs::write(&task_path, corral_core::render_task(&record.task)).unwrap();
        if let Some(stored) = stored {
            write(&self.corral.join(&path), stored);
        }
        path
    }

    /// Writes a legacy file under `.corral`.
    pub fn put(&self, rel: &str, bytes: &[u8]) {
        write(&self.corral.join(rel), bytes);
    }

    pub fn invariant(&self) {
        let journal = corral_core::migrate::Journal::read(&self.corral).unwrap();
        let problems = directory_problems(&self.corral, journal.as_ref()).unwrap();
        assert!(problems.is_empty(), "directory invariant: {problems:?}");
    }

    pub fn integrity(&self, rel: &str) -> u64 {
        let file = self.corral.join(rel).join("corral.db");
        let problems = corral_core::sqlite::migrate::integrity_problems(&file).unwrap();
        assert!(problems.is_empty(), "{rel}: {problems:?}");
        corral_core::sqlite::migrate::read_meta(&file)
            .unwrap()
            .change_seq
    }
}

fn collect(corral: &Path, path: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_dir() {
        if path.join("CACHEDIR.TAG").is_file() {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            collect(corral, &entry.unwrap().path(), out);
        }
    } else {
        let rel = path
            .strip_prefix(corral)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        out.insert(rel, fs::read(path).unwrap());
    }
}

/// Replaces `from` with `to` (same length) in a legacy file and restores its mtime, so only the
/// content hash tells the change apart.
pub fn alter_in_place(corral: &Path, rel: &str, from: &str, to: &str) {
    assert_eq!(from.len(), to.len());
    let path = corral.join(rel);
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{rel} lacks {from:?}");
    fs::write(&path, text.replacen(from, to, 1)).unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(mtime).unwrap();
}

/// A barrier that "kills" the command at `at` (the step returns an error, as a kill would end
/// it) and records every barrier passed.
pub fn kill_at(at: &'static str) -> (Barrier, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let barrier: Barrier = Arc::new(move |id: &str| {
        log.lock().unwrap().push(id.to_owned());
        if id == at {
            Err(anyhow!("killed at {id}"))
        } else {
            Ok(())
        }
    });
    (barrier, seen)
}

/// A barrier that runs `act` at `at` and lets the command go on.
pub fn act_at(at: &'static str, act: impl Fn() -> Result<()> + Send + Sync + 'static) -> Barrier {
    Arc::new(move |id: &str| if id == at { act() } else { Ok(()) })
}

pub fn killed(result: Result<impl std::fmt::Debug>, at: &str) {
    let error = result.expect_err("the command should stop at the barrier");
    assert_eq!(format!("{error}"), format!("killed at {at}"), "{error:#}");
}
