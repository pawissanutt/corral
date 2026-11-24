use std::fs::{self, File};
use std::io::{BufReader, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{
    Activity, ProjectDelta, ProjectFacts, Revision, TreeFacts, delta, git, provider::Reply,
    structure,
};

/// Rebuildable system map cache in `<workspace>/.corral-next/cache/system-map/`.
#[derive(Debug, Clone)]
pub struct FactsCache {
    directory: PathBuf,
}

impl FactsCache {
    pub fn new(workspace: &Path) -> Self {
        Self {
            directory: workspace.join(".corral-next/cache/system-map/v2"),
        }
    }

    /// Read structural facts by full tree object id. A cache hit does not access the repository.
    pub fn tree_facts(&self, repo: &Path, tree: &str) -> Result<TreeFacts> {
        let tree = object_id(tree)?;
        // The older cache has a different `spawns` meaning. Keep structural
        // entries separate in both directions, including on daemon rollback.
        self.get("trees-v3", &tree, || structure::collect(repo, &tree))
    }

    /// Read facts for a revision produced by [`crate::resolve_revision`].
    /// The 7- and 30-day activity windows end at this revision's commit time.
    pub fn project_facts(&self, repo: &Path, revision: &Revision) -> Result<ProjectFacts> {
        let commit = object_id(&revision.commit)?;
        let structure = self.tree_facts(repo, &revision.tree)?;
        let history_key = if revision.shallow_boundary.is_empty() {
            commit
        } else {
            let boundary = Sha256::digest(revision.shallow_boundary.join("\n"));
            format!("{commit}-{boundary:x}")
        };
        let activity: Activity =
            self.get("activity", &history_key, || git::activity(repo, revision))?;
        Ok(ProjectFacts {
            revision: revision.clone(),
            structure,
            activity,
        })
    }

    /// Compare two resolved revisions, without reading either one's history or working tree.
    pub fn delta(&self, repo: &Path, base: &Revision, head: &Revision) -> Result<ProjectDelta> {
        let base_tree = object_id(&base.tree)?;
        let head_tree = object_id(&head.tree)?;
        let base_commit = object_id(&base.commit)?;
        let head_commit = object_id(&head.commit)?;
        if base_tree == head_tree {
            return Ok(ProjectDelta {
                files: vec![],
                skipped_non_utf8_paths: 0,
                dependencies: vec![],
                module_references: vec![],
                runtime: Default::default(),
                changes: None,
                dependents: None,
                tests: None,
            });
        }
        self.get("deltas-v3", &format!("{base_tree}-{head_tree}"), || {
            let before = self.tree_facts(repo, &base_tree)?;
            let after = self.tree_facts(repo, &head_tree)?;
            let (files, skipped_non_utf8_paths) = git::changes(repo, &base_commit, &head_commit)?;
            Ok(delta::compare(
                &before,
                &after,
                files,
                skipped_non_utf8_paths,
            ))
        })
    }

    /// Cache a validated extension reply, separately from built-in facts. The
    /// namespace identifies the provider configuration and repository; the reply
    /// is reused for commits that share the same tree(s).
    pub fn provider_reply(
        &self,
        namespace: &[u8],
        revision: &Revision,
        base: Option<&Revision>,
        load: impl FnOnce() -> Result<Reply>,
    ) -> Result<Reply> {
        let tree = object_id(revision.tree())?;
        let trees = if let Some(base) = base {
            format!("{}-{tree}", object_id(base.tree())?)
        } else {
            tree
        };
        let namespace = Sha256::digest(namespace);
        self.get("provider-v1", &format!("{namespace:x}-{trees}"), load)
    }

    fn get<T: Serialize + DeserializeOwned>(
        &self,
        category: &str,
        key: &str,
        compute: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let dir = self.directory.join(category);
        let path = dir.join(format!("{key}.json"));
        match File::open(&path) {
            Ok(file) => match serde_json::from_reader(BufReader::new(file)) {
                Ok(facts) => return Ok(facts),
                Err(error) if error.is_io() => {
                    return Err(error).context(format!("read {}", path.display()));
                }
                Err(_) => {} // An interrupted/invalid cache entry is safe to rebuild.
            },
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
        }
        let facts = compute()?;
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)
            .with_context(|| format!("create cache temporary file in {}", dir.display()))?;
        {
            let mut writer = BufWriter::new(tmp.as_file_mut());
            serde_json::to_writer(&mut writer, &facts).context("serialize system map facts")?;
            writer.flush().context("flush system map facts")?;
        }
        tmp.persist(&path)
            .with_context(|| format!("publish {}", path.display()))?;
        Ok(facts)
    }
}

fn object_id(id: &str) -> Result<String> {
    ensure!(
        matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "expected a full Git object ID"
    );
    Ok(id.to_ascii_lowercase())
}
