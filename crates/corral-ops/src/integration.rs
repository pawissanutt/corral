//! Factual integration context. Semantic advice belongs to the attributed reviewer, not Git.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use corral_core::{Artifact, ArtifactKind, ReviewPolicy, RunHead, RunOutcome, Status, TaskRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::gate_tier::GatePhase;
use crate::{Ctx, verification};
use corral_core::records::RecordArea;
#[cfg(test)]
use corral_core::{Run, Task};

const PACKETS: RecordArea<IntegrationPacket> = RecordArea::new("integration");

mod size_limits;
pub(crate) use size_limits::effective_size_limits;

#[cfg(test)]
mod tests;

/// The workspace's persistent Git fact memo: `$XDG_CACHE_HOME/corral/<workspace-id>/cache/git`
/// (falling back to `$HOME/.cache`), where workspace-id is the first 16 hex digits of the SHA-256
/// of the canonical workspace root. Disabled when neither directory is an absolute path.
pub(crate) fn git_memo(ctx: &Ctx) -> corral_git::Memo {
    let absolute = |name| {
        std::env::var_os(name)
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let Some(cache) =
        absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|h| h.join(".cache")))
    else {
        return corral_git::Memo::disabled();
    };
    let root = ctx.store.root();
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
    let id: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    corral_git::Memo::new(cache.join("corral").join(id).join("cache").join("git"))
}

#[derive(Debug)]
pub(crate) struct StalePacket;

impl std::fmt::Display for StalePacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stale_retry: integration packet task, run, head or base changed")
    }
}

impl std::error::Error for StalePacket {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RepairKey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<corral_core::CompletionDriver>,
    pub task: String,
    pub run: String,
    pub head: String,
    pub base: String,
    pub reason: String,
}

impl RepairKey {
    pub fn new<T: TaskRef + ?Sized>(task: &T, run: &RunHead, reason: &str) -> Result<Self> {
        Ok(Self {
            driver: Self::driver(task),
            task: task.id().clone(),
            run: run.id.clone(),
            head: corral_git::rev_sha(&run.worktree, "HEAD")?,
            base: corral_git::rev_sha(&run.worktree, &run.base)?,
            reason: reason.to_string(),
        })
    }

    pub fn digest(&self) -> Result<String> {
        // Ordinary legacy board preparations keep their exact receipt identity. A scoped
        // completion's driver/run replacement is not another code repair; live run/base/driver
        // still fence submission in check(), without changing its issued cycle identity.
        if self.driver.is_none() {
            return Ok(verification::digest(&serde_json::to_vec(self)?));
        }
        Ok(verification::digest(&serde_json::to_vec(&(
            &self.task,
            &self.head,
            &self.reason,
        ))?))
    }

    pub(crate) fn driver<T: TaskRef + ?Sized>(task: &T) -> Option<corral_core::CompletionDriver> {
        task.cowork_run().map(|cowork| {
            cowork.completion.as_ref().map_or_else(
                || corral_core::CompletionDriver::Assistant {
                    session: cowork.session.clone(),
                    scratch: cowork.scratch.clone(),
                },
                |scope| scope.driver.clone(),
            )
        })
    }

    /// Called inside the task write lock as well as before any external delivery.
    pub fn check<T: TaskRef + ?Sized>(&self, task: &T) -> Result<()> {
        let Some(run) = task
            .current_run_head()?
            .filter(|r| *task.id() == self.task && r.id == self.run)
        else {
            return Err(StalePacket.into());
        };
        if Self::driver(task) != self.driver
            || *task.status() != Status::Review
            || run.outcome != Some(RunOutcome::Done)
            || corral_git::rev_sha(&run.worktree, "HEAD")? != self.head
            || corral_git::rev_sha(&run.worktree, &run.base)? != self.base
        {
            return Err(StalePacket.into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LandedTask {
    pub id: String,
    pub title: Option<String>,
    pub run: Option<String>,
    pub approved_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LandedCommit {
    pub sha: String,
    pub subject: String,
    /// Empty means unattributed: subjects and branch names are never attribution evidence.
    pub tasks: Vec<LandedTask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileSize {
    pub path: String,
    pub candidate_sloc: Option<usize>,
    pub main_sloc: Option<usize>,
    pub ceiling: Option<usize>,
}

impl FileSize {
    pub fn at(
        repo: &std::path::Path,
        key: &RepairKey,
        path: String,
        size_limits: &BTreeMap<String, usize>,
    ) -> Result<Self> {
        let count = |sha: &str| -> Result<Option<usize>> {
            Ok(corral_git::read_blob(repo, sha, &path)?.map(|text| {
                text.lines()
                    .filter(|line| {
                        let line = line.trim();
                        !line.is_empty() && !line.starts_with("//")
                    })
                    .count()
            }))
        };
        Ok(Self {
            candidate_sloc: count(&key.head)?,
            main_sloc: count(&key.base)?,
            ceiling: std::path::Path::new(&path)
                .extension()
                .and_then(|extension| extension.to_str())
                .and_then(|extension| size_limits.get(extension))
                .copied(),
            path,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewerNote {
    pub reviewer: String,
    pub sha: String,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IntegrationPacket {
    pub key: RepairKey,
    pub dispatch_base: String,
    pub merge_base: String,
    /// Historical drift since dispatch, including commits the worker has already integrated.
    pub dispatch_history: Vec<LandedCommit>,
    /// Everything reachable from main but not the candidate, regardless of dispatch history.
    pub missing_from_candidate: Vec<LandedCommit>,
    pub conflict_paths: Vec<String>,
    pub candidate_changed_paths: Vec<String>,
    pub main_changed_paths: Vec<String>,
    pub overlaps: Vec<String>,
    pub file_sizes: Vec<FileSize>,
    pub sloc_method: String,
    pub gate_commands: Vec<String>,
    pub git_details: Option<String>,
    pub reviewer_note: Option<ReviewerNote>,
}

impl IntegrationPacket {
    pub fn build<T: TaskRef + ?Sized>(
        ctx: &Ctx,
        task: &T,
        run: &RunHead,
        policy: &ReviewPolicy,
        key: RepairKey,
        mut conflict_paths: Vec<String>,
        git_details: Option<String>,
    ) -> Result<Self> {
        let _hot = corral_core::work::HotScope::enter();
        let repo = &run.worktree;
        let memo = git_memo(ctx);
        let merge_base = memo.merge_base(repo, &key.base, &key.head)?;
        let dispatch_commits = corral_git::commits_between(repo, &run.base_sha, &key.base)?;
        let missing_commits = corral_git::commits_between(repo, &key.head, &key.base)?;
        let shas = dispatch_commits
            .iter()
            .chain(&missing_commits)
            .map(|commit| commit.sha.clone())
            .collect();
        // Git may take longer than a view lease. Pin only the attribution lookup,
        // not the subprocess work before and after it.
        // Like the original packet boundary, acquire the view outside its internal
        // HotScope; caller-owned scopes still count acquisition. Only DB/Git work is
        // measured here, equally for live and explicitly frozen callers.
        drop(_hot);
        let approvals = ctx.read_step(|read| {
            let _hot = corral_core::work::HotScope::enter();
            crate::approvals::prepare(read)?;
            crate::approvals::lookup(read, task.project(), &shas)
        })?;
        let _hot = corral_core::work::HotScope::enter();
        let commits = |commits: Vec<corral_git::Commit>| -> Result<Vec<LandedCommit>> {
            commits
                .into_iter()
                .map(|c| {
                    let tasks = approvals.get(&c.sha).cloned().unwrap_or_default();
                    Ok(LandedCommit {
                        sha: c.sha,
                        subject: c.subject,
                        tasks,
                    })
                })
                .collect()
        };
        let dispatch_history = commits(dispatch_commits)?;
        let missing_from_candidate = commits(missing_commits)?;
        let candidate_changed_paths = memo.changed_paths_between(repo, &merge_base, &key.head)?;
        let main_changed_paths = memo.changed_paths_between(repo, &merge_base, &key.base)?;
        let candidate_paths: BTreeSet<_> = candidate_changed_paths.iter().cloned().collect();
        let main_paths: BTreeSet<_> = main_changed_paths.iter().cloned().collect();
        let overlaps = candidate_paths.intersection(&main_paths).cloned().collect();
        conflict_paths.sort();
        conflict_paths.dedup();
        let paths: BTreeSet<_> = candidate_paths
            .union(&main_paths)
            .cloned()
            .chain(conflict_paths.iter().cloned())
            .collect();
        let mut file_sizes = Vec::new();
        let size_limits = effective_size_limits(ctx, task, repo, policy)?;
        for path in paths {
            file_sizes.push(FileSize::at(repo, &key, path, &size_limits)?);
        }
        let reviewer_note = run
            .review
            .as_ref()
            .filter(|r| r.sha == key.head)
            .and_then(|r| {
                r.notes.as_ref().map(|notes| ReviewerNote {
                    reviewer: r.reviewer.clone(),
                    sha: r.sha.clone(),
                    notes: notes.clone(),
                })
            });
        // Legacy runs keep the full gate; tiered runs list their contract's review-phase
        // commands so no full-tier command reaches a reduced-tier run.
        let gate_commands = if run.gate_rules.is_legacy() {
            policy.gate.clone()
        } else {
            crate::gate_tier::select(ctx, task, run, GatePhase::Review)?.commands
        };
        Ok(Self {
            key,
            dispatch_base: run.base_sha.clone(),
            merge_base,
            dispatch_history,
            missing_from_candidate,
            conflict_paths,
            candidate_changed_paths,
            main_changed_paths,
            overlaps,
            file_sizes,
            sloc_method: "Nonblank lines excluding whole-line // comments; null means absent or binary. Ceilings follow the effective project extension limits.".into(),
            gate_commands,
            git_details,
            reviewer_note,
        })
    }

    pub fn compact(&self) -> String {
        let landed: BTreeSet<_> = self
            .missing_from_candidate
            .iter()
            .flat_map(|c| c.tasks.iter().map(|t| &t.id))
            .collect();
        let unattributed = self
            .missing_from_candidate
            .iter()
            .filter(|c| c.tasks.is_empty())
            .count();
        format!(
            "{} conflict paths; {} overlapping paths; {} missing commits ({} recorded tasks, {} unattributed). Full identities, dispatch history, sizes and gate commands are in the packet",
            self.conflict_paths.len(),
            self.overlaps.len(),
            self.missing_from_candidate.len(),
            landed.len(),
            unattributed
        )
    }

    pub fn attach(&self, ctx: &Ctx) -> Result<Artifact> {
        let digest = self.key.digest()?;
        PACKETS.save(&ctx.store, &digest, self)?;
        let file = ctx
            .store
            .scratch_tmp()?
            .join("integration")
            .join(format!("{digest}.json"));
        std::fs::create_dir_all(
            file.parent()
                .context("integration artifact has no parent")?,
        )
        .context("creating integration packet directory")?;
        // No timestamp or abbreviated/truncated data: identical inputs produce identical artifacts.
        std::fs::write(&file, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", file.display()))?;
        crate::artifacts::add_artifact_for_run(
            ctx,
            &self.key.task,
            &self.key.run,
            &file,
            Some(ArtifactKind::Report),
            Some("Integration repair packet"),
        )
    }
}

#[cfg(test)]
fn approvals(ctx: &Ctx, candidate: &Task) -> Result<BTreeMap<String, Vec<LandedTask>>> {
    let tasks: BTreeMap<_, _> = ctx
        .store
        .list()?
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    let mut approvals: BTreeMap<String, Vec<LandedTask>> = BTreeMap::new();
    for event in ctx.store.events_of(None, &["approved"])?.iter() {
        if event.kind != "approved" || event.detail["strategy"].as_str() == Some("keep_branch") {
            continue;
        }
        let (Some(id), Some(sha)) = (event.task.as_ref(), event.detail["sha"].as_str()) else {
            continue;
        };
        let task = tasks.get(id);
        if task.is_some_and(|t| t.project != candidate.project) {
            continue;
        }
        let landed = LandedTask {
            id: id.clone(),
            title: task.map(|t| t.title.clone()),
            run: event.detail["run"].as_str().map(str::to_string),
            approved_sha: sha.to_string(),
        };
        let rows = approvals.entry(sha.to_string()).or_default();
        if !rows.contains(&landed) {
            rows.push(landed);
        }
    }
    for rows in approvals.values_mut() {
        rows.sort_by(|a, b| (&a.id, &a.run).cmp(&(&b.id, &b.run)));
    }
    Ok(approvals)
}

#[cfg(test)]
mod derived_rows_tests {
    use super::*;
    use corral_core::Store;
    use corral_core::work::{self, Counter};
    use serde_json::json;
    use std::path::Path;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = crate::test_support::command("git", repo)
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn derived_approval_packet_matches_full_board_oracle_with_landed_git_commits() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("base.rs"), "fn base() {}\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "initial"]);
        let initial = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "-qb", "candidate"]);
        std::fs::write(repo.join("candidate.rs"), "fn candidate() {}\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "candidate"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("landed.rs"), "fn landed() {}\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "landed"]);
        let base = git(&repo, &["rev-parse", "HEAD"]);
        let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
        let task = ctx
            .store
            .create("Candidate", Some("app"), Status::Review, "")
            .unwrap();
        let landed = ctx
            .store
            .create("Landed", Some("app"), Status::Done, "")
            .unwrap();
        let other = ctx
            .store
            .create("Other repo", Some("other"), Status::Done, "")
            .unwrap();
        for id in [&landed.id, &other.id] {
            ctx.store
                .log_event(
                    Some(id),
                    "approved",
                    json!({"sha":base,"previous_sha":initial,"strategy":"rebase","run":"R1"}),
                )
                .unwrap();
        }
        // Duplicate approvals and unknown historical task identities retain baseline rules.
        ctx.store
            .log_event(
                Some(&landed.id),
                "approved",
                json!({"sha":base,"strategy":"rebase","run":"R1"}),
            )
            .unwrap();
        ctx.store
            .log_event(
                Some("T-999"),
                "approved",
                json!({"sha":base,"strategy":"rebase","run":"R2"}),
            )
            .unwrap();
        ctx.store
            .log_event(
                Some(&landed.id),
                "approved",
                json!({"sha":head,"strategy":"keep_branch","run":"R3"}),
            )
            .unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1","agent":"worker","worktree":repo,"branch":"candidate","base":"main",
            "base_sha":initial,"started":"2026-10-01T00:00:00Z","outcome":"done"
        }))
        .unwrap();
        let key = RepairKey {
            driver: RepairKey::driver(&task),
            task: task.id.clone(),
            run: run.id.clone(),
            head,
            base: base.clone(),
            reason: "merge".into(),
        };
        let policy = ReviewPolicy::default();
        crate::approvals::prepare(&ctx).unwrap();
        let oracle = approvals(&ctx, &task).unwrap();
        let packet = IntegrationPacket::build(
            &ctx,
            &task,
            &corral_core::RunHead::of(&run).unwrap(),
            &policy,
            key.clone(),
            vec![],
            None,
        )
        .unwrap();
        for commit in packet
            .dispatch_history
            .iter()
            .chain(&packet.missing_from_candidate)
        {
            assert_eq!(
                commit.tasks,
                oracle.get(&commit.sha).cloned().unwrap_or_default()
            );
        }
        assert_eq!(packet.dispatch_history.len(), 1);
        assert_eq!(packet.dispatch_history[0].tasks.len(), 2);
        let measure = || {
            let before = work::snapshot();
            let result = IntegrationPacket::build(
                &ctx,
                &task,
                &corral_core::RunHead::of(&run).unwrap(),
                &policy,
                key.clone(),
                vec![],
                None,
            )
            .unwrap();
            let counts = work::snapshot().since(&before);
            counts.check_identity().unwrap();
            assert_eq!(counts.unattributed_hydrations(), 0);
            (result, counts)
        };
        let (base_packet, base_counts) = measure();
        for i in 0..10 {
            let mut old = ctx
                .store
                .create("Unrelated", Some("app"), Status::Done, "")
                .unwrap();
            old.runs.extend((0..10).map(|_| run.clone()));
            ctx.store.fixture_put_task(&old).unwrap();
            ctx.store
                .log_event(Some(&old.id), "unrelated", json!({"i":i}))
                .unwrap();
        }
        crate::approvals::prepare(&ctx).unwrap();
        let (grown_packet, grown_counts) = measure();
        assert_eq!(base_packet, grown_packet);
        for counter in Counter::ALL {
            assert_eq!(
                base_counts.get(counter),
                grown_counts.get(counter),
                "{counter:?}"
            );
        }
        assert_eq!(base_counts.get(Counter::EventDecodes), 0);
        let recomputed = crate::approvals::recomputations(&ctx).unwrap();
        for area in ["resources", "condition-counts"] {
            corral_core::records::RecordArea::new(area)
                .save(&ctx.store, "observation", &json!({"count":10}))
                .unwrap();
            assert_eq!(measure().0, packet);
            assert_eq!(crate::approvals::recomputations(&ctx).unwrap(), recomputed);
        }
        std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();
        crate::approvals::prepare(&ctx).unwrap();
        assert_eq!(measure().0, packet);
        ctx.store
            .update(&landed.id, |task| {
                task.title = "Renamed".into();
                Ok(())
            })
            .unwrap();
        let packet = measure().0;
        assert_eq!(
            packet.dispatch_history[0].tasks[0].title.as_deref(),
            Some("Renamed")
        );
        // A newly appended approval of a selected SHA changes attribution, not just the
        // cursor. The production packet boundary must consume it without manual preparation.
        ctx.store
            .log_event(
                Some(&landed.id),
                "approved",
                json!({"sha":base,"strategy":"rebase","run":"R4"}),
            )
            .unwrap();
        let updated = measure().0;
        let oracle = approvals(&ctx, &task).unwrap();
        for commit in updated
            .dispatch_history
            .iter()
            .chain(&updated.missing_from_candidate)
        {
            assert_eq!(commit.tasks, oracle[&commit.sha]);
        }
        assert_ne!(updated, packet);
    }
}
