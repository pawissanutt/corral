//! Collection of idle hostless assistant scratch, and doctor's orphan listing.
use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::Store;
use corral_core::agent::{Executor, GenerationFence};
use corral_core::assistant::{AssistantContainer, AssistantScratch, AssistantSession};
use corral_herdr::PaneInfo;
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;

use super::{git, holder, limits, measure, remove, repo_of};
use crate::Ctx;
use crate::cleanup_worktree::Callers;
use crate::watchdog::{ProcessControl, SystemProcesses};

#[derive(Debug, Clone, Serialize)]
pub struct Holder {
    pub pid: u32,
    pub command: Vec<String>,
    pub age_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GcItem {
    pub session: String,
    pub slug: String,
    pub project: String,
    pub path: PathBuf,
    /// `remove` (dry run), `removed`, `skip` or `failed`.
    pub action: &'static str,
    pub reason: String,
    pub bytes: Option<u64>,
    /// Its work goes (or would go) to session diff artifacts before removal.
    pub preserve: bool,
    pub artifacts: Vec<String>,
    pub holders: Vec<Holder>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub idle_days: i64,
    pub stop_stale: bool,
    /// Why pane checks did not run; resident-process checks still apply.
    pub panes_unavailable: Option<String>,
    pub items: Vec<GcItem>,
}

impl GcReport {
    pub fn failed(&self) -> usize {
        self.items.iter().filter(|i| i.action == "failed").count()
    }
}

struct Sweep<'a> {
    ctx: &'a Ctx,
    now: Timestamp,
    idle_days: i64,
    dry_run: bool,
    stop_stale: bool,
    panes: Vec<PaneInfo>,
}

/// Collect ended or abandoned hostless sessions after the idle threshold. Missing directories
/// can be forgotten immediately, but active finishes and live hosts always remain protected.
/// A dry run lists every record without changing it or signalling any process.
pub fn gc(ctx: &Ctx, now: Timestamp, dry_run: bool) -> Result<GcReport> {
    gc_with_options(ctx, now, dry_run, false)
}

pub fn gc_with_options(
    ctx: &Ctx,
    now: Timestamp,
    dry_run: bool,
    stop_stale: bool,
) -> Result<GcReport> {
    let (panes, panes_unavailable) = match ctx.herdr.pane_list() {
        Ok(panes) => (panes, None),
        Err(error) => (Vec::new(), Some(format!("{error:#}"))),
    };
    let sweep = Sweep {
        ctx,
        now,
        idle_days: limits(ctx).idle_days,
        dry_run,
        stop_stale,
        panes,
    };
    let mut items = Vec::new();
    for session in ctx.store.list_assistants()? {
        let visited: Vec<GcItem> = session
            .scratch
            .iter()
            .map(|scratch| sweep.visit(&session, scratch))
            .collect();
        if !dry_run {
            sweep.record_sizes(&session.id, &visited)?;
        }
        items.extend(visited);
    }
    Ok(GcReport {
        dry_run,
        idle_days: sweep.idle_days,
        stop_stale,
        panes_unavailable,
        items,
    })
}

impl Sweep<'_> {
    fn visit(&self, session: &AssistantSession, scratch: &AssistantScratch) -> GcItem {
        let mut item = GcItem {
            session: session.id.clone(),
            slug: scratch.slug.clone(),
            project: scratch.project.clone(),
            path: scratch.path.clone(),
            action: "skip",
            reason: String::new(),
            bytes: None,
            preserve: false,
            artifacts: Vec::new(),
            holders: Vec::new(),
        };
        if let Err(error) = self.try_visit(session, scratch, &mut item) {
            item.action = "failed";
            item.reason = format!("{error:#}");
        }
        item
    }

    fn try_visit(
        &self,
        session: &AssistantSession,
        scratch: &AssistantScratch,
        item: &mut GcItem,
    ) -> Result<()> {
        let Some(_admission) = self.ctx.store.try_assistant_scratch_gc(&session.id)? else {
            item.reason = "assistant startup or host holds its lifecycle lease".into();
            return Ok(());
        };
        let _guard = super::lock(
            self.ctx,
            &format!("scratch-{}-{}", session.id, scratch.slug),
        )?;
        let session = self.ctx.store.get_assistant(&session.id)?;
        if !session.scratch.iter().any(|s| s.slug == scratch.slug) {
            item.reason = "already removed".into();
            return Ok(());
        }
        let current = super::find(self.ctx, &session.id, &scratch.slug)?;
        let scratch = &current;
        let container = self.ctx.store.get_assistant_container(&session.id)?;
        // Also protect a fenced host that has not actually exited yet.
        for generation in &container.state.generations {
            if let Some(bound) = &generation.bound
                && !crate::agent::bridge::liveness::gone(&Executor {
                    pid: bound.pid,
                    start_time: bound.start_time.clone(),
                })?
            {
                item.reason = format!(
                    "live assistant host pid {} (generation {})",
                    bound.pid, generation.n
                );
                return Ok(());
            }
        }
        let repo = repo_of(scratch)?;
        crate::cleanup_worktree::validate_scratch(&repo, &scratch.path, &scratch.branch)?;
        let missing = !scratch.path.is_dir();
        if !missing {
            item.bytes = Some(measure(&scratch.path)?.total());
        }
        if let Some(holder) = holder(&self.ctx.store.list()?, scratch) {
            item.reason = format!("pending finish: {holder}");
            return Ok(());
        }
        let work = git::work(&repo, scratch)?;
        let mut idle_since = activity(&session, &container).max(scratch.created);
        if let Some(head) = &work.head {
            idle_since = idle_since.max(git::committed_at(&repo, head)?);
        }
        let idle = self.now.duration_since(idle_since);
        let pending_start = container
            .state
            .current_generation()
            .is_some_and(|generation| generation.bound.is_none() && generation.fenced.is_none());
        if (!missing || pending_start) && idle < SignedDuration::from_hours(self.idle_days * 24) {
            item.reason = format!(
                "idle {}d of {}d; {}",
                idle.as_hours() / 24,
                self.idle_days,
                if session.ended.is_some() {
                    "session ended"
                } else if pending_start {
                    "open session; pending host startup"
                } else {
                    "open session; no live host"
                }
            );
            return Ok(());
        }
        let why = if missing {
            "missing directory"
        } else if session.ended.is_some() {
            "idle ended session"
        } else {
            "idle open session; no live host"
        };
        let busy = crate::cleanup_worktree::scratch_residents(&scratch.path, Callers::Occupy)?;
        for process in &busy {
            if let Some(snapshot) = SystemProcesses.inspect(process.pid)?
                && snapshot.started == process.started
            {
                item.holders.push(Holder {
                    pid: process.pid,
                    command: snapshot.argv,
                    age_secs: snapshot.elapsed_secs,
                });
            }
        }
        if !busy.is_empty() && !self.stop_stale {
            item.reason = format!(
                "live process cwd: {:?}; {why}; use --stop-stale to stop holders",
                item.holders
            );
            return Ok(());
        }
        if self.stop_stale && !busy.is_empty() {
            let protected = crate::cleanup_worktree::protected_pids()?;
            if busy.iter().any(|process| protected.contains(&process.pid)) {
                item.reason = format!("protected calling process still uses the scratch; {why}");
                return Ok(());
            }
        }
        if !self.stop_stale
            && let Some(pane) = pane_in(&self.panes, &scratch.path)
        {
            item.reason = format!("pane {pane} is open there; {why}; use --stop-stale to collect");
            return Ok(());
        }
        let Some(_landing) = corral_git::try_landing_lock(&repo)? else {
            item.reason = "landing in progress".into();
            return Ok(());
        };
        item.preserve = work.any();
        if self.dry_run {
            item.action = "remove";
            item.reason = format!(
                "{why}; {}{}",
                if item.preserve {
                    "save bundle/diff artifacts first"
                } else {
                    "nothing beyond its base"
                },
                if busy.is_empty() {
                    ""
                } else {
                    "; would stop listed holders"
                }
            );
            return Ok(());
        }
        // A bootstrap prepared days ago cannot bind after this exclusive claim is released.
        self.ctx
            .store
            .settle_assistant_container(&session.id, |container, _| {
                for generation in &mut container.state.generations {
                    if generation.bound.is_none() {
                        generation.fenced.get_or_insert(GenerationFence {
                            reason: "idle scratch collection".into(),
                            at: self.now,
                        });
                    }
                }
                Ok(())
            })?;
        if !busy.is_empty() {
            crate::cleanup_worktree::stop_scratch_residents(&scratch.path, &busy)?;
        }
        let removed = remove(self.ctx, &session.id, &current, Callers::Occupy)?;
        self.ctx.store.log_event(
            None,
            "assistant_scratch_collected",
            serde_json::json!({
                "session": session.id, "slug": current.slug, "project": current.project,
                "branch": current.branch, "head": removed.head, "artifacts": removed.artifacts,
                "bytes": item.bytes,
            }),
        )?;
        item.action = "removed";
        item.preserve = !removed.artifacts.is_empty();
        item.reason = if item.preserve {
            format!("{why}; work saved as {}", removed.artifacts.join(", "))
        } else {
            format!("{why}; nothing beyond its base")
        };
        item.artifacts = removed.artifacts;
        Ok(())
    }

    fn record_sizes(&self, session: &str, items: &[GcItem]) -> Result<()> {
        let kept: Vec<(&str, u64)> = items
            .iter()
            .filter(|item| item.action != "removed")
            .filter_map(|item| Some((item.slug.as_str(), item.bytes?)))
            .collect();
        if kept.is_empty() {
            return Ok(());
        }
        self.ctx.store.update_assistant(session, |s| {
            for scratch in &mut s.scratch {
                if let Some((_, bytes)) = kept.iter().find(|(slug, _)| *slug == scratch.slug) {
                    scratch.bytes = Some(*bytes);
                    scratch.measured_at = Some(self.now);
                }
            }
            Ok(())
        })?;
        Ok(())
    }
}

fn activity(session: &AssistantSession, container: &AssistantContainer) -> Timestamp {
    let mut last = session
        .ended
        .unwrap_or(session.created)
        .max(session.created);
    for generation in &container.state.generations {
        last = last.max(generation.created);
        if let Some(bound) = &generation.bound {
            last = last.max(bound.at);
        }
    }
    for turn in &container.state.turns {
        for at in [turn.started, turn.ended].into_iter().flatten() {
            last = last.max(at);
        }
    }
    for receipt in &container.state.receipts {
        last = last.max(receipt.committed_at.unwrap_or(receipt.at));
    }
    for input in &container.owner_inputs {
        last = last.max(input.at);
    }
    last
}

fn pane_in(panes: &[PaneInfo], path: &Path) -> Option<String> {
    panes
        .iter()
        .find(|pane| {
            pane.cwd.as_ref().is_some_and(|cwd| {
                cwd.starts_with(path) || cwd.canonicalize().is_ok_and(|c| c.starts_with(path))
            })
        })
        .map(|pane| pane.pane_id.clone())
}

/// `A-<n>` from a scratch directory name `A-<n>-<slug>`.
fn scratch_session(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("A-")?;
    let end = rest.find('-')?;
    (end > 0 && end + 1 < rest.len() && rest[..end].bytes().all(|b| b.is_ascii_digit()))
        .then(|| &name[..2 + end])
}

/// `.worktrees/A-<n>-*` directories that no session record lists: the session record is
/// missing, or it does not name the directory (an interrupted create or a hand-made checkout).
pub fn orphans(store: &Store) -> Result<Vec<String>> {
    let sessions = store.list_assistants()?;
    let repos: BTreeSet<PathBuf> = store
        .config
        .projects
        .keys()
        .filter_map(|project| store.project_path(project).ok()?.canonicalize().ok())
        .collect();
    let mut found = Vec::new();
    for repo in repos {
        let dir = repo.join(".worktrees");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("listing {}", dir.display())),
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name();
            let Some(session) = name.to_str().and_then(scratch_session) else {
                continue;
            };
            if !path.is_dir() {
                continue;
            }
            match sessions.iter().find(|s| s.id == session) {
                None => found.push(format!(
                    "{} (session record {session} is missing)",
                    path.display()
                )),
                Some(s) if !s.scratch.iter().any(|x| x.path == path) => {
                    found.push(format!("{} (not recorded by {session})", path.display()))
                }
                Some(_) => {}
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_directory_names_map_to_their_session() {
        for (name, session) in [
            ("A-1-spike", Some("A-1")),
            ("A-12-spike-cache", Some("A-12")),
            ("A-1-", None),
            ("A--spike", None),
            ("A-x-spike", None),
            ("T-601", None),
            ("A-3", None),
        ] {
            assert_eq!(scratch_session(name), session, "{name}");
        }
    }

    #[test]
    fn a_pane_inside_the_scratch_holds_it() {
        let pane = |id: &str, cwd: &str| PaneInfo {
            pane_id: id.into(),
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            agent: None,
            status: corral_herdr::AgentStatus::Unknown,
            title: None,
            cwd: Some(PathBuf::from(cwd)),
        };
        let panes = [
            pane("w1:p1", "/repo"),
            pane("w1:p2", "/repo/.worktrees/A-1-x/src"),
        ];
        assert_eq!(
            pane_in(&panes, Path::new("/repo/.worktrees/A-1-x")),
            Some("w1:p2".into())
        );
        assert_eq!(pane_in(&panes, Path::new("/repo/.worktrees/A-1-y")), None);
    }
}
