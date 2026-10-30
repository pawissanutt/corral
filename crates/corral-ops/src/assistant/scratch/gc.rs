//! Owner-run collection of idle scratch from ended sessions, and doctor's orphan listing.
use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::assistant::{AssistantScratch, AssistantSession};
use corral_core::{Store, Task};
use corral_herdr::PaneInfo;
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;

use super::{git, holder, limits, measure, remove, repo_of};
use crate::Ctx;
use crate::cleanup_worktree::Callers;

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
}

#[derive(Debug, Clone, Serialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub idle_days: i64,
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
    tasks: Vec<Task>,
    panes: Vec<PaneInfo>,
}

/// A scratch is collected once its session has ended and it has been idle (since the later of
/// the session end and its newest commit) for the configured days, unless a task or co-work
/// finish holds it, a process or pane has its cwd there, or a landing runs in its repository.
/// A dry run measures and lists without changing anything.
pub fn gc(ctx: &Ctx, now: Timestamp, dry_run: bool) -> Result<GcReport> {
    let (panes, panes_unavailable) = match ctx.herdr.pane_list() {
        Ok(panes) => (panes, None),
        Err(error) => (Vec::new(), Some(format!("{error:#}"))),
    };
    let sweep = Sweep {
        ctx,
        now,
        idle_days: limits(ctx).idle_days,
        dry_run,
        tasks: ctx.store.list()?,
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
        if scratch.path.is_dir() {
            item.bytes = Some(measure(&scratch.path)?.total());
        }
        let Some(ended) = session.ended else {
            item.reason = "session open".into();
            return Ok(());
        };
        let repo = repo_of(scratch)?;
        let work = git::work(&repo, scratch)?;
        let mut idle_since = ended;
        if let Some(head) = &work.head {
            idle_since = idle_since.max(git::committed_at(&repo, head)?);
        }
        let idle = self.now.duration_since(idle_since);
        if idle < SignedDuration::from_hours(self.idle_days * 24) {
            item.reason = format!("idle {}d of {}d", idle.as_hours() / 24, self.idle_days);
            return Ok(());
        }
        if let Some(holder) = holder(&self.tasks, scratch) {
            item.reason = format!("pending finish: {holder}");
            return Ok(());
        }
        // The shell or process running GC counts too: its directory is still in use.
        let busy = crate::cleanup_worktree::scratch_residents(&scratch.path, Callers::Occupy)?;
        if !busy.is_empty() {
            item.reason = format!("live process cwd: {busy:?}");
            return Ok(());
        }
        if let Some(pane) = pane_in(&self.panes, &scratch.path) {
            item.reason = format!("pane {pane} is open there");
            return Ok(());
        }
        item.preserve = work.any();
        if self.dry_run {
            item.action = "remove";
            item.reason = if item.preserve {
                "idle; its work would be saved as a diff artifact first".into()
            } else {
                "idle; nothing beyond its base".into()
            };
            return Ok(());
        }
        let Some(_landing) = corral_git::try_landing_lock(&repo)? else {
            item.reason = "landing in progress".into();
            return Ok(());
        };
        let _guard = super::lock(
            self.ctx,
            &format!("scratch-{}-{}", session.id, scratch.slug),
        )?;
        // Re-read under the lock: a concurrent discard may have removed it already.
        let Ok(current) = super::find(self.ctx, &session.id, &scratch.slug) else {
            item.reason = "already removed".into();
            return Ok(());
        };
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
            format!("idle; work saved as {}", removed.artifacts.join(", "))
        } else {
            "idle; nothing beyond its base".into()
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
