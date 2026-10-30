use std::fs;
use std::path::{Path, PathBuf};

use crate::tests::{init, sh};
use crate::*;

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    checkout: PathBuf,
    head: String,
}

impl Fixture {
    fn new() -> Self {
        let (tmp, repo) = init();
        let repo = repo.canonicalize().unwrap();
        let head = rev_sha(&repo, "HEAD").unwrap();
        let checkout = repo.join(".worktrees/target");
        create_target_branch(&repo, "target", &head).unwrap();
        Self {
            _tmp: tmp,
            repo,
            checkout,
            head,
        }
    }

    fn add(&self) {
        ensure_target_checkout(&self.repo, &self.checkout, "target", &self.head).unwrap();
    }

    fn untouched_main(&self) {
        assert_eq!(current_branch(&self.repo).unwrap(), "main");
        assert_eq!(rev_sha(&self.repo, "refs/heads/main").unwrap(), self.head);
        assert_eq!(rev_sha(&self.repo, "HEAD").unwrap(), self.head);
        assert_eq!(fs::read(self.repo.join("a.txt")).unwrap(), b"one\ntwo\n");
        assert!(!self.repo.join("untracked").exists());
    }

    fn refused(&self, checkout: &Path, branch: &str, head: &str) {
        let refs = git(&self.repo, &["show-ref"]).unwrap();
        let worktrees = git(&self.repo, &["worktree", "list", "--porcelain", "-z"]).unwrap();
        assert!(check_target_checkout(&self.repo, checkout, branch, head).is_err());
        assert!(ensure_target_checkout(&self.repo, checkout, branch, head).is_err());
        assert_eq!(git(&self.repo, &["show-ref"]).unwrap(), refs);
        assert_eq!(
            git(&self.repo, &["worktree", "list", "--porcelain", "-z"]).unwrap(),
            worktrees
        );
        self.untouched_main();
    }
}

#[test]
fn reconstructs_absent_target_from_existing_ref() {
    // Given an owned ref without any checkout or parent directory.
    let f = Fixture::new();
    // When reconstruction is requested.
    f.add();
    // Then the exact target is clean and shares Git identity; the main checkout is untouched.
    check_target_checkout(&f.repo, &f.checkout, "target", &f.head).unwrap();
    assert_eq!(
        git_common_dir(&f.repo).unwrap(),
        git_common_dir(&f.checkout).unwrap()
    );
    assert_eq!(git_common_dir(&f.repo).unwrap(), f.repo.join(".git"));
    f.untouched_main();
}

#[test]
fn reconstructs_missing_target_with_matching_stale_registration() {
    // Given a checkout deleted outside Git, leaving its registration.
    let f = Fixture::new();
    f.add();
    fs::remove_dir_all(&f.checkout).unwrap();
    // When reconstruction is requested.
    f.add();
    // Then the registration and checkout are valid again, without moving refs or main files.
    check_target_checkout(&f.repo, &f.checkout, "target", &f.head).unwrap();
    assert_eq!(rev_sha(&f.repo, "refs/heads/target").unwrap(), f.head);
    f.untouched_main();
}

#[test]
fn refuses_recreating_a_missing_ref() {
    // Given an owned target whose ref no longer exists.
    let f = Fixture::new();
    sh(&f.repo, &["branch", "-D", "target"]);
    // When checking or ensuring it, both fail.
    f.refused(&f.checkout, "target", &f.head);
    // Then no ref, checkout, or parent was created.
    assert!(!f.checkout.parent().unwrap().exists());
    assert!(git(&f.repo, &["show-ref", "--verify", "refs/heads/target"]).is_err());
}

#[test]
fn creates_branch_only_from_exact_commit_and_refuses_existing_or_nonliteral_names() {
    // Given a repository and an explicit commit.
    let (_tmp, repo) = init();
    let head = rev_sha(&repo, "HEAD").unwrap();
    // When creating literal and invalid targets.
    create_target_branch(&repo, "release/topic", &head).unwrap();
    for branch in [
        "main",
        "release/topic",
        "@{-1}",
        "-bad",
        "refs/../bad",
        "bad\nname",
    ] {
        assert!(
            create_target_branch(&repo, branch, &head).is_err(),
            "{branch}"
        );
    }
    for head in ["HEAD", &head[..12], "--help"] {
        assert!(create_target_branch(&repo, "invalid", head).is_err());
    }
    let tree = rev_sha(&repo, "HEAD^{tree}").unwrap();
    assert!(create_target_branch(&repo, "tree", &tree).is_err());
    // Then only the requested branch exists, at the exact commit, and main is unchanged.
    assert_eq!(rev_sha(&repo, "refs/heads/release/topic").unwrap(), head);
    assert_eq!(current_branch(&repo).unwrap(), "main");
    assert_eq!(rev_sha(&repo, "HEAD").unwrap(), head);
    assert_eq!(fs::read(repo.join("a.txt")).unwrap(), b"one\ntwo\n");
}

#[test]
fn refuses_dirty_and_untracked_targets_without_discarding_files() {
    for (name, content) in [("a.txt", "changed"), ("untracked", "keep")] {
        // Given a registered target with changed or untracked content.
        let f = Fixture::new();
        f.add();
        fs::write(f.checkout.join(name), content).unwrap();
        // When checking or ensuring it.
        f.refused(&f.checkout, "target", &f.head);
        // Then the content is preserved.
        assert_eq!(fs::read_to_string(f.checkout.join(name)).unwrap(), content);
    }
}

#[test]
fn refuses_staged_and_ignored_files() {
    for staged in [false, true] {
        // Given either an ignored untracked file or staged content.
        let f = Fixture::new();
        f.add();
        if staged {
            fs::write(f.checkout.join("a.txt"), "staged").unwrap();
            sh(&f.checkout, &["add", "a.txt"]);
        } else {
            fs::write(f.repo.join(".git/info/exclude"), "ignored\n").unwrap();
            fs::write(f.checkout.join("ignored"), "keep").unwrap();
        }
        // When checking or ensuring it.
        f.refused(&f.checkout, "target", &f.head);
        // Then the index or ignored file is preserved.
        if staged {
            assert!(!git(&f.checkout, &["diff", "--cached"]).unwrap().is_empty());
        } else {
            assert_eq!(fs::read(f.checkout.join("ignored")).unwrap(), b"keep");
        }
    }
}

#[test]
fn refuses_wrong_branch_registration_and_detached_head() {
    for detached in [false, true] {
        // Given a target path registered on another branch or detached HEAD.
        let f = Fixture::new();
        f.add();
        if detached {
            sh(&f.checkout, &["checkout", "--detach"]);
        } else {
            sh(&f.checkout, &["checkout", "-b", "other"]);
        }
        // When checking or ensuring the owned branch.
        f.refused(&f.checkout, "target", &f.head);
        // Then no branch switch occurs.
        assert_eq!(
            git(&f.checkout, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap()
                .trim(),
            if detached { "HEAD" } else { "other" }
        );
    }
}

#[test]
fn refuses_branch_checked_out_elsewhere_or_twice() {
    for duplicate in [false, true] {
        // Given the owned branch checked out at an unexpected path, possibly also at target.
        let f = Fixture::new();
        if duplicate {
            f.add();
        }
        let other = f.repo.join(".worktrees/other");
        sh(
            &f.repo,
            &[
                "worktree",
                "add",
                "--force",
                other.to_str().unwrap(),
                "target",
            ],
        );
        // When checking or ensuring target.
        f.refused(&f.checkout, "target", &f.head);
        // Then the unexpected checkout remains intact.
        assert_eq!(current_branch(&other).unwrap(), "target");
        assert_eq!(f.checkout.exists(), duplicate);
    }
}

#[test]
fn refuses_ref_movement_for_existing_and_missing_checkouts() {
    for existing in [false, true] {
        // Given an externally moved target ref.
        let f = Fixture::new();
        if existing {
            f.add();
        }
        sh(&f.repo, &["checkout", "-b", "new-head"]);
        sh(&f.repo, &["commit", "--allow-empty", "-m", "new"]);
        let moved = rev_sha(&f.repo, "HEAD").unwrap();
        sh(&f.repo, &["checkout", "main"]);
        sh(&f.repo, &["update-ref", "refs/heads/target", &moved]);
        // When checking or ensuring the journal's original head.
        f.refused(&f.checkout, "target", &f.head);
        // Then the external ref movement is not undone.
        assert_eq!(rev_sha(&f.repo, "refs/heads/target").unwrap(), moved);
        assert_eq!(f.checkout.exists(), existing);
    }
}

#[test]
fn refuses_foreign_repository_or_unregistered_directory() {
    for foreign in [false, true] {
        // Given a real directory not owned by this repository's worktree registration.
        let f = Fixture::new();
        fs::create_dir_all(&f.checkout).unwrap();
        if foreign {
            sh(&f.checkout, &["init", "-b", "target"]);
        }
        fs::write(f.checkout.join("keep"), "foreign").unwrap();
        // When checking or ensuring it.
        f.refused(&f.checkout, "target", &f.head);
        // Then it is not adopted or deleted.
        assert_eq!(fs::read(f.checkout.join("keep")).unwrap(), b"foreign");
    }
}

#[test]
fn refuses_registered_path_replaced_by_foreign_repository() {
    // Given a valid registration but a foreign repository substituted at its path.
    let f = Fixture::new();
    f.add();
    fs::remove_dir_all(&f.checkout).unwrap();
    fs::create_dir(&f.checkout).unwrap();
    sh(&f.checkout, &["init", "-b", "target"]);
    fs::write(f.checkout.join("keep"), "foreign").unwrap();
    // When checking or ensuring it.
    f.refused(&f.checkout, "target", &f.head);
    // Then the foreign repository survives.
    assert!(f.checkout.join(".git").is_dir());
    assert_eq!(fs::read(f.checkout.join("keep")).unwrap(), b"foreign");
}

#[test]
fn refuses_noncanonical_nested_external_and_main_paths() {
    // Given canonical ownership and several invalid paths.
    let f = Fixture::new();
    f.add();
    // When checking/ensuring aliases, nested paths, the main checkout, and external paths.
    for path in [
        f.repo.clone(),
        f.repo.join("elsewhere"),
        f.checkout.join("nested"),
        f.repo.join(".worktrees/../.worktrees/target"),
        f.repo.join(".worktrees/./target"),
    ] {
        f.refused(&path, "target", &f.head);
    }
    f.refused(&f.repo, "main", &f.head);
    // Then the actual target still passes.
    check_target_checkout(&f.repo, &f.checkout, "target", &f.head).unwrap();
}

#[cfg(unix)]
#[test]
fn refuses_symlink_checkout_and_parent_aliases() {
    use std::os::unix::fs::symlink;

    for parent_alias in [false, true] {
        // Given a target or its .worktrees parent replaced by a symlink.
        let f = Fixture::new();
        f.add();
        let source = if parent_alias {
            f.checkout.parent().unwrap().to_path_buf()
        } else {
            f.checkout.clone()
        };
        let moved = f.repo.join("moved");
        fs::rename(&source, &moved).unwrap();
        symlink(&moved, &source).unwrap();
        // When checking or ensuring the aliased path.
        f.refused(&f.checkout, "target", &f.head);
        // Then the symlink and contents are preserved.
        assert!(fs::symlink_metadata(&source).unwrap().is_symlink());
        assert!(f.checkout.join("a.txt").exists());
    }
}

#[test]
fn refuses_git_operations_and_locked_stale_registration() {
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
        "index.lock",
        "HEAD.lock",
    ] {
        // Given a target with operation metadata, even if its files are clean.
        let f = Fixture::new();
        f.add();
        let admin = git(&f.checkout, &["rev-parse", "--absolute-git-dir"]).unwrap();
        let marker = Path::new(admin.trim()).join(marker);
        fs::write(&marker, &f.head).unwrap();
        // When checking or ensuring it.
        f.refused(&f.checkout, "target", &f.head);
        // Then operation metadata survives.
        assert!(marker.exists());
    }
    let f = Fixture::new();
    f.add();
    sh(&f.repo, &["worktree", "lock", f.checkout.to_str().unwrap()]);
    fs::remove_dir_all(&f.checkout).unwrap();
    f.refused(&f.checkout, "target", &f.head);
    assert!(!f.checkout.exists());
}

#[test]
fn refuses_symbolic_branch_ref() {
    // Given a branch ref redirected to main.
    let f = Fixture::new();
    sh(
        &f.repo,
        &["symbolic-ref", "refs/heads/target", "refs/heads/main"],
    );
    // When checking or ensuring it.
    f.refused(&f.checkout, "target", &f.head);
    // Then it remains symbolic and no checkout is made.
    assert_eq!(
        git(&f.repo, &["symbolic-ref", "refs/heads/target"])
            .unwrap()
            .trim(),
        "refs/heads/main"
    );
    assert!(!f.checkout.exists());
}

#[test]
fn refuses_stale_registration_with_wrong_branch_or_ongoing_operation() {
    for wrong_branch in [false, true] {
        // Given missing files but stale registration with unsafe identity or operation state.
        let f = Fixture::new();
        f.add();
        let admin = git(&f.checkout, &["rev-parse", "--absolute-git-dir"]).unwrap();
        if wrong_branch {
            sh(&f.checkout, &["checkout", "-b", "other"]);
        } else {
            fs::create_dir(Path::new(admin.trim()).join("rebase-merge")).unwrap();
        }
        fs::remove_dir_all(&f.checkout).unwrap();
        // When checking or ensuring it.
        f.refused(&f.checkout, "target", &f.head);
        // Then registration state is preserved and no directory is reconstructed.
        assert!(Path::new(admin.trim()).exists());
        assert!(!f.checkout.exists());
    }
}

#[test]
fn refuses_wrong_registration_backlink() {
    // Given a target whose registration claims another directory.
    let f = Fixture::new();
    f.add();
    let admin = git(&f.checkout, &["rev-parse", "--absolute-git-dir"]).unwrap();
    let backlink = Path::new(admin.trim()).join("gitdir");
    let wrong = f.repo.join(".worktrees/other/.git");
    fs::write(&backlink, wrong.to_str().unwrap()).unwrap();
    // When checking or ensuring it.
    f.refused(&f.checkout, "target", &f.head);
    // Then the mismatched registration is not repaired or adopted.
    assert_eq!(
        fs::read_to_string(backlink).unwrap(),
        wrong.to_str().unwrap()
    );
}

#[test]
fn reconstructs_without_touching_dirty_main_checkout() {
    // Given unrelated staged, unstaged, and untracked main-checkout changes.
    let f = Fixture::new();
    fs::write(f.repo.join("a.txt"), "staged").unwrap();
    sh(&f.repo, &["add", "a.txt"]);
    fs::write(f.repo.join("a.txt"), "unstaged").unwrap();
    fs::write(f.repo.join("untracked"), "keep").unwrap();
    let index = fs::read(f.repo.join(".git/index")).unwrap();
    // When reconstructing the target.
    f.add();
    // Then all main-checkout state remains byte-identical and its branch/ref stay unchanged.
    assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(f.repo.join("a.txt")).unwrap(), b"unstaged");
    assert_eq!(fs::read(f.repo.join("untracked")).unwrap(), b"keep");
    assert_eq!(current_branch(&f.repo).unwrap(), "main");
    assert_eq!(rev_sha(&f.repo, "HEAD").unwrap(), f.head);
    assert_eq!(rev_sha(&f.repo, "refs/heads/main").unwrap(), f.head);
}

#[test]
fn classifies_fixed_lock_markers_without_changing_bytes_or_mtime() {
    for marker in [
        "index.lock",
        "HEAD.lock",
        "refs/heads/target.lock",
        "packed-refs.lock",
    ] {
        // Given a real linked target with one administrative or shared lock.
        let f = Fixture::new();
        f.add();
        let admin = git(&f.checkout, &["rev-parse", "--absolute-git-dir"]).unwrap();
        let admin = Path::new(admin.trim());
        let common = git_common_dir(&f.repo).unwrap();
        let directory = if marker == "index.lock" || marker == "HEAD.lock" {
            admin
        } else {
            &common
        };
        let path = directory.join(marker);
        fs::write(&path, b"owned elsewhere\n").unwrap();
        let mtime = fs::symlink_metadata(&path).unwrap().modified().unwrap();
        let preserved: Vec<_> = [
            path.clone(),
            admin.join("index"),
            admin.join("HEAD"),
            common.join("refs/heads/target"),
        ]
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            (path, bytes, modified)
        })
        .collect();
        // When either normal preflight or reconstruction checks the target.
        for result in [
            check_target_checkout(&f.repo, &f.checkout, "target", &f.head),
            ensure_target_checkout(&f.repo, &f.checkout, "target", &f.head),
        ] {
            // Then contention retains the precise marker, path and original timestamp.
            let error = result.unwrap_err();
            let lock = error.downcast_ref::<GitLockContention>().unwrap();
            assert_eq!(lock.marker, marker);
            assert_eq!(lock.path, path);
            assert_eq!(lock.mtime, mtime);
            assert!(!error.is::<GitOperationOngoing>());
        }
        for (path, bytes, modified) in preserved {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
    }
}

#[test]
fn classifies_fixed_operation_markers_in_linked_worktree_admin() {
    for marker in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
    ] {
        // Given an operation marker and a simultaneous transient lock in a linked worktree.
        let f = Fixture::new();
        f.add();
        let admin = git(&f.checkout, &["rev-parse", "--absolute-git-dir"]).unwrap();
        let admin = Path::new(admin.trim());
        let path = admin.join(marker);
        let is_directory = matches!(marker, "rebase-apply" | "rebase-merge" | "sequencer");
        let content = if is_directory {
            fs::create_dir(&path).unwrap();
            path.join("keep")
        } else {
            path.clone()
        };
        fs::write(&content, &f.head).unwrap();
        fs::write(admin.join("index.lock"), "keep lock").unwrap();
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        // When normal target checks encounter both markers.
        for result in [
            check_target_checkout(&f.repo, &f.checkout, "target", &f.head),
            ensure_target_checkout(&f.repo, &f.checkout, "target", &f.head),
        ] {
            // Then the operation is classified first, with its metadata untouched.
            let error = result.unwrap_err();
            let operation = error.downcast_ref::<GitOperationOngoing>().unwrap();
            assert_eq!(operation.marker, marker);
            assert_eq!(operation.path, path);
            assert!(!error.is::<GitLockContention>());
        }
        assert_eq!(fs::read_to_string(content).unwrap(), f.head);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), mtime);
        assert_eq!(fs::read(admin.join("index.lock")).unwrap(), b"keep lock");
    }
}

#[test]
fn ignores_unrelated_branch_and_other_worktree_markers() {
    // Given locks on unrelated refs and the main checkout, not this linked worktree.
    let f = Fixture::new();
    f.add();
    let common = git_common_dir(&f.repo).unwrap();
    for marker in [
        "refs/heads/other.lock",
        "refs/heads/target-other.lock",
        "index.lock",
        "HEAD.lock",
        "MERGE_HEAD",
    ] {
        fs::write(common.join(marker), "unrelated").unwrap();
    }
    // When checking only the owned target.
    check_target_checkout(&f.repo, &f.checkout, "target", &f.head).unwrap();
    // Then unrelated markers survive and never become target contention.
    for marker in ["refs/heads/other.lock", "index.lock", "MERGE_HEAD"] {
        assert_eq!(fs::read(common.join(marker)).unwrap(), b"unrelated");
    }
}

#[test]
fn ownership_failures_take_precedence_over_marker_classification() {
    for mismatch in ["branch", "backlink", "foreign", "head"] {
        // Given a formerly owned checkout with broken identity and an administrative lock.
        let f = Fixture::new();
        f.add();
        let admin = git(&f.checkout, &["rev-parse", "--absolute-git-dir"]).unwrap();
        let admin = Path::new(admin.trim());
        match mismatch {
            "branch" => sh(&f.checkout, &["checkout", "-b", "other"]),
            "backlink" => {
                fs::write(admin.join("gitdir"), "/not-the-target/.git\n").unwrap();
            }
            "foreign" => {
                fs::remove_dir_all(&f.checkout).unwrap();
                fs::create_dir(&f.checkout).unwrap();
                sh(&f.checkout, &["init", "-b", "target"]);
            }
            "head" => sh(&f.checkout, &["commit", "--allow-empty", "-m", "moved"]),
            _ => unreachable!(),
        }
        fs::write(admin.join("index.lock"), "keep").unwrap();
        // When checking or ensuring the original ownership contract.
        for result in [
            check_target_checkout(&f.repo, &f.checkout, "target", &f.head),
            ensure_target_checkout(&f.repo, &f.checkout, "target", &f.head),
        ] {
            // Then identity failures cannot be turned into an infrastructure wait.
            let error = result.unwrap_err();
            assert!(!error.is::<GitLockContention>(), "{mismatch}: {error:#}");
            assert!(!error.is::<GitOperationOngoing>(), "{mismatch}: {error:#}");
        }
        assert_eq!(fs::read(admin.join("index.lock")).unwrap(), b"keep");
    }
}

#[test]
fn recorded_wait_checker_uses_no_git_and_only_fixed_paths() {
    const CHILD: &str = "CORRAL_TARGET_MARKER_NO_GIT";
    if let Some(repo) = std::env::var_os(CHILD) {
        let common = Path::new(&repo).join(".git");
        let admin = common.join("worktrees/target");
        let before = work::invocations();
        // When rechecking an existing wait with Git absent from PATH.
        let error = check_target_markers(&admin, &common, "target").unwrap_err();
        let lock = error.downcast_ref::<GitLockContention>().unwrap();
        assert_eq!(lock.marker, "index.lock");
        fs::remove_file(&lock.path).unwrap();
        check_target_markers(&admin, &common, "target").unwrap();
        // Then nested markers are not searched and no Git process was invoked.
        assert_eq!(work::invocations(), before);
        return;
    }
    // Given a real linked worktree and an isolated child with no executable search path.
    let f = Fixture::new();
    f.add();
    let common = git_common_dir(&f.repo).unwrap();
    let admin = common.join("worktrees/target");
    fs::write(admin.join("index.lock"), "wait").unwrap();
    fs::create_dir(admin.join("unrelated")).unwrap();
    fs::write(admin.join("unrelated/MERGE_HEAD"), "not a marker").unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "target_tests::recorded_wait_checker_uses_no_git_and_only_fixed_paths",
            "--nocapture",
        ])
        .env(CHILD, &f.repo)
        .env("PATH", f.repo.join("no-executables"))
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

#[test]
fn target_check_never_refreshes_the_index_or_leaves_index_lock() {
    // Given a clean target whose index stat data is stale (mtime changed, content equal).
    let f = Fixture::new();
    f.add();
    let admin = git(&f.checkout, &["rev-parse", "--absolute-git-dir"]).unwrap();
    let index = Path::new(admin.trim()).join("index");
    let file = fs::File::options()
        .write(true)
        .open(f.checkout.join("a.txt"))
        .unwrap();
    file.set_modified(
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000),
    )
    .unwrap();
    drop(file);
    let before = fs::read(&index).unwrap();
    let mtime = fs::metadata(&index).unwrap().modified().unwrap();
    // When the target is checked, then it passes.
    check_target_checkout(&f.repo, &f.checkout, "target", &f.head).unwrap();
    // Then git did not rewrite the index (optional locks are off) and left no lock.
    assert_eq!(fs::read(&index).unwrap(), before);
    assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), mtime);
    assert!(!Path::new(admin.trim()).join("index.lock").exists());
}
