use std::fs;
use std::path::Path;
use std::process::Command;

use super::*;

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    sh(&repo, &["init", "-b", "main"]);
    sh(&repo, &["config", "user.name", "T"]);
    sh(&repo, &["config", "user.email", "t@example.com"]);
    sh(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-m", "init"]);
    (tmp, repo)
}

fn wt(tmp: &tempfile::TempDir, repo: &Path, branch: &str) -> PathBuf {
    let p = tmp.path().join("wts").join(branch);
    worktree_add(repo, &p, branch, "main").unwrap();
    p
}

#[test]
fn basics_and_worktree_add() {
    let (tmp, repo) = init();
    assert!(is_repo(&repo));
    assert!(!is_repo(tmp.path()));
    assert_eq!(
        repo_root(&repo).unwrap().canonicalize().unwrap(),
        repo.canonicalize().unwrap()
    );
    assert_eq!(current_branch(&repo).unwrap(), "main");
    let w = wt(&tmp, &repo, "feat");
    assert_eq!(current_branch(&w).unwrap(), "feat");
    assert_eq!(
        rev_sha(&w, "HEAD").unwrap(),
        rev_sha(&repo, "main").unwrap()
    );

    worktree_remove(&repo, &w, false).unwrap();
    assert!(!w.exists());
    let w2 = wt(&tmp, &repo, "feat");
    assert_eq!(current_branch(&w2).unwrap(), "feat");
}

#[test]
fn diff_and_commit() {
    let (tmp, repo) = init();
    let base = rev_sha(&repo, "HEAD").unwrap();
    let w = wt(&tmp, &repo, "feat");
    assert!(is_clean(&w).unwrap());
    fs::write(w.join("b.txt"), "x\ny\n").unwrap();
    sh(&w, &["add", "b.txt"]);
    sh(&w, &["commit", "-m", "b"]);
    fs::write(w.join("a.txt"), "one\n").unwrap();
    fs::write(w.join("new.txt"), "n1\nn2\nn3\n").unwrap();
    assert!(!is_clean(&w).unwrap());
    let s = diff_stat(&w, &base).unwrap();
    assert_eq!((s.added, s.removed), (5, 1));
    assert_eq!(s.files.len(), 3);
    assert!(
        s.files
            .iter()
            .any(|f| f.path == "new.txt" && f.untracked && f.added == 3)
    );
    let t = diff_text(&w, &base).unwrap();
    assert!(t.contains("new.txt") && t.contains("+n2") && t.contains("b.txt"));
    assert!(commit_all(&w, "all").unwrap());
    assert!(!commit_all(&w, "again").unwrap());
    assert!(is_clean(&w).unwrap());
}

#[test]
fn land_fast_forward() {
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join("f.txt"), "f\n").unwrap();
    assert!(commit_all(&w, "f").unwrap());
    let sha = land(&repo, &w, "feat", "main", MergeStrategy::FastForward).unwrap();
    assert_eq!(sha, rev_sha(&w, "HEAD").unwrap());
    assert!(repo.join("f.txt").exists());
}

#[test]
fn checked_landing_rejects_base_move_at_the_final_barrier() {
    // Given identities observed by acceptance before entering the landing transaction.
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    let base = rev_sha(&repo, "main").unwrap();
    fs::write(w.join("f.txt"), "candidate\n").unwrap();
    commit_all(&w, "candidate").unwrap();
    let head = rev_sha(&w, "HEAD").unwrap();
    // Deterministically move the base at the verification/landing barrier.
    fs::write(repo.join("other.txt"), "other task\n").unwrap();
    commit_all(&repo, "other task").unwrap();
    let moved = rev_sha(&repo, "main").unwrap();
    let _lock = landing_lock(&repo).unwrap();
    let result = land_checked(&repo, &w, "feat", "main", &head, &base);
    assert!(result.unwrap_err().to_string().starts_with("stale_retry:"));
    assert_eq!(rev_sha(&repo, "main").unwrap(), moved);
    assert!(!repo.join("f.txt").exists());
}

#[test]
fn checked_landing_rejects_candidate_move_at_the_final_barrier() {
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    let base = rev_sha(&repo, "main").unwrap();
    fs::write(w.join("f.txt"), "checked\n").unwrap();
    commit_all(&w, "checked").unwrap();
    let checked = rev_sha(&w, "HEAD").unwrap();
    fs::write(w.join("f.txt"), "unchecked\n").unwrap();
    commit_all(&w, "unchecked").unwrap();
    let _lock = landing_lock(&repo).unwrap();
    assert!(land_checked(&repo, &w, "feat", "main", &checked, &base).is_err());
    assert_eq!(rev_sha(&repo, "main").unwrap(), base);
}

#[test]
fn landing_lock_is_shared_across_worktrees() {
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    let lock = landing_lock(&w).unwrap();
    let contender = fs::OpenOptions::new()
        .write(true)
        .open(repo.join(".git/corral-landing.lock"))
        .unwrap();
    assert!(matches!(
        contender.try_lock(),
        Err(fs::TryLockError::WouldBlock)
    ));
    drop(lock);
    assert!(contender.try_lock().is_ok());
}

#[test]
fn land_rebase_after_main_advanced() {
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join("f.txt"), "f\n").unwrap();
    commit_all(&w, "f").unwrap();
    fs::write(repo.join("m.txt"), "m\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-m", "m"]);

    let sha = land(&repo, &w, "feat", "main", MergeStrategy::Rebase).unwrap();
    assert_eq!(sha, rev_sha(&repo, "main").unwrap());
    assert!(repo.join("f.txt").exists() && repo.join("m.txt").exists());
}

#[test]
fn land_rejects_dirty_main_and_wrong_branch() {
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join("f.txt"), "f\n").unwrap();
    commit_all(&w, "f").unwrap();
    fs::write(repo.join("untracked.txt"), "u").unwrap();
    fs::write(repo.join("a.txt"), "dirty\n").unwrap();
    let err = land(&repo, &w, "feat", "main", MergeStrategy::FastForward).unwrap_err();
    assert!(err.to_string().contains("has uncommitted changes"), "{err}");
    sh(&repo, &["checkout", "--", "a.txt"]);
    let err = land(&repo, &w, "feat", "other", MergeStrategy::FastForward).unwrap_err();
    assert!(err.to_string().contains("is not on other"), "{err}");
    land(&repo, &w, "feat", "main", MergeStrategy::FastForward).unwrap();
}

#[test]
fn rebase_conflict_aborts_cleanly() {
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join("a.txt"), "feat\n").unwrap();
    commit_all(&w, "feat edit").unwrap();
    fs::write(repo.join("a.txt"), "main\n").unwrap();
    sh(&repo, &["commit", "-am", "main edit"]);
    let before = rev_sha(&repo, "main").unwrap();
    let err = land(&repo, &w, "feat", "main", MergeStrategy::Rebase).unwrap_err();
    assert!(err.to_string().contains("rebase conflict"), "{err}");
    assert!(is_clean(&w).unwrap());
    assert!(!w.join(".git").is_dir());
    assert_eq!(current_branch(&w).unwrap(), "feat");
    assert_eq!(rev_sha(&repo, "main").unwrap(), before);
}

#[test]
fn delete_branch_removes_it() {
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    worktree_remove(&repo, &w, true).unwrap();
    delete_branch(&repo, "feat", "main").unwrap();
    assert!(rev_sha(&repo, "feat").is_err());
}

#[test]
fn delete_branch_refuses_unmerged_even_with_merged_upstream() {
    // Given an unmerged branch whose upstream already contains its commit.
    let (tmp, repo) = init();
    let w = wt(&tmp, &repo, "feat");
    fs::write(w.join("f.txt"), "unmerged\n").unwrap();
    commit_all(&w, "unmerged").unwrap();
    sh(&repo, &["branch", "upstream", "feat"]);
    sh(&repo, &["branch", "--set-upstream-to=upstream", "feat"]);
    let before = rev_sha(&repo, "feat").unwrap();
    worktree_remove(&repo, &w, false).unwrap();

    // When safe deletion is requested against main.
    let result = delete_branch(&repo, "feat", "main");

    // Then the unmerged commit and branch survive.
    assert!(result.is_err());
    assert_eq!(rev_sha(&repo, "feat").unwrap(), before);
}
