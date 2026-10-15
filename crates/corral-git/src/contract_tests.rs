use std::fs;
use std::path::Path;

use crate::tests::{init, sh, wt};
use crate::*;

fn commit(repo: &Path, message: &str) -> String {
    sh(repo, &["add", "-A"]);
    sh(repo, &["commit", "-m", message]);
    rev_sha(repo, "HEAD").unwrap()
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    head: String,
    refs: Vec<u8>,
    status: Vec<u8>,
    index: Vec<u8>,
}

fn snapshot(repo: &Path) -> Snapshot {
    let index = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )
    .unwrap();
    // Read the index before status; --no-optional-locks prohibits status refreshing it.
    Snapshot {
        head: rev_sha(repo, "HEAD").unwrap(),
        refs: git_raw(repo, &["for-each-ref"]).unwrap().stdout,
        index: fs::read(index.trim_end()).unwrap(),
        status: git_raw(
            repo,
            &["--no-optional-locks", "status", "--porcelain=v1", "-z"],
        )
        .unwrap()
        .stdout,
    }
}

#[test]
fn preflight_handles_fast_forward_and_clean_divergence_with_deletion_without_mutation() {
    // Given an explicit candidate that deletes a committed file.
    let (tmp, repo) = init();
    let original = rev_sha(&repo, "HEAD").unwrap();
    let w = wt(&tmp, &repo, "feat");
    fs::remove_file(w.join("a.txt")).unwrap();
    fs::write(w.join("new.txt"), "new\n").unwrap();
    let head = commit(&w, "delete and add");
    let before = snapshot(&repo);
    // When preflighting a fast-forward, then the committed result includes the deletion.
    let MergePreflight::Clean { tree, .. } = merge_preflight(&repo, &original, &head).unwrap()
    else {
        panic!("expected clean fast-forward")
    };
    assert_eq!(tree, git(&w, &["rev-parse", "HEAD^{tree}"]).unwrap().trim());
    assert_eq!(snapshot(&repo), before);

    // Given divergence and staged, unstaged, and untracked content in the caller checkout.
    fs::write(repo.join("main.txt"), "main\n").unwrap();
    let base = commit(&repo, "main change");
    fs::write(repo.join("main.txt"), "staged\n").unwrap();
    sh(&repo, &["add", "-A"]);
    fs::write(repo.join("main.txt"), "unstaged\n").unwrap();
    fs::write(repo.join("untracked"), "untouched\n").unwrap();
    let before = snapshot(&repo);
    // When simulating the clean merge, then every checkout identity and byte remains unchanged.
    assert!(matches!(
        merge_preflight(&repo, &base, &head).unwrap(),
        MergePreflight::Clean { .. }
    ));
    assert_eq!(snapshot(&repo), before);
    assert_eq!(fs::read(repo.join("main.txt")).unwrap(), b"unstaged\n");
    assert_eq!(fs::read(repo.join("untracked")).unwrap(), b"untouched\n");
}

#[test]
fn preflight_reports_exact_special_conflict_paths_and_messages_without_mutation() {
    // Given a true content conflict on a filename unsafe for line-oriented parsing.
    let (tmp, repo) = init();
    let path = "-odd\tname\n[*]:file.txt";
    fs::write(repo.join(path), "original\n").unwrap();
    commit(&repo, "special path");
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join(path), "feature\n").unwrap();
    let head = commit(&w, "feature");
    fs::write(repo.join(path), "base\n").unwrap();
    let base = commit(&repo, "base");
    let before = snapshot(&repo);
    // When preflighting, then conflict paths and informational message paths remain exact.
    let MergePreflight::Conflict {
        paths, messages, ..
    } = merge_preflight(&repo, &base, &head).unwrap()
    else {
        panic!("expected content conflict")
    };
    assert_eq!(paths, [path]);
    assert!(
        messages
            .iter()
            .any(|m| m.paths == [path] && !m.kind.is_empty() && !m.message.is_empty())
    );
    assert_eq!(snapshot(&repo), before);
}

#[test]
fn preflight_rejects_invalid_inputs_and_real_git_infrastructure_failure() {
    // Given a repository and a separate unrelated root commit.
    let (_tmp, repo) = init();
    let base = rev_sha(&repo, "HEAD").unwrap();
    sh(&repo, &["checkout", "--orphan", "unrelated"]);
    sh(&repo, &["rm", "-rf", "."]);
    fs::write(repo.join("other"), "other\n").unwrap();
    let head = commit(&repo, "unrelated root");
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]).unwrap();
    let before = snapshot(&repo);
    // When inputs or merge execution fail, then no failure is classified as a merge conflict.
    for invalid in ["main", &base[..12], "--help", &"f".repeat(40), tree.trim()] {
        assert!(merge_preflight(&repo, invalid, &head).is_err(), "{invalid}");
    }
    assert!(merge_preflight(&repo, &base, &head).is_err());
    assert_eq!(snapshot(&repo), before);
}

#[test]
fn committed_helpers_preserve_exact_paths_subjects_order_and_deleted_blobs() {
    // Given renamed/deleted files and two commits with fixed timestamps.
    let (_tmp, repo) = init();
    let older = rev_sha(&repo, "HEAD").unwrap();
    let special = "-renamed\tfile\n[*]:.rs";
    fs::rename(repo.join("a.txt"), repo.join(special)).unwrap();
    let first = commit(&repo, "rename: exact subject");
    fs::write(repo.join("other.rs"), "// ignored\ncode\n").unwrap();
    let newer = commit(&repo, "second subject");
    fs::write(repo.join(special), "not committed\n").unwrap();
    // When reading the packet inputs, then only committed identities and content are returned.
    assert_eq!(merge_base(&repo, &older, &newer).unwrap(), older);
    assert_eq!(
        commits_between(&repo, &older, &newer).unwrap(),
        [
            Commit {
                sha: newer.clone(),
                subject: "second subject".to_owned()
            },
            Commit {
                sha: first,
                subject: "rename: exact subject".to_owned()
            },
        ]
    );
    let expected_order = git(
        &repo,
        &[
            "rev-list",
            "--topo-order",
            &format!("{older}..{newer}"),
            "--",
        ],
    )
    .unwrap();
    assert_eq!(
        commits_between(&repo, &older, &newer)
            .unwrap()
            .iter()
            .map(|c| c.sha.as_str())
            .collect::<Vec<_>>(),
        expected_order.lines().collect::<Vec<_>>()
    );
    assert_eq!(
        changed_paths_between(&repo, &older, &newer).unwrap(),
        [special, "a.txt", "other.rs"]
    );
    assert_eq!(
        read_blob(&repo, &older, "a.txt").unwrap().as_deref(),
        Some("one\ntwo\n")
    );
    assert_eq!(read_blob(&repo, &newer, "a.txt").unwrap(), None);
    assert_eq!(
        read_blob(&repo, &newer, special).unwrap().as_deref(),
        Some("one\ntwo\n")
    );
    assert_eq!(read_blob(&repo, &newer, "absent").unwrap(), None);
    assert!(commits_between(&repo, "HEAD", &newer).is_err());
    assert!(changed_paths_between(&repo, &older, "missing").is_err());
    assert!(read_blob(&repo, &"f".repeat(40), "absent").is_err());
}

#[test]
fn blob_reader_distinguishes_binary_from_invalid_utf8_text() {
    // Given both binary content and invalid UTF-8 without binary NUL markers.
    let (_tmp, repo) = init();
    fs::write(repo.join("binary"), [0xff, 0, 1]).unwrap();
    fs::write(repo.join("invalid"), [0xff, b'\n']).unwrap();
    fs::write(repo.join("empty"), []).unwrap();
    let sha = commit(&repo, "blob types");
    // When reading committed blobs, then absent/binary is None and invalid text is an error.
    assert_eq!(read_blob(&repo, &sha, "binary").unwrap(), None);
    assert!(read_blob(&repo, &sha, "invalid").is_err());
    assert_eq!(
        read_blob(&repo, &sha, "empty").unwrap(),
        Some(String::new())
    );
}

#[test]
fn merge_clean_history_can_rebase_conflict_and_restores_candidate() {
    // Given a candidate that edits then reverts a file while main independently edits it.
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join("a.txt"), "transient feature edit\n").unwrap();
    commit(&w, "transient edit");
    fs::write(w.join("a.txt"), "one\ntwo\n").unwrap();
    let head = commit(&w, "revert transient edit");
    fs::write(repo.join("a.txt"), "main edit\n").unwrap();
    let base = commit(&repo, "main edit");
    assert!(matches!(
        merge_preflight(&repo, &base, &head).unwrap(),
        MergePreflight::Clean { .. }
    ));
    // When replaying the intermediate commit, then actual unmerged paths are captured before abort.
    let RebaseOutcome::Conflict { paths, message } = rebase(&w, &base).unwrap() else {
        panic!("expected replay conflict")
    };
    assert_eq!(paths, ["a.txt"]);
    assert!(!message.is_empty());
    assert_eq!(rev_sha(&w, "HEAD").unwrap(), head);
    assert_eq!(rev_sha(&repo, "HEAD").unwrap(), base);
    assert!(is_clean(&w).unwrap());
    assert_eq!(fs::read(w.join("a.txt")).unwrap(), b"one\ntwo\n");
    assert!(!rebase_in_progress(&w).unwrap());
}

#[test]
fn rebase_infrastructure_failure_is_not_a_conflict() {
    // Given a clean candidate and an invalid rebase target.
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    let before = snapshot(&w);
    // When Git rejects the target before starting a rebase, then it returns Err without mutation.
    assert!(rebase(&w, "no-such-target").is_err());
    assert_eq!(snapshot(&w), before);
}

#[test]
fn modify_delete_conflict_reports_special_path_in_preflight_and_rebase() {
    // Given a committed special filename, deleted upstream and modified by the candidate.
    let (tmp, repo) = init();
    let path = "delete\tme\n[*].txt";
    fs::write(repo.join(path), "original\n").unwrap();
    commit(&repo, "special file");
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join(path), "modified\n").unwrap();
    let head = commit(&w, "modify file");
    fs::remove_file(repo.join(path)).unwrap();
    let base = commit(&repo, "delete file");
    let before = snapshot(&repo);
    // When simulating and replaying a modify/delete, then both outcomes expose the literal path.
    let MergePreflight::Conflict { paths, .. } = merge_preflight(&repo, &base, &head).unwrap()
    else {
        panic!("expected modify/delete conflict")
    };
    assert_eq!(paths, [path]);
    assert_eq!(snapshot(&repo), before);
    let RebaseOutcome::Conflict { paths, .. } = rebase(&w, &base).unwrap() else {
        panic!("expected replay modify/delete conflict")
    };
    assert_eq!(paths, [path]);
    assert_eq!(rev_sha(&w, "HEAD").unwrap(), head);
    assert!(is_clean(&w).unwrap());
    assert_eq!(fs::read(w.join(path)).unwrap(), b"modified\n");
}

#[cfg(unix)]
#[test]
fn rebase_abort_failure_is_an_error_not_a_conflict() {
    use std::os::unix::fs::PermissionsExt;

    // Given divergent clean commits and a real post-checkout hook that creates an index lock.
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join("feature"), "feature\n").unwrap();
    commit(&w, "feature");
    fs::write(repo.join("main"), "main\n").unwrap();
    let base = commit(&repo, "main");
    let hooks = tmp.path().join("hooks");
    fs::create_dir(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    fs::write(
        &hook,
        "#!/bin/sh\nlock=$(git rev-parse --path-format=absolute --git-path index.lock) || exit 1\n: > \"$lock\"\n",
    ).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    sh(
        &repo,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    // When replay and abort both fail to acquire the index, then abort failure is visible.
    let error = rebase(&w, &base).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("aborting failed rebase"), "{message}");
    assert!(message.contains("index.lock"), "{message}");
    assert!(rebase_in_progress(&w).unwrap());
}
