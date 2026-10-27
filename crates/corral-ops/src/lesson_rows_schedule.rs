//! Keyed scheduling facts. A request changes one project generation, not every task row.
//! Per-task counters retain one generation per project so moving a task preserves the
//! full-task oracle's attribution to its *current* project.

use std::collections::BTreeSet;
use std::path::Path;

use super::*;

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Project {
    pub requested: Option<Event>,
    pub history: usize,
    pub landings: usize,
    pub failures: usize,
}

impl Project {
    fn generation(&self) -> u64 {
        self.requested.as_ref().map_or(0, |event| event.seq)
    }

    pub fn due(&self) -> DueRow {
        DueRow {
            requested: self.requested.clone(),
            has_history: self.history > 0,
            landings: self.landings,
            failures: self.failures,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Counts {
    generation: u64,
    landings: usize,
    failures: usize,
}

fn counts_path(root: &Path, project: &str, id: &str) -> PathBuf {
    root.join("counts")
        .join(key(project))
        .join(format!("{}.json", key(id)))
}

/// Adjust constant-size project totals on task creation, deletion, move or history change.
pub(super) fn task_changed(
    root: &Path,
    id: &str,
    before: Option<&Meta>,
    after: Option<&Meta>,
) -> Result<()> {
    for (meta, adding) in [(before, false), (after, true)] {
        let Some(meta) = meta else { continue };
        let Some(project) = &meta.project else {
            continue;
        };
        let path = project_path(root, project);
        let mut row: Project = read(&path)?.unwrap_or_default();
        let counts: Option<Counts> = read(&counts_path(root, project, id))?;
        let tally = counts
            .as_ref()
            .filter(|counts| counts.generation == row.generation());
        let landings = tally.map_or(0, |counts| counts.landings);
        let failures = tally.map_or(0, |counts| counts.failures);
        if adding {
            row.history += usize::from(meta.history);
            row.landings += landings;
            row.failures += failures;
        } else {
            row.history -= usize::from(meta.history);
            row.landings -= landings;
            row.failures -= failures;
        }
        save(&path, &row)?;
    }
    Ok(())
}

pub(super) fn apply(root: &Path, projects: &BTreeSet<String>, event: &Event) -> Result<()> {
    if event.kind == "lessons_due" {
        if let Some(project) = event.detail["project"].as_str() {
            save(
                &announcement_path(root, project, event.detail["request"].as_str()),
                &true,
            )?;
        }
        return Ok(());
    }
    if event.kind == "lessons_distill_requested" {
        if let Some(project) = event.detail["project"].as_str() {
            let path = project_path(root, project);
            let mut row: Project = read(&path)?.unwrap_or_default();
            row.requested = Some(event.clone());
            row.landings = 0;
            row.failures = 0;
            save(&path, &row)?;
        }
        return Ok(());
    }
    let Some(id) = &event.task else {
        return Ok(());
    };
    let landing = event.kind == "approved"
        && matches!(
            event.detail["strategy"].as_str(),
            Some("fast_forward" | "rebase")
        );
    let failure = (event.kind == "gate" && event.detail["result"] == "fail")
        || matches!(event.kind.as_str(), "changes_requested" | "discarded")
        || corral_core::actionable_kind(event) == Some("escalated");
    if !landing && !failure {
        return Ok(());
    }
    let meta: Option<Meta> = read(&task_path(root, id))?;
    for project in projects {
        let project_path = project_path(root, project);
        let mut row: Project = read(&project_path)?.unwrap_or_default();
        let path = counts_path(root, project, id);
        let mut count: Counts = read(&path)?.unwrap_or_default();
        if count.generation != row.generation() {
            count = Counts {
                generation: row.generation(),
                ..Counts::default()
            };
        }
        count.landings += usize::from(landing);
        count.failures += usize::from(failure);
        save(&path, &count)?;
        if meta.as_ref().and_then(|meta| meta.project.as_ref()) == Some(project) {
            row.landings += usize::from(landing);
            row.failures += usize::from(failure);
            save(&project_path, &row)?;
        }
    }
    Ok(())
}
