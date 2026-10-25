use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use crate::history::{git_utf8, validate_commit};
use crate::{git, git_raw, repo_root, validate_branch_name};

/// Return the canonical shared Git directory, including when called from a linked worktree.
///
/// # Errors
/// Returns an error if Git cannot resolve the repository or the directory cannot be canonicalized.
pub fn git_common_dir(repo: &Path) -> Result<PathBuf> {
    let path = git_utf8(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    canonical(Path::new(path.trim_end_matches('\n')))
}

/// Create a literal, previously absent branch at an explicit full commit object ID.
/// Caller holds the repository `landing_lock`; no checkout is changed.
///
/// # Errors
/// Refuses invalid branch names, non-commit/expression heads, existing refs, and Git failures.
pub fn create_target_branch(repo: &Path, branch: &str, head: &str) -> Result<()> {
    validate_branch_name(branch)?;
    validate_commit(repo, head)?;
    git(repo, &["branch", "--no-track", "--", branch, head])?;
    Ok(())
}

/// A target checkout's `index.lock` is present. Git creates it only briefly, so callers should
/// retry on a later pass instead of treating it as a stuck Git operation.
#[derive(Debug)]
pub struct TransientIndexLock;

impl std::fmt::Display for TransientIndexLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Git operation ongoing: index.lock")
    }
}

impl std::error::Error for TransientIndexLock {}

/// Check an owned target checkout without modifying refs, registrations, or files.
/// Status runs with `--no-optional-locks`, so the check itself never creates `index.lock`.
/// Ownership and the expected full commit ID come from the caller's common-dir journal.
/// Caller holds `landing_lock`. Paths must be canonical, absolute, direct children of
/// `<repo>/.worktrees`; aliases, main checkouts, duplicate registrations, moved refs,
/// foreign repositories, dirty files (including ignored/untracked), and Git operations fail.
///
/// # Errors
/// Returns an error for any identity/state mismatch or filesystem/Git failure.
pub fn check_target_checkout(
    repo: &Path,
    checkout: &Path,
    branch: &str,
    expected_head: &str,
) -> Result<()> {
    let registration = target_registration(repo, checkout, branch, expected_head)?;
    ensure!(registration.is_some(), "target worktree is not registered");
    ensure!(present(checkout)?, "target checkout is missing");
    ensure!(
        canonical(checkout)? == checkout,
        "checkout path is an alias"
    );
    ensure!(repo_root(checkout)? == checkout, "checkout is not its root");
    ensure!(
        git_common_dir(checkout)? == git_common_dir(repo)?,
        "target checkout belongs to another repository"
    );
    let git_dir = git_utf8(checkout, &["rev-parse", "--absolute-git-dir"])?;
    let git_dir = canonical(Path::new(git_dir.trim_end_matches('\n')))?;
    ensure!(
        git_dir == target_admin(repo, checkout)?,
        "checkout uses the wrong worktree registration"
    );
    let backlink = fs::read_to_string(git_dir.join("gitdir"))
        .context("reading target worktree registration backlink")?;
    ensure!(
        Path::new(backlink.trim_end_matches('\n')) == checkout.join(".git"),
        "target registration points to another checkout"
    );
    ensure!(
        git_utf8(checkout, &["symbolic-ref", "--quiet", "HEAD"])?.trim_end_matches('\n')
            == format!("refs/heads/{branch}"),
        "target checkout is on the wrong branch"
    );
    ensure!(
        git_utf8(checkout, &["rev-parse", "--verify", "HEAD"])?.trim_end() == expected_head,
        "target HEAD moved"
    );
    no_operation(&git_dir)?;
    ensure!(
        git_utf8(
            checkout,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignored=matching",
                "--ignore-submodules=none",
            ],
        )?
        .is_empty(),
        "target checkout is dirty"
    );
    Ok(())
}

/// Check an existing owned target, or reconstruct a missing one from its existing branch.
/// Uses the same identity/state checks as [`check_target_checkout`]. Caller holds
/// `landing_lock` and supplies ownership from its common-dir journal. Only a missing path
/// may be created; a matching stale registration may be removed without force. Never
/// creates a missing ref, resets a checkout, switches the main checkout, or adopts a directory.
///
/// # Errors
/// Returns an error for mismatched identity/state, unsafe stale registration, or Git/I/O failure.
pub fn ensure_target_checkout(
    repo: &Path,
    checkout: &Path,
    branch: &str,
    expected_head: &str,
) -> Result<()> {
    let registered = target_registration(repo, checkout, branch, expected_head)?.is_some();
    if present(checkout)? {
        return check_target_checkout(repo, checkout, branch, expected_head);
    }
    let path = checkout
        .to_str()
        .context("non-UTF-8 target checkout path")?;
    if registered {
        // Git refuses locked or otherwise unsafe registrations. Never prune unrelated entries.
        git(repo, &["worktree", "remove", "--", path])?;
    }
    let parent = checkout.parent().context("target checkout has no parent")?;
    if !present(parent)? {
        fs::create_dir(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    git(repo, &["worktree", "add", "--", path, branch])?;
    check_target_checkout(repo, checkout, branch, expected_head)
}

/// Detach a controller-owned verification checkout at an exact commit. Creates a missing
/// canonical direct child of `<repo>/.worktrees`; an existing one must be a registered, clean
/// (tracked and untracked) worktree of the same repository. Ignored build output is retained.
///
/// # Errors
/// Refuses aliases, foreign or unregistered checkouts, dirty files and Git failures.
pub fn verification_checkout(repo: &Path, checkout: &Path, head: &str) -> Result<()> {
    validate_commit(repo, head)?;
    let root = canonical(repo)?;
    let parent = root.join(".worktrees");
    let name = checkout
        .file_name()
        .context("verification checkout has no name")?;
    ensure!(
        checkout.as_os_str() == parent.join(name).as_os_str(),
        "verification checkout must be a canonical direct child of .worktrees"
    );
    let path = checkout
        .to_str()
        .context("non-UTF-8 verification checkout path")?;
    if present(checkout)? {
        ensure!(
            canonical(checkout)? == checkout && repo_root(checkout)? == checkout,
            "verification checkout path is an alias"
        );
        ensure!(
            git_common_dir(checkout)? == git_common_dir(repo)?
                && registrations(repo)?
                    .iter()
                    .skip(1)
                    .any(|entry| entry.path == checkout),
            "verification checkout is not a registered worktree of this repository"
        );
        ensure!(
            git_utf8(
                checkout,
                &[
                    "--no-optional-locks",
                    "status",
                    "--porcelain=v1",
                    "--untracked-files=all",
                    "--ignore-submodules=none",
                ],
            )?
            .is_empty(),
            "verification checkout is dirty"
        );
        git(checkout, &["checkout", "--quiet", "--detach", head])?;
    } else {
        if !present(&parent)? {
            fs::create_dir(&parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        git(repo, &["worktree", "add", "--detach", "--", path, head])?;
    }
    ensure!(
        git_utf8(checkout, &["rev-parse", "--verify", "HEAD"])?.trim_end() == head,
        "verification checkout is not at the candidate"
    );
    Ok(())
}

fn canonical(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("canonicalizing {}", path.display()))
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("checking {}", path.display())),
    }
}

fn no_operation(git_dir: &Path) -> Result<()> {
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
        "BISECT_LOG",
        "HEAD.lock",
    ] {
        ensure!(
            !present(&git_dir.join(name))?,
            "Git operation ongoing: {name}"
        );
    }
    if present(&git_dir.join("index.lock"))? {
        return Err(TransientIndexLock.into());
    }
    Ok(())
}

fn target_admin(repo: &Path, checkout: &Path) -> Result<PathBuf> {
    let directory = git_common_dir(repo)?.join("worktrees");
    let mut found = None;
    for entry in
        fs::read_dir(&directory).with_context(|| format!("reading {}", directory.display()))?
    {
        let path = entry.context("reading worktree registration entry")?.path();
        let backlink = fs::read_to_string(path.join("gitdir"))
            .with_context(|| format!("reading {} registration", path.display()))?;
        if Path::new(backlink.trim_end_matches('\n')) == checkout.join(".git") {
            ensure!(found.is_none(), "duplicate target registration backlink");
            ensure!(canonical(&path)? == path, "aliased worktree registration");
            found = Some(path);
        }
    }
    found.context("target has no matching common-dir registration")
}

struct Registration {
    path: PathBuf,
    head: String,
    branch: Option<String>,
}

fn registrations(repo: &Path) -> Result<Vec<Registration>> {
    let text = git_utf8(repo, &["worktree", "list", "--porcelain", "-z"])?;
    text.split("\0\0")
        .filter(|record| !record.is_empty())
        .map(|record| {
            let mut fields = record.split('\0');
            let path = fields
                .next()
                .and_then(|field| field.strip_prefix("worktree "))
                .context("malformed worktree path")?;
            let mut head = None;
            let mut branch = None;
            for field in fields {
                if let Some(value) = field.strip_prefix("HEAD ") {
                    head = Some(value.to_owned());
                } else if let Some(value) = field.strip_prefix("branch ") {
                    branch = Some(value.to_owned());
                }
            }
            Ok(Registration {
                path: PathBuf::from(path),
                head: head.unwrap_or_default(),
                branch,
            })
        })
        .collect()
}

fn target_registration(
    repo: &Path,
    checkout: &Path,
    branch: &str,
    expected_head: &str,
) -> Result<Option<Registration>> {
    validate_branch_name(branch)?;
    validate_commit(repo, expected_head)?;
    let root = canonical(repo)?;
    ensure!(
        root.as_os_str() == repo.as_os_str() && repo_root(repo)? == root,
        "repository path is not canonical root"
    );
    let parent = root.join(".worktrees");
    let name = checkout
        .file_name()
        .context("target checkout has no name")?;
    ensure!(
        checkout.as_os_str() == parent.join(name).as_os_str(),
        "target must be a canonical direct child of .worktrees"
    );
    if present(&parent)? {
        ensure!(
            canonical(&parent)? == parent,
            ".worktrees parent is an alias"
        );
    }
    if present(checkout)? {
        let metadata = fs::symlink_metadata(checkout).context("reading target metadata")?;
        ensure!(
            metadata.is_dir() && !metadata.is_symlink(),
            "target is not a real directory"
        );
        ensure!(
            canonical(checkout)? == checkout,
            "checkout path is an alias"
        );
    }
    let reference = format!("refs/heads/{branch}");
    let symbolic = git_raw(repo, &["symbolic-ref", "--quiet", &reference])?;
    ensure!(
        symbolic.status.code() == Some(1),
        "target branch is symbolic or unreadable"
    );
    ensure!(
        git_utf8(repo, &["show-ref", "--verify", "--hash", &reference])?.trim_end()
            == expected_head,
        "target branch ref moved"
    );
    let entries = registrations(repo)?;
    ensure!(
        entries.first().is_none_or(|entry| entry.path != checkout),
        "target is the main checkout"
    );
    let mut owned = None;
    for entry in entries {
        if entry.path == checkout {
            ensure!(owned.is_none(), "duplicate target registration");
            ensure!(
                entry.branch.as_deref() == Some(reference.as_str()) && entry.head == expected_head,
                "wrong target branch registration"
            );
            owned = Some(entry);
        } else {
            ensure!(
                entry.branch.as_deref() != Some(reference.as_str()),
                "target branch is checked out elsewhere"
            );
            if present(&entry.path)? {
                ensure!(
                    canonical(&entry.path)? != checkout,
                    "aliased target registration"
                );
            }
        }
    }
    if owned.is_some() {
        no_operation(&target_admin(repo, checkout)?)?;
    }
    Ok(owned)
}
