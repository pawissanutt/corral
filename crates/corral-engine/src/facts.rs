//! Project facts for background readers. No engine operation calls a provider.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use corral_facts::{
    FactsCache, ProjectDelta, ProjectFacts, Revision,
    provider::{ProviderCoverage, Reply},
    resolve_revision,
};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    Config,
    config::{Project, Slot},
    extension::{self, CommandContext},
};

const SNAPSHOT_WANT: &[&str] = &["files", "definitions", "references"];
const DELTA_WANT: &[&str] = &["changes", "dependents", "tests"];
const TIMEOUT: Duration = Duration::from_secs(600);
const MAX_STDOUT: usize = 64 * 1024 * 1024;

/// Built-in reads and provider enrichment are separate so the daemon can
/// publish the built-in result before calling an extension.
pub struct FactsReader<'a> {
    root: PathBuf,
    repo: PathBuf,
    cache: FactsCache,
    project: &'a str,
    settings: &'a Project,
    provider: Option<(&'a str, &'a str)>,
    timeout: Duration,
    max_stdout: usize,
}

#[derive(Debug, Serialize)]
pub struct FactsReport {
    pub extension: String,
    pub coverage: ProviderCoverage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<'a> FactsReader<'a> {
    pub fn new(root: &Path, config: &'a Config, project: &'a str) -> Result<Self> {
        let settings = config
            .projects
            .get(project)
            .with_context(|| format!("unknown project {project:?}"))?;
        let root = std::path::absolute(root).context("resolve workspace path for facts")?;
        Ok(Self {
            repo: root.join(&settings.path),
            cache: FactsCache::new(&root),
            root,
            project,
            settings,
            provider: config.extension_for_slot(project, Slot::Facts),
            timeout: TIMEOUT,
            max_stdout: MAX_STDOUT,
        })
    }

    pub fn resolve_revision(&self, rev: &str) -> Result<Revision> {
        resolve_revision(&self.repo, rev)
    }

    pub fn builtin_snapshot(&self, rev: &Revision) -> Result<ProjectFacts> {
        self.cache.project_facts(&self.repo, rev)
    }

    pub fn builtin_delta(&self, base: &Revision, head: &Revision) -> Result<ProjectDelta> {
        self.cache.delta(&self.repo, base, head)
    }

    /// Apply the optional provider to a fresh built-in snapshot. None means no
    /// provider was configured; a failed reply leaves the snapshot untouched.
    pub fn enrich_snapshot(&self, snapshot: &mut ProjectFacts) -> Option<FactsReport> {
        self.ask(&snapshot.revision, None, SNAPSHOT_WANT, |reply| {
            Ok(reply.merge_snapshot(&mut snapshot.structure))
        })
    }

    /// Apply the optional provider to a fresh built-in delta for base and head.
    pub fn enrich_delta(
        &self,
        base: &Revision,
        head: &Revision,
        delta: &mut ProjectDelta,
    ) -> Option<FactsReport> {
        self.ask(head, Some(base), DELTA_WANT, |reply| {
            let before = self.cache.tree_facts(&self.repo, base.tree())?;
            let after = self.cache.tree_facts(&self.repo, head.tree())?;
            Ok(reply.merge_delta(delta, &before, &after))
        })
    }

    fn ask(
        &self,
        revision: &Revision,
        base: Option<&Revision>,
        want: &[&str],
        merge: impl FnOnce(Reply) -> Result<ProviderCoverage>,
    ) -> Option<FactsReport> {
        let (name, line) = self.provider?;
        let result = (|| {
            // Do not share provider answers across projects or command/environment
            // changes, even for the same tree. This key is hashed by FactsCache.
            let namespace = serde_json::to_vec(&(
                &self.repo,
                self.project,
                name,
                line,
                &self.settings.env,
                want,
            ))
            .context("encode facts provider cache namespace")?;
            let reply = self.cache.provider_reply(&namespace, revision, base, || {
                let mut request = Map::from_iter([
                    (
                        "repo".into(),
                        serde_json::to_value(&self.repo).context("encode facts repository path")?,
                    ),
                    (
                        "want".into(),
                        serde_json::to_value(want).context("encode facts categories")?,
                    ),
                ]);
                if let Some(base) = base {
                    request.insert("query".into(), Value::from("delta"));
                    request.insert("base".into(), Value::from(base.commit()));
                    request.insert("head".into(), Value::from(revision.commit()));
                } else {
                    request.insert("query".into(), Value::from("snapshot"));
                    request.insert("rev".into(), Value::from(revision.commit()));
                }
                let context = CommandContext {
                    root: &self.root,
                    cwd: &self.repo,
                    project: Some(self.project),
                    extension: name,
                    env: &self.settings.env,
                };
                let output =
                    extension::run(line, request, &context, self.timeout, self.max_stdout)?;
                Reply::parse(&output, want)
            })?;
            merge(reply)
        })();
        let (coverage, error) = match result {
            Ok(coverage) => (coverage, None),
            Err(error) => (
                ProviderCoverage::unavailable(want),
                Some(format!("{error:#}")),
            ),
        };
        Some(FactsReport {
            extension: name.to_owned(),
            coverage,
            error,
        })
    }
}

#[cfg(all(test, unix))]
#[path = "facts_tests.rs"]
mod tests;
