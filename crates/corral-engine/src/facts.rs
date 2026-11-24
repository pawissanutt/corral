//! Project facts for background readers. No engine operation calls a provider.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use corral_facts::{
    EntryPoint, EntryPointDelta, EntryPoints, FactsCache, ProjectDelta, ProjectFacts, Revision,
    TreeFacts, compare_entry_points,
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

const SNAPSHOT_WANT: &[&str] = &["files", "definitions", "references", "entries"];
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
    base: Option<&'a str>,
    provider: Option<(&'a str, &'a str)>,
    timeout: Duration,
    max_stdout: usize,
    cancelled: Option<&'a dyn Fn() -> bool>,
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
            cache: FactsCache::new(&root).with_operation_enums(&settings.operation_enums),
            root,
            project,
            settings,
            base: settings.base.as_deref().or(config.defaults.base.as_deref()),
            provider: config.extension_for_slot(project, Slot::Facts),
            timeout: TIMEOUT,
            max_stdout: MAX_STDOUT,
            cancelled: None,
        })
    }

    /// Allow a background reader to terminate its provider during shutdown.
    pub fn with_cancellation(mut self, cancelled: &'a dyn Fn() -> bool) -> Self {
        self.cancelled = Some(cancelled);
        self
    }

    /// Use the same project base as dispatch. Detached projects without an
    /// inferred base can still be inspected at their checkout HEAD.
    pub fn base_branch(&self) -> Result<Option<String>> {
        match self.base {
            Some(branch) => Ok(Some(branch.to_owned())),
            None => corral_git::default_branch(&self.repo)
                .with_context(|| format!("discover default branch for project {}", self.project)),
        }
    }

    pub fn project_tip(&self, branch: Option<&str>) -> Result<String> {
        let reference = match branch {
            Some(branch) => {
                corral_git::validate_branch_name(branch).context("validate project base branch")?;
                format!("refs/heads/{branch}")
            }
            None => "HEAD".into(),
        };
        corral_git::rev_sha(&self.repo, &reference).context("resolve project base")
    }

    pub fn resolve_revision(&self, rev: &str) -> Result<Revision> {
        resolve_revision(&self.repo, rev)
    }

    /// Older done events recorded only the head. A landing attempt may have
    /// moved the run's recorded base; limit it to the history of that head.
    pub fn legacy_submitted_base(&self, base: &str, head: &str) -> Result<String> {
        corral_git::merge_base(&self.repo, base, head).context("resolve legacy submission base")
    }

    pub fn builtin_tree(&self, rev: &Revision) -> Result<TreeFacts> {
        self.cache.tree_facts(&self.repo, rev.tree())
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
            let before = self.builtin_tree(base)?;
            let after = (base.tree() != head.tree())
                .then(|| self.builtin_tree(head))
                .transpose()?;
            Ok(reply.merge_delta(delta, &before, after.as_ref().unwrap_or(&before)))
        })
    }

    /// Compare provider-overlaid snapshots independently of the other delta categories.
    /// None leaves the caller's built-in entry delta intact.
    pub fn enrich_entry_delta(
        &self,
        (base, before): (&Revision, &TreeFacts),
        (head, after): (&Revision, &TreeFacts),
        validate: impl Fn(&EntryPoint) -> Result<()>,
    ) -> Option<EntryPointDelta> {
        // Reuse the runtime view's immutable snapshot requests without loading activity.
        // Never compare one provider snapshot against the other side after a failure.
        let (old, base_provided) = self.provider_entries(base, before, &validate)?;
        if base.tree() == head.tree() {
            return base_provided.then(|| compare_entry_points(&old, &old));
        }
        let (new, head_provided) = self.provider_entries(head, after, &validate)?;
        (base_provided || head_provided).then(|| compare_entry_points(&old, &new))
    }

    fn provider_entries(
        &self,
        rev: &Revision,
        tree: &TreeFacts,
        validate: &impl Fn(&EntryPoint) -> Result<()>,
    ) -> Option<(EntryPoints, bool)> {
        let mut result = None;
        self.ask(rev, None, SNAPSHOT_WANT, |reply| {
            let (entries, coverage) = reply.snapshot_entries(tree);
            let provided = coverage.provides("entries");
            if provided {
                entries.entries.iter().try_for_each(validate)?;
            }
            result = Some((entries, provided));
            Ok(coverage)
        })?;
        result
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
                    hold: None,
                };
                let output = match self.cancelled {
                    Some(cancelled) => extension::run_cancellable(
                        line,
                        request,
                        &context,
                        self.timeout,
                        self.max_stdout,
                        cancelled,
                    ),
                    None => extension::run(line, request, &context, self.timeout, self.max_stdout),
                }?;
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
