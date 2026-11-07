use super::{
    content::{FILES_MAX, TOTAL_MAX, safe_path},
    git::{Snapshot, git, scan},
};
use anyhow::{Context, Result, ensure};
use corral_core::Store;
use std::path::Path;

impl Snapshot {
    /// Restore into a NEW disposable repository, never overwrite a checkout or move source
    /// work. C3 may adopt only after this verification; interrupted directories are not seeds.
    pub fn restore_new(&self, store: &Store, destination: &Path) -> Result<()> {
        ensure!(
            !destination
                .try_exists()
                .context("inspect restore destination")?,
            "snapshot_destination_exists: {}",
            destination.display()
        );
        let parent = destination
            .parent()
            .context("restore destination has no parent")?;
        let parent = std::path::absolute(parent).context("resolve restore parent")?;
        let guard = corral_core::fs_guard::DirHandle::open(&parent)?;
        ensure!(
            self.state.files.len() <= FILES_MAX,
            "snapshot_oversized: file manifest"
        );
        let mut total = self
            .state
            .index
            .bytes
            .checked_add(self.bundle.bytes)
            .context("snapshot_oversized: length overflow")?;
        let mut paths = std::collections::BTreeSet::new();
        for entry in &self.state.files {
            safe_path(&entry.path)?;
            ensure!(
                paths.insert(&entry.path),
                "snapshot_corrupt: duplicate path {}",
                entry.path
            );
            ensure!(
                entry.data.is_some() == entry.mode.is_some()
                    && entry.mode.is_none_or(|m| m <= 0o777),
                "snapshot_corrupt: mode for {}",
                entry.path
            );
            total = total
                .checked_add(entry.data.as_ref().map_or(0, |d| d.bytes))
                .context("snapshot_oversized: length overflow")?;
        }
        ensure!(
            total <= TOTAL_MAX,
            "snapshot_oversized: manifest bytes exceed 64 MiB"
        );
        let temp = tempfile::tempdir_in(&parent).context("create private restoration directory")?;
        let bundle = temp.path().join("seed.bundle");
        std::fs::write(&bundle, self.bundle.load(store)?).context("write verified bundle")?;
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).context("create restore repository")?;
        ensure!(
            matches!(self.state.object_format.as_str(), "sha1" | "sha256"),
            "snapshot_corrupt: object format"
        );
        git(
            &repo,
            &[
                "init",
                "--template=",
                &format!("--object-format={}", self.state.object_format),
            ],
            None,
        )?;
        git(
            &repo,
            &[
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                bundle.to_str().context("bundle path is not UTF-8")?,
                "refs/heads/base:refs/corral/base",
                "refs/heads/head:refs/corral/head",
                "refs/heads/branch:refs/corral/branch",
                "refs/heads/index:refs/corral/index",
            ],
            None,
        )?;
        for (name, expected) in [
            ("base", &self.state.base),
            ("head", &self.state.head),
            ("branch", &self.state.branch_tip),
        ] {
            let actual = git(&repo, &["rev-parse", &format!("refs/corral/{name}")], None)?;
            ensure!(
                actual == format!("{expected}\n").as_bytes(),
                "snapshot_corrupt: bundle {name} identity differs"
            );
        }
        let branch = format!("refs/heads/{}", self.state.branch);
        git(&repo, &["check-ref-format", &branch], None)?;
        git(
            &repo,
            &["update-ref", &branch, &self.state.branch_tip],
            None,
        )?;
        if self.state.detached {
            git(
                &repo,
                &["update-ref", "--no-deref", "HEAD", &self.state.head],
                None,
            )?;
        } else {
            ensure!(
                self.state.head == self.state.branch_tip,
                "snapshot_corrupt: attached HEAD differs from branch"
            );
            git(&repo, &["symbolic-ref", "HEAD", &branch], None)?;
        }
        std::fs::write(repo.join(".git/index"), self.state.index.load(store)?)
            .context("restore exact index bytes")?;
        for entry in &self.state.files {
            if let Some(content) = &entry.data {
                let path = repo.join(&entry.path);
                std::fs::create_dir_all(path.parent().context("file has no parent")?)
                    .with_context(|| format!("create parent of {}", entry.path))?;
                std::fs::write(&path, content.load(store)?)
                    .with_context(|| format!("restore {}", entry.path))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        &path,
                        std::fs::Permissions::from_mode(entry.mode.context("missing file mode")?),
                    )?;
                }
            }
        }
        ensure!(
            scan(&repo, &self.state.base, &self.state.branch)?.state == self.state,
            "snapshot_restore_mismatch: exact index/work manifest differs"
        );
        // The index tree's objects must be complete, not only the raw index file.
        git(&repo, &["fsck", "--full", "--no-reflogs"], None)?;
        crate::agent::failpoint("handoff_restore_verified");
        guard.revalidate()?;
        ensure!(
            !destination.try_exists()?,
            "snapshot_destination_exists: {}",
            destination.display()
        );
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            rustix::fs::renameat_with(
                rustix::fs::CWD,
                &repo,
                rustix::fs::CWD,
                destination,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .context("publish verified restore without replacing an existing destination")?;
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        anyhow::bail!(
            "snapshot_unsupported: atomic no-replace restore publication requires Linux or macOS"
        )
    }
}
