use std::fs;
use std::process::Command;
use std::sync::{Arc, Barrier};

use corral_core::Store;
use corral_core::memory::*;
use jiff::Timestamp;
use tempfile::tempdir;

fn at() -> Timestamp {
    Timestamp::from_second(123456).unwrap()
}
fn selector(scope: MemoryScope, id: &str) -> MemorySelector {
    MemorySelector {
        scope,
        id: id.into(),
    }
}
fn entry(s: &MemorySelector) -> MemoryEntry {
    MemoryEntry {
        metadata: MemoryMetadata {
            version: 1,
            id: s.id.clone(),
            scope: s.scope.clone(),
            title: "Useful guidance".into(),
            kind: MemoryKind::Guidance,
            summary: "Keep evidence separate from interpretation.".into(),
            project_into_prompt: true,
            provenance: MemoryProvenance {
                kind: ProvenanceKind::AgentInference,
                sources: vec!["task:T-123".into()],
                exact_quote: None,
            },
            updated_at: at(),
            author: "caller-selected-author".into(),
        },
        body: "Longer detail.\n".into(),
    }
}
fn put(store: &Store, s: &MemorySelector) -> MemoryItem {
    store
        .memory_put(s, entry(s), None, "worker:T-644", at())
        .unwrap()
}

#[test]
fn scoped_crud_working_tree_hashes_and_absence() {
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    assert!(
        store
            .memory_list(&MemoryScope::Workspace, 0, 20)
            .unwrap()
            .entries
            .is_empty()
    );
    for scope in [
        MemoryScope::Workspace,
        MemoryScope::Project("corral".into()),
        MemoryScope::Role(MemoryRole::Pm),
        MemoryScope::Role(MemoryRole::Assistant),
        MemoryScope::Role(MemoryRole::Worker),
    ] {
        let s = selector(scope, "guide");
        assert!(store.memory_read(&s, 0, 4096).unwrap().is_none());
        let created = put(&store, &s);
        assert_eq!(created.metadata.author, "worker:T-644");
        assert_eq!(created.metadata.updated_at, at());
        assert_eq!(created.hash.len(), 64);
        let mut replacement = entry(&s);
        replacement.body = "Replacement".into();
        assert!(
            store
                .memory_put(&s, replacement.clone(), None, "pm", at())
                .is_err()
        );
        let changed = store
            .memory_put(&s, replacement, Some(&created.hash), "pm", at())
            .unwrap();
        assert_ne!(changed.hash, created.hash);
        assert!(store.memory_remove(&s, &created.hash).is_err());
        assert_eq!(
            store.memory_read(&s, 0, 4096).unwrap().unwrap().body,
            "Replacement"
        );
        store.memory_remove(&s, &changed.hash).unwrap();
        assert!(store.memory_remove(&s, &changed.hash).is_err());
        assert!(store.memory_read(&s, 0, 4096).unwrap().is_none());
    }
    let s = selector(MemoryScope::Workspace, "manual");
    let before = put(&store, &s);
    let path = store.dir().join("memory/workspace/manual.md");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("Longer detail.", "Uncommitted owner edit.");
    fs::write(&path, &text).unwrap();
    let fresh = Store::open(dir.path()).unwrap();
    let read = fresh.memory_read(&s, 0, 4096).unwrap().unwrap();
    assert_eq!(read.body, "Uncommitted owner edit.\n");
    assert_ne!(read.item.hash, before.hash);
    assert!(
        fresh
            .memory_put(&s, entry(&s), Some(&before.hash), "pm", at())
            .is_err()
    );
    assert_eq!(fs::read_to_string(path).unwrap(), text);
}

#[test]
fn concurrent_compare_and_swap_has_one_winner() {
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let s = selector(MemoryScope::Project("corral".into()), "race");
    let original = put(&store, &s);
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let store = store.clone();
            let s = s.clone();
            let hash = original.hash.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut e = entry(&s);
                e.body = format!("writer {i}");
                barrier.wait();
                store.memory_put(&s, e, Some(&hash), "worker", at()).is_ok()
            })
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>(),
        1
    );
}

#[test]
fn validation_bounds_and_repairable_errors() {
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for id in ["", "..", "../escape", "a/b", "a.md", "a\\b", "/abs", "😀"] {
        let s = selector(MemoryScope::Workspace, id);
        assert!(store.memory_put(&s, entry(&s), None, "pm", at()).is_err());
        assert!(store.memory_read(&s, 0, 1).is_err());
    }
    assert!(
        store
            .memory_list(&MemoryScope::Project("../bad".into()), 0, 1)
            .is_err()
    );
    let s = selector(MemoryScope::Workspace, "bounded");
    let mut e = entry(&s);
    e.metadata.summary = "😀".repeat(129);
    assert!(store.memory_put(&s, e, None, "pm", at()).is_err());
    let mut e = entry(&s);
    e.body = "x".repeat(ENTRY_BYTES);
    assert!(store.memory_put(&s, e, None, "pm", at()).is_err());
    let mut e = entry(&s);
    e.metadata.id = "different".into();
    assert!(store.memory_put(&s, e, None, "pm", at()).is_err());
    let mut e = entry(&s);
    e.metadata.provenance.kind = ProvenanceKind::OwnerQuote;
    assert!(store.memory_put(&s, e, None, "pm", at()).is_err());
    put(&store, &s);
    assert!(store.memory_read(&s, 0, 0).is_err());
    assert!(store.memory_read(&s, 0, READ_BYTES + 1).is_err());
    assert!(store.memory_list(&s.scope, 0, PAGE_ENTRIES + 1).is_err());
    let path = store.dir().join("memory/workspace/bounded.md");
    for bytes in [
        b"broken frontmatter".to_vec(),
        vec![0xff],
        vec![b'x'; ENTRY_BYTES + 1],
    ] {
        fs::write(&path, bytes).unwrap();
        let error = format!("{:#}", store.memory_list(&s.scope, 0, 20).unwrap_err());
        assert!(error.contains("bounded.md"), "{error}");
        assert!(store.memory_read(&s, 0, 4096).is_err());
        assert!(store.memory_projection(MemoryRole::Pm, None).is_err());
    }
}

#[test]
fn pagination_is_sorted_bounded_and_utf8_safe() {
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for i in (0..25).rev() {
        put(
            &store,
            &selector(MemoryScope::Workspace, &format!("entry-{i:02}")),
        );
    }
    let page = store.memory_list(&MemoryScope::Workspace, 0, 20).unwrap();
    assert!(!page.entries.is_empty() && page.entries.len() <= 20);
    assert_eq!(page.entries[0].metadata.id, "entry-00");
    let page2 = store
        .memory_list(&MemoryScope::Workspace, page.next_offset.unwrap(), 20)
        .unwrap();
    assert_eq!(page2.entries.len() + page.entries.len(), 25);
    assert_eq!(
        page2.entries[0].metadata.id,
        format!("entry-{:02}", page.entries.len())
    );
    assert!(page2.next_offset.is_none());
    let s = selector(MemoryScope::Workspace, "unicode");
    let mut e = entry(&s);
    e.body = "😀é中\n".repeat(1000);
    store.memory_put(&s, e.clone(), None, "pm", at()).unwrap();
    let mut offset = 0;
    let mut body = String::new();
    loop {
        let read = store.memory_read(&s, offset, 4096).unwrap().unwrap();
        assert!(read.body.len() <= 4096);
        body.push_str(&read.body);
        match read.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(body, e.body);
    assert!(store.memory_read(&s, 1, 4096).is_err());
    assert!(store.memory_read(&s, 0, 1).is_err());
    assert!(store.memory_read(&s, e.body.len() + 1, 4096).is_err());
}

#[test]
fn complete_unicode_projection_budget_and_omitted_retrieval() {
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for scope in [
        MemoryScope::Workspace,
        MemoryScope::Role(MemoryRole::Worker),
        MemoryScope::Project("corral".into()),
    ] {
        for i in (0..24).rev() {
            let s = selector(scope.clone(), &format!("unicode-{i:02}"));
            let mut e = entry(&s);
            e.metadata.summary = "😀".repeat(128); // Exactly 512 bytes.
            e.body = format!("Full detail for {}", s.id);
            store.memory_put(&s, e, None, "worker", at()).unwrap();
        }
    }
    let private = selector(MemoryScope::Role(MemoryRole::Pm), "pm-private");
    put(&store, &private);
    let other = selector(MemoryScope::Project("other".into()), "other-private");
    put(&store, &other);
    let hidden = selector(MemoryScope::Workspace, "not-projected");
    let mut e = entry(&hidden);
    e.metadata.project_into_prompt = false;
    store.memory_put(&hidden, e, None, "pm", at()).unwrap();
    let result = store
        .memory_projection(MemoryRole::Worker, Some("corral"))
        .unwrap();
    assert!(result.text.len() <= PROJECTION_BYTES);
    assert!(result.included > 0 && result.omitted > 0);
    assert_eq!(result.included + result.omitted, 72);
    assert!(
        result
            .text
            .contains("omitted; use memory_list and memory_read")
    );
    assert!(result.text.contains("Provenance: agent_inference"));
    assert!(result.text.contains("Sources: task:T-123"));
    assert!(result.text.contains("not authority"));
    assert!(!result.text.contains("pm-private"));
    assert!(!result.text.contains("other-private"));
    assert!(!result.text.contains("not-projected"));
    assert!(!result.text.contains("Full detail"));
    // Every included summary is whole, never byte/character truncated.
    let full_summary = format!("Summary: {}\n", "😀".repeat(128));
    assert_eq!(result.text.matches(&full_summary).count(), result.included);
    assert_eq!(
        result.text,
        store
            .memory_projection(MemoryRole::Worker, Some("corral"))
            .unwrap()
            .text
    );
    let workspace_end = result.text.find("\n# Shared worker").unwrap();
    let role_end = result.text.find("\n# Bound project").unwrap();
    assert!(workspace_end <= 6144);
    assert!(role_end - workspace_end <= 2048);
    assert!(result.text.len() - role_end <= 4096);
    let omitted = selector(MemoryScope::Workspace, "unicode-23");
    assert!(!result.text.contains("## unicode-23:"));
    assert_eq!(
        store.memory_read(&omitted, 0, 4096).unwrap().unwrap().body,
        "Full detail for unicode-23"
    );
    let pm = store.memory_projection(MemoryRole::Pm, None).unwrap();
    assert!(pm.text.contains("pm-private"));
    assert!(!pm.text.contains("Bound project"));
    assert!(pm.text.len() <= 8192);
    assert!(
        store
            .memory_projection(MemoryRole::Pm, Some("corral"))
            .is_err()
    );
    let assistant = store
        .memory_projection(MemoryRole::Assistant, None)
        .unwrap();
    assert!(!assistant.text.contains("pm-private"));
    assert!(!assistant.text.contains("Bound project"));
}

#[test]
fn json_escaping_reduces_pages_without_losing_content() {
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for i in 0..12 {
        let s = selector(MemoryScope::Workspace, &format!("wire-{i:02}"));
        let mut e = entry(&s);
        e.metadata.provenance.exact_quote = Some("\\\"".repeat(700));
        e.body = "\u{0001}".repeat(2000);
        store.memory_put(&s, e, None, "pm", at()).unwrap();
    }
    let mut offset = 0;
    let mut count = 0;
    loop {
        let page = store
            .memory_list(&MemoryScope::Workspace, offset, 20)
            .unwrap();
        assert!(
            serde_json::to_vec(&page).unwrap().len() <= corral_core::agent::RESULT_MAX_BYTES - 1024
        );
        assert!(!page.entries.is_empty());
        count += page.entries.len();
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(count, 12);
    let s = selector(MemoryScope::Workspace, "wire-00");
    let mut offset = 0;
    let mut body = String::new();
    loop {
        let read = store.memory_read(&s, offset, 4096).unwrap().unwrap();
        assert!(
            serde_json::to_vec(&read).unwrap().len() <= corral_core::agent::RESULT_MAX_BYTES - 1024
        );
        assert!(!read.body.is_empty());
        body.push_str(&read.body);
        match read.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(body, "\u{0001}".repeat(2000));
    let mut e = entry(&s);
    e.metadata.provenance.exact_quote = Some("x".repeat(METADATA_JSON_BYTES));
    assert!(store.memory_put(&s, e, None, "pm", at()).is_err());
}

#[test]
fn missing_memory_default_projection_and_applicable_malformed_inputs() {
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let empty = store.memory_projection(MemoryRole::Pm, None).unwrap();
    assert_eq!((empty.included, empty.omitted), (0, 0));
    let s = selector(MemoryScope::Workspace, "default");
    put(&store, &s);
    let path = store.dir().join("memory/workspace/default.md");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("project_into_prompt: true\n", "");
    fs::write(&path, text).unwrap();
    assert!(
        !store
            .memory_read(&s, 0, 4096)
            .unwrap()
            .unwrap()
            .item
            .metadata
            .project_into_prompt
    );
    assert_eq!(
        store
            .memory_projection(MemoryRole::Pm, None)
            .unwrap()
            .included,
        0
    );
    let other = selector(MemoryScope::Project("other".into()), "broken");
    put(&store, &other);
    fs::write(
        store.dir().join("memory/projects/other/broken.md"),
        "broken",
    )
    .unwrap();
    // Unrelated project input must neither leak nor break an authorized projection.
    assert!(
        store
            .memory_projection(MemoryRole::Worker, Some("corral"))
            .is_ok()
    );
    assert!(
        store
            .memory_projection(MemoryRole::Worker, Some("other"))
            .is_err()
    );
    fs::write(path, "broken").unwrap();
    assert!(store.memory_projection(MemoryRole::Pm, None).is_err());
}

#[cfg(unix)]
#[test]
fn refuses_symlinks_directories_and_special_files_without_reading_them() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let s = selector(MemoryScope::Workspace, "special");
    let item = put(&store, &s);
    let path = store.dir().join("memory/workspace/special.md");
    fs::remove_file(&path).unwrap();
    symlink("/dev/zero", &path).unwrap();
    assert!(store.memory_read(&s, 0, 4096).is_err());
    assert!(store.memory_list(&s.scope, 0, 20).is_err());
    assert!(store.memory_remove(&s, &item.hash).is_err());
    assert!(
        store
            .memory_put(&s, entry(&s), Some(&item.hash), "pm", at())
            .is_err()
    );
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(store.memory_read(&s, 0, 4096).is_err());
    fs::remove_dir(&path).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    assert!(store.memory_read(&s, 0, 4096).is_err());
    assert!(store.memory_put(&s, entry(&s), None, "pm", at()).is_err());
    fs::remove_file(&path).unwrap();
    fs::remove_dir(store.dir().join("memory/workspace")).unwrap();
    symlink(dir.path(), store.dir().join("memory/workspace")).unwrap();
    assert!(store.memory_read(&s, 0, 4096).is_err());
    assert!(store.memory_put(&s, entry(&s), None, "pm", at()).is_err());
    assert!(!dir.path().join("special.md").exists());
}

#[test]
fn memory_writes_leave_git_head_index_and_unrelated_bytes_untouched() {
    let dir = tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };
    git(&["init", "-q"]);
    fs::write(dir.path().join("unrelated"), "baseline").unwrap();
    git(&["add", "unrelated"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "baseline",
    ]);
    fs::write(dir.path().join("unrelated"), "staged").unwrap();
    git(&["add", "unrelated"]);
    fs::write(dir.path().join("unrelated"), "unstaged").unwrap();
    let store = Store::init(dir.path()).unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = git(&["rev-parse", "HEAD"]);
    let s = selector(MemoryScope::Workspace, "git-visible");
    let item = put(&store, &s);
    assert_eq!(head, git(&["rev-parse", "HEAD"]));
    assert_eq!(index, fs::read(dir.path().join(".git/index")).unwrap());
    assert_eq!(
        fs::read_to_string(dir.path().join("unrelated")).unwrap(),
        "unstaged"
    );
    assert!(
        String::from_utf8(git(&[
            "ls-files",
            "--others",
            "--exclude-standard",
            ".corral/memory"
        ]))
        .unwrap()
        .contains("git-visible.md")
    );
    store.memory_remove(&s, &item.hash).unwrap();
    assert_eq!(head, git(&["rev-parse", "HEAD"]));
    assert_eq!(index, fs::read(dir.path().join(".git/index")).unwrap());
}
