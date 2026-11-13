//! Git movement inputs without spawning git or reading repository object contents.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitTarget {
    /// A linked worktree; its gitdir is resolved from `.git` and cached.
    Worktree { task: String, path: PathBuf },
    /// A project's common directory and relevant integration branches.
    Repo {
        project: String,
        path: PathBuf,
        branches: Vec<String>,
    },
}

#[derive(Debug, Default)]
pub struct GitWatch {
    fingerprints: BTreeMap<GitTarget, Vec<StatFingerprint>>,
    gitdirs: BTreeMap<PathBuf, GitDirs>,
}

#[derive(Debug, Clone)]
struct GitDirs {
    git: PathBuf,
    common: PathBuf,
}

type StatFingerprint = (bool, u64, i128, u64);

#[derive(Debug, Default, Clone)]
pub struct GitChanges {
    pub tasks: BTreeSet<String>,
    pub projects: BTreeSet<String>,
}

impl GitTarget {
    fn path(&self) -> &Path {
        match self {
            Self::Worktree { path, .. } | Self::Repo { path, .. } => path,
        }
    }
}

impl GitWatch {
    /// New, changed and unwatchable targets are dirty; omitted targets are forgotten.
    pub fn refresh(&mut self, targets: &[GitTarget]) -> GitChanges {
        let retained: BTreeSet<_> = targets.iter().collect();
        self.fingerprints
            .retain(|target, _| retained.contains(target));
        let paths: BTreeSet<_> = targets.iter().map(GitTarget::path).collect();
        self.gitdirs
            .retain(|path, _| paths.contains(path.as_path()));
        let mut changes = GitChanges::default();
        for target in targets {
            let path = target.path();
            if !self.gitdirs.contains_key(path)
                && let Some(dirs) = resolve_gitdirs(path)
            {
                self.gitdirs.insert(path.to_path_buf(), dirs);
            }
            let (fingerprint, watchable) = match self.gitdirs.get(path) {
                Some(dirs) => fingerprint(target, dirs),
                None => (Vec::new(), false),
            };
            let changed = !watchable || self.fingerprints.get(target) != Some(&fingerprint);
            self.fingerprints.insert(target.clone(), fingerprint);
            if changed {
                match target {
                    GitTarget::Worktree { task, .. } => {
                        changes.tasks.insert(task.clone());
                    }
                    GitTarget::Repo { project, .. } => {
                        changes.projects.insert(project.clone());
                    }
                }
            }
        }
        changes
    }
}

fn resolve_gitdirs(path: &Path) -> Option<GitDirs> {
    let dotgit = path.join(".git");
    let git = if dotgit.is_dir() {
        dotgit
    } else if dotgit.is_file() {
        let text = fs::read_to_string(&dotgit).ok()?;
        let relative = text.trim().strip_prefix("gitdir:")?.trim();
        if relative.is_empty() {
            return None;
        }
        path.join(relative)
    } else if path.join("HEAD").is_file() {
        // A configured project can also point at a bare repository.
        path.to_path_buf()
    } else {
        return None;
    };
    let common = match fs::read_to_string(git.join("commondir")) {
        Ok(text) => {
            let relative = text.trim();
            if relative.is_empty() {
                return None;
            }
            git.join(relative)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => git.clone(),
        Err(_) => return None,
    };
    Some(GitDirs { git, common })
}

fn fingerprint(target: &GitTarget, dirs: &GitDirs) -> (Vec<StatFingerprint>, bool) {
    let logs = match target {
        GitTarget::Worktree { .. } => dirs.git.join("logs"),
        GitTarget::Repo { .. } => dirs.common.join("logs"),
    };
    // An unreadable metadata path is as unsafe as disabled reflogs or reftable.
    let watchable = logs.is_dir()
        && matches!(fs::metadata(dirs.common.join("reftable")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound);
    let stats = match target {
        GitTarget::Worktree { .. } => vec![
            stat_fingerprint(&dirs.git.join("HEAD")),
            stat_fingerprint(&dirs.git.join("index")),
            stat_fingerprint(&dirs.git.join("logs/HEAD")),
        ],
        GitTarget::Repo { branches, .. } => {
            let mut stats = Vec::with_capacity(3 + 2 * branches.len());
            stats.push(stat_fingerprint(&dirs.common.join("packed-refs")));
            stats.push(stat_fingerprint(&dirs.common.join("logs/HEAD")));
            for branch in branches {
                stats.push(stat_fingerprint(
                    &dirs.common.join("refs/heads").join(branch),
                ));
                stats.push(stat_fingerprint(
                    &dirs.common.join("logs/refs/heads").join(branch),
                ));
            }
            stats.push(stat_fingerprint(&dirs.common.join("corral-plan-targets")));
            stats
        }
    };
    (stats, watchable)
}

/// Shared filesystem token: `(exists, len, mtime_ns, ino)`.
pub fn stat_fingerprint(path: &Path) -> (bool, u64, i128, u64) {
    let Ok(metadata) = fs::metadata(path) else {
        return (false, 0, 0, 0);
    };
    let mtime = metadata
        .modified()
        .ok()
        .map_or(0, |time| match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos() as i128,
            Err(error) => -(error.duration().as_nanos() as i128),
        });
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        metadata.ino()
    };
    #[cfg(not(unix))]
    let ino = 0;
    (true, metadata.len(), mtime, ino)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.logAllRefUpdates=true",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn git_watch_detects_head_move_and_ignores_quiet_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["commit", "--allow-empty", "-m", "initial"]);
        let worktree = tmp.path().join("worktree");
        git(
            &repo,
            &["worktree", "add", "-b", "task", worktree.to_str().unwrap()],
        );
        let targets = [
            GitTarget::Repo {
                project: "project".into(),
                path: repo.clone(),
                branches: vec!["main".into()],
            },
            GitTarget::Worktree {
                task: "T-001".into(),
                path: worktree.clone(),
            },
        ];
        let mut watch = GitWatch::default();
        let initial = watch.refresh(&targets);
        assert_eq!(initial.tasks, BTreeSet::from(["T-001".into()]));
        assert_eq!(initial.projects, BTreeSet::from(["project".into()]));
        let quiet = watch.refresh(&targets);
        assert!(quiet.tasks.is_empty() && quiet.projects.is_empty());

        git(
            &worktree,
            &["commit", "--allow-empty", "-m", "worktree moved"],
        );
        let moved = watch.refresh(&targets);
        assert_eq!(moved.tasks, BTreeSet::from(["T-001".into()]));
        assert!(moved.projects.is_empty());
        git(&repo, &["update-ref", "refs/heads/main", "task"]);
        let moved = watch.refresh(&targets);
        assert!(moved.tasks.is_empty());
        assert_eq!(moved.projects, BTreeSet::from(["project".into()]));
        let quiet = watch.refresh(&targets);
        assert!(quiet.tasks.is_empty() && quiet.projects.is_empty());

        watch.refresh(&targets[..1]);
        assert_eq!(
            watch.refresh(&targets).tasks,
            BTreeSet::from(["T-001".into()])
        );
    }
}
