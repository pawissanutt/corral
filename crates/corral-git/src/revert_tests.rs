use super::*;
use std::fs;
use std::process::{Command, Stdio};

/// Exercise production subprocesses in an empty environment, not the owner's Git config.
fn isolated() -> bool {
    let name = std::thread::current().name().unwrap().to_owned();
    if std::env::var("CORRAL_REVERT_TEST").as_deref() == Ok(&name) {
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    let gitconfig = home.path().join("gitconfig");
    fs::write(&gitconfig, "").unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", &name, "--nocapture"])
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("TMPDIR", home.path())
        .env("TMP", home.path())
        .env("TEMP", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", gitconfig)
        .env("GIT_AUTHOR_NAME", "Revert test")
        .env("GIT_AUTHOR_EMAIL", "revert@test.invalid")
        .env("GIT_COMMITTER_NAME", "Revert test")
        .env("GIT_COMMITTER_EMAIL", "revert@test.invalid")
        .env("GIT_AUTHOR_DATE", "2026-10-16T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-16T00:00:00Z")
        .env("CORRAL_REVERT_TEST", &name)
        .stdin(Stdio::null());
    #[cfg(unix)]
    child.env("PATH", "/usr/bin:/bin");
    #[cfg(windows)]
    {
        child.env("PATH", std::env::var_os("PATH").unwrap());
        child.env("SystemRoot", std::env::var_os("SystemRoot").unwrap());
    }
    let out = child.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    true
}

fn command(repo: &Path, args: &[&str]) -> String {
    let out = git_raw(repo, args).unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

fn commit(repo: &Path, path: &str, content: &str) -> String {
    fs::write(repo.join(path), content).unwrap();
    command(repo, &["add", "--", path]);
    command(repo, &["commit", "-qm", path]);
    command(repo, &["rev-parse", "HEAD"])
}

struct Fx {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    range: RevertRange,
    originals: Vec<String>,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        fs::create_dir(&repo).unwrap();
        command(&repo, &["init", "-qb", "main"]);
        command(&repo, &["config", "user.name", "Revert test"]);
        command(&repo, &["config", "user.email", "revert@test.invalid"]);
        command(&repo, &["config", "commit.gpgsign", "false"]);
        command(&repo, &["config", "core.autocrlf", "false"]);
        let hooks = tmp.path().join("hooks");
        fs::create_dir(&hooks).unwrap();
        command(
            &repo,
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        );
        commit(&repo, "task.txt", "original\n");
        let base_before = commit(&repo, "peer-before.txt", "keep before\n");
        let first = commit(&repo, "task.txt", "task\n");
        let landed_sha = commit(&repo, "second.txt", "task second\n");
        Self {
            _tmp: tmp,
            repo,
            range: RevertRange {
                base_before,
                landed_sha: landed_sha.clone(),
            },
            originals: vec![landed_sha, first],
        }
    }

    fn revert(&self) -> Result<RevertedRange> {
        let _lock = crate::landing_lock(&self.repo)?;
        revert_range_prepared(&self.repo, "main", &self.range, |_, _| Ok(()))
    }
}

#[test]
fn revert_preserves_unrelated_commits_before_and_after_a_multiple_commit_landing() {
    if isolated() {
        return;
    }
    // Given an exact two-commit landing surrounded by unrelated base commits.
    let fx = Fx::new();
    let later = commit(&fx.repo, "peer-after.txt", "keep after\n");
    let branches = command(&fx.repo, &["branch", "--format=%(refname)"]);

    // When the landing is reverted.
    let result = fx.revert().unwrap();

    // Then only the task's changes disappear, and history is appended, not rewritten.
    assert_eq!(result.original_shas, fx.originals);
    assert_eq!(result.revert_shas.len(), 1);
    assert_eq!(result.base_before, later);
    assert_eq!(command(&fx.repo, &["rev-parse", "HEAD^"]), later);
    assert_eq!(
        command(&fx.repo, &["rev-parse", "HEAD"]),
        result.revert_shas[0]
    );
    assert_eq!(
        fs::read_to_string(fx.repo.join("task.txt")).unwrap(),
        "original\n"
    );
    assert!(!fx.repo.join("second.txt").exists());
    assert_eq!(
        fs::read_to_string(fx.repo.join("peer-before.txt")).unwrap(),
        "keep before\n"
    );
    assert_eq!(
        fs::read_to_string(fx.repo.join("peer-after.txt")).unwrap(),
        "keep after\n"
    );
    assert!(crate::is_clean(&fx.repo).unwrap());
    assert_eq!(
        command(&fx.repo, &["branch", "--format=%(refname)"]),
        branches
    );
    assert_eq!(
        command(&fx.repo, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[test]
fn revert_conflict_names_later_sha_and_paths_without_changing_base_or_index() {
    if isolated() {
        return;
    }
    // Given a later commit replacing the task's changed line.
    let fx = Fx::new();
    let later = commit(&fx.repo, "task.txt", "later owner change\n");
    let index = command(&fx.repo, &["write-tree"]);
    let worktrees = command(&fx.repo, &["worktree", "list", "--porcelain"]);

    // When the task is reverted.
    let error = fx.revert().unwrap_err().to_string();

    // Then the exact conflict is named and the base remains clean and unchanged.
    assert!(error.contains(&later), "{error}");
    assert!(error.contains("task.txt"), "{error}");
    assert_eq!(command(&fx.repo, &["rev-parse", "HEAD"]), later);
    assert_eq!(command(&fx.repo, &["write-tree"]), index);
    assert_eq!(
        fs::read_to_string(fx.repo.join("task.txt")).unwrap(),
        "later owner change\n"
    );
    assert!(fx.repo.join("second.txt").exists());
    assert!(crate::is_clean(&fx.repo).unwrap());
    assert_eq!(
        command(&fx.repo, &["worktree", "list", "--porcelain"]),
        worktrees
    );
}

#[test]
fn revert_preserves_nonconflicting_later_edits_in_the_same_file() {
    if isolated() {
        return;
    }
    // Given independent edits separated by more than Git's conflict context.
    let fx = Fx::new();
    let original = (0..20).map(|n| format!("line {n}\n")).collect::<String>();
    let before = commit(&fx.repo, "shared.txt", &original);
    let task = original.replacen("line 0\n", "task line\n", 1);
    let landed = commit(&fx.repo, "shared.txt", &task);
    let later = task.replace("line 19\n", "later line\n");
    commit(&fx.repo, "shared.txt", &later);
    let range = RevertRange {
        base_before: before,
        landed_sha: landed,
    };

    // When the task's independent line is reverted.
    let _lock = crate::landing_lock(&fx.repo).unwrap();
    revert_range_prepared(&fx.repo, "main", &range, |_, _| Ok(())).unwrap();

    // Then the later edit remains rather than treating every path overlap as a conflict.
    assert_eq!(
        fs::read_to_string(fx.repo.join("shared.txt")).unwrap(),
        original.replace("line 19\n", "later line\n")
    );
}

#[test]
fn preparation_refusal_does_not_change_base() {
    if isolated() {
        return;
    }
    // Given a clean base and a refused durable operational preparation.
    let fx = Fx::new();
    let before = command(&fx.repo, &["rev-parse", "HEAD"]);
    let _lock = crate::landing_lock(&fx.repo).unwrap();

    // When preparation rejects the otherwise valid inverse.
    let result = revert_range_prepared(&fx.repo, "main", &fx.range, |_, _| {
        bail!("fixture operational refusal")
    });

    // Then no revert is applied and the private worktree is gone.
    assert!(result.is_err());
    assert_eq!(command(&fx.repo, &["rev-parse", "HEAD"]), before);
    assert!(crate::is_clean(&fx.repo).unwrap());
    assert_eq!(
        command(&fx.repo, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[test]
fn unlanded_range_is_refused_without_changing_base() {
    if isolated() {
        return;
    }
    // Given a kept branch with an unlanded commit.
    let fx = Fx::new();
    let before = command(&fx.repo, &["rev-parse", "HEAD"]);
    command(&fx.repo, &["checkout", "-qb", "kept"]);
    let sha = commit(&fx.repo, "kept.txt", "unlanded\n");
    command(&fx.repo, &["checkout", "main"]);
    let range = RevertRange {
        base_before: before.clone(),
        landed_sha: sha,
    };
    let _lock = crate::landing_lock(&fx.repo).unwrap();

    // When an unlanded range is requested.
    let result = revert_range_prepared(&fx.repo, "main", &range, |_, _| Ok(()));

    // Then the current base is untouched.
    assert!(result.is_err());
    assert_eq!(command(&fx.repo, &["rev-parse", "HEAD"]), before);
    assert!(crate::is_clean(&fx.repo).unwrap());
}

#[test]
fn merged_side_branch_is_reverted_once_with_the_exact_landed_range() {
    if isolated() {
        return;
    }
    // Given a fast-forward landing that includes a task's own merged side branch.
    let fx = Fx::new();
    let before = command(&fx.repo, &["rev-parse", "HEAD"]);
    command(&fx.repo, &["branch", "side"]);
    let first = commit(&fx.repo, "task.txt", "merged task\n");
    command(&fx.repo, &["checkout", "side"]);
    let side = commit(&fx.repo, "side.txt", "side task\n");
    command(&fx.repo, &["checkout", "main"]);
    command(&fx.repo, &["merge", "--no-ff", "-m", "task merge", "side"]);
    let merge = command(&fx.repo, &["rev-parse", "HEAD"]);
    let landed = commit(&fx.repo, "tail.txt", "task tail\n");
    let later = commit(&fx.repo, "later.txt", "keep later\n");
    let range = RevertRange {
        base_before: before,
        landed_sha: landed.clone(),
    };
    let _lock = crate::landing_lock(&fx.repo).unwrap();

    // When the range, including its merge, is inverted.
    let result = revert_range_prepared(&fx.repo, "main", &range, |_, _| Ok(())).unwrap();

    // Then all task effects are gone without double-reverting the merged side commit.
    let mut originals = result.original_shas;
    originals.sort();
    let mut expected = vec![first, side, merge, landed];
    expected.sort();
    assert_eq!(originals, expected);
    assert_eq!(
        fs::read_to_string(fx.repo.join("task.txt")).unwrap(),
        "task\n"
    );
    assert!(!fx.repo.join("side.txt").exists());
    assert!(!fx.repo.join("tail.txt").exists());
    assert_eq!(
        fs::read_to_string(fx.repo.join("later.txt")).unwrap(),
        "keep later\n"
    );
    assert_eq!(command(&fx.repo, &["rev-parse", "HEAD^"]), later);
    assert!(crate::is_clean(&fx.repo).unwrap());
}

#[test]
fn merge_mainline_follows_recorded_base_even_when_it_is_the_second_parent() {
    if isolated() {
        return;
    }
    // Given a task that merged an advanced main through its second parent before FF landing.
    let fx = Fx::new();
    command(&fx.repo, &["checkout", "-qb", "task-merge"]);
    let task = commit(&fx.repo, "task.txt", "task before main merge\n");
    command(&fx.repo, &["checkout", "main"]);
    let before = commit(&fx.repo, "peer-integrated.txt", "keep integrated peer\n");
    command(&fx.repo, &["checkout", "task-merge"]);
    command(
        &fx.repo,
        &["merge", "--no-ff", "-m", "integrate main", "main"],
    );
    let landed = command(&fx.repo, &["rev-parse", "HEAD"]);
    command(&fx.repo, &["checkout", "main"]);
    command(&fx.repo, &["merge", "--ff-only", "task-merge"]);
    let range = RevertRange {
        base_before: before,
        landed_sha: landed.clone(),
    };
    let _lock = crate::landing_lock(&fx.repo).unwrap();

    // When the exact approval range is reverted.
    let result = revert_range_prepared(&fx.repo, "main", &range, |_, _| Ok(())).unwrap();

    // Then the chosen -m 2 preserves the peer and removes the task's branch effects.
    assert_eq!(result.original_shas, vec![landed, task]);
    assert_eq!(
        fs::read_to_string(fx.repo.join("task.txt")).unwrap(),
        "task\n"
    );
    assert_eq!(
        fs::read_to_string(fx.repo.join("peer-integrated.txt")).unwrap(),
        "keep integrated peer\n"
    );
    assert!(crate::is_clean(&fx.repo).unwrap());
}
