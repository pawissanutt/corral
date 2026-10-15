use std::fs;
use std::path::Path;
use std::process::Command;

use crate::tests::{init, sh};
use crate::{changed_paths_with_renames, observed_paths, rev_sha};

fn commit(repo: &Path) -> String {
    sh(repo, &["add", "-A"]);
    sh(repo, &["commit", "-m", "paths"]);
    rev_sha(repo, "HEAD").unwrap()
}

fn sorted(paths: &[&str]) -> Vec<String> {
    let mut paths: Vec<_> = paths.iter().map(|p| (*p).to_owned()).collect();
    paths.sort();
    paths
}

#[test]
fn committed_paths_include_both_exact_rename_endpoints() {
    // Given a committed rename, addition, modification, and deletion.
    let (_tmp, repo) = init();
    fs::write(repo.join("delete me\n"), "deleted\n").unwrap();
    fs::write(repo.join("modified file"), "before\n").unwrap();
    let older = commit(&repo);
    let renamed = "-new name\twith\nnewline";
    fs::rename(repo.join("a.txt"), repo.join(renamed)).unwrap();
    fs::remove_file(repo.join("delete me\n")).unwrap();
    fs::write(repo.join("modified file"), "after\n").unwrap();
    fs::write(repo.join("added\nfile"), "added\n").unwrap();
    let newer = commit(&repo);
    // When comparing the explicit commits, then every literal endpoint is returned.
    assert_eq!(
        changed_paths_with_renames(&repo, &older, &newer).unwrap(),
        sorted(&[
            renamed,
            "a.txt",
            "delete me\n",
            "modified file",
            "added\nfile"
        ])
    );
    assert!(
        changed_paths_with_renames(&repo, &newer, &newer)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn observed_paths_include_staged_unstaged_untracked_and_rename_paths() {
    // Given a staged rename and a file modified in both index and worktree.
    let (_tmp, repo) = init();
    let modified = "modified with\nnewline";
    fs::write(repo.join(modified), "initial\n").unwrap();
    commit(&repo);
    let renamed = "renamed with\nnewline";
    sh(&repo, &["mv", "--", "a.txt", renamed]);
    fs::write(repo.join(modified), "staged\n").unwrap();
    sh(&repo, &["add", "--", modified]);
    fs::write(repo.join(modified), "unstaged\n").unwrap();
    fs::write(repo.join("staged addition\n"), "staged addition\n").unwrap();
    sh(&repo, &["add", "--", "staged addition\n"]);
    fs::create_dir(repo.join("untracked dir")).unwrap();
    fs::write(repo.join("untracked dir/file\nname"), "untracked\n").unwrap();
    // When observing status, then paths are exact, sorted, and deduplicated.
    assert_eq!(
        observed_paths(&repo).unwrap(),
        sorted(&[
            "a.txt",
            renamed,
            modified,
            "staged addition\n",
            "untracked dir/file\nname"
        ])
    );
}

#[test]
fn observed_paths_include_both_unstaged_move_endpoints() {
    // Given a worktree move not yet staged.
    let (_tmp, repo) = init();
    let renamed = "unstaged rename\n";
    fs::rename(repo.join("a.txt"), repo.join(renamed)).unwrap();
    // When observing status, then the deletion and untracked destination are included.
    assert_eq!(observed_paths(&repo).unwrap(), sorted(&["a.txt", renamed]));
}

#[test]
fn path_apis_reject_invalid_commits_and_git_errors() {
    // Given valid commits plus a non-repository directory and missing directory.
    let (tmp, repo) = init();
    let sha = rev_sha(&repo, "HEAD").unwrap();
    let tree = rev_sha(&repo, "HEAD^{tree}").unwrap();
    // When inputs cannot name committed trees, then the error remains visible.
    for invalid in ["HEAD", &sha[..12], "--help", &"f".repeat(40), &tree] {
        assert!(changed_paths_with_renames(&repo, invalid, &sha).is_err());
        assert!(changed_paths_with_renames(&repo, &sha, invalid).is_err());
    }
    for invalid_repo in [tmp.path().to_path_buf(), tmp.path().join("absent")] {
        assert!(changed_paths_with_renames(&invalid_repo, &sha, &sha).is_err());
        assert!(observed_paths(&invalid_repo).is_err());
    }
    assert!(observed_paths(&repo).unwrap().is_empty());
}

#[test]
fn path_apis_report_absent_git_without_global_environment_changes() {
    const CHILD_REPO: &str = "CORRAL_PATH_TEST_NO_GIT_REPO";
    if let Some(repo) = std::env::var_os(CHILD_REPO) {
        let repo = Path::new(&repo);
        let sha = "1".repeat(40);
        for error in [
            observed_paths(repo).unwrap_err(),
            changed_paths_with_renames(repo, &sha, &sha).unwrap_err(),
        ] {
            assert!(format!("{error:#}").contains("failed to spawn"));
        }
        return;
    }
    // Given a subprocess with an empty executable search path.
    let (tmp, repo) = init();
    let empty_path = tmp.path().join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    // When both APIs run without Git, then they return contextual spawn errors.
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "path_tests::path_apis_report_absent_git_without_global_environment_changes",
            "--nocapture",
        ])
        .env("PATH", empty_path)
        .env(CHILD_REPO, repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "child failed: {} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"));
}

#[cfg(unix)]
#[test]
fn observed_paths_do_not_open_fifos_or_follow_symlink_targets() {
    use std::os::unix::fs::symlink;

    // Given an external FIFO with no writer, and tracked/untracked symlinks.
    let (tmp, repo) = init();
    symlink("old target", repo.join("tracked link")).unwrap();
    commit(&repo);
    let fifo = tmp.path().join("fifo");
    let out = Command::new("mkfifo").arg(&fifo).output().unwrap();
    assert!(out.status.success());
    fs::remove_file(repo.join("tracked link")).unwrap();
    symlink(&fifo, repo.join("tracked link")).unwrap();
    symlink(&fifo, repo.join("untracked link\n")).unwrap();
    symlink("/dev/zero", repo.join("unbounded target")).unwrap();
    let out = Command::new("mkfifo")
        .arg(repo.join("untracked fifo"))
        .output()
        .unwrap();
    assert!(out.status.success());
    // When observing paths, then only Git-observable paths appear without target reads.
    // Git ignores untracked FIFOs themselves; their symlink names remain observable.
    assert_eq!(
        observed_paths(&repo).unwrap(),
        sorted(&["tracked link", "untracked link\n", "unbounded target"])
    );
}
