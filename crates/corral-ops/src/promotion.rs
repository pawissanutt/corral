//! Read-only, source-backed eligibility and ranking for Idea promotion.
use std::collections::{BTreeSet, HashSet};

use anyhow::Result;
use corral_core::{Status, TaskSize};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[cfg(test)]
#[path = "promotion_tests.rs"]
mod tests;

/// Eligible Idea and the persisted inputs behind its rank and overlap prediction.
/// Eligibility does not authorize a move or a launch; predicted overlaps exclude Ideas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionCandidate {
    pub id: String,
    pub why: String,
    /// IDs of active tasks with intersecting declared or historically inferred paths.
    pub predicted_conflicts: Vec<String>,
    /// False when this Idea or any compared active task has no inferable paths.
    pub overlap_checked: bool,
    /// One-based order, by priority, oldest creation, conflict risk, then ID.
    pub rank: usize,
    pub project: String,
    pub priority: String,
    pub created: Timestamp,
    pub size: Option<TaskSize>,
    pub acceptance: String,
    pub plan_required: bool,
    pub labels: Vec<String>,
    /// Every dependency was observed Done.
    pub depends_on: Vec<String>,
    pub write_scope: Vec<String>,
    pub likely_files: Vec<String>,
    pub compared_tasks: Vec<String>,
    pub unchecked_tasks: Vec<String>,
    pub overlap_evidence: Vec<String>,
}

/// All eligible Ideas, irrespective of promotion mode or current free capacity.
/// Reads tasks and bounded historical lesson signals; never mutates board state.
pub fn promotion_shortlist(ctx: &Ctx) -> Result<Vec<PromotionCandidate>> {
    let tasks = ctx.store.list()?;
    let mut out = Vec::new();
    for task in tasks.iter().filter(|t| t.status == Status::Idea) {
        let Some(project) = task.project.as_deref() else {
            continue;
        };
        if !ctx.store.config.projects.contains_key(project)
            || matches!(task.size, Some(TaskSize::L | TaskSize::Xl))
            || task.design
            || ctx
                .store
                .config
                .plan_required(Some(project), task.plan_required)
            || task
                .labels
                .iter()
                .any(|l| l.eq_ignore_ascii_case("owner-only"))
            || task.dispatch_hold
            || task.launch.is_some()
            || !task.depends_on.iter().all(|id| {
                tasks
                    .iter()
                    .any(|t| t.id == *id && t.status == Status::Done)
            })
        {
            continue;
        }
        let Some(acceptance) = acceptance(&task.body) else {
            continue;
        };
        let likely_files = crate::lessons::likely_files(ctx, task)?;
        let paths: BTreeSet<_> = task.write_scope.iter().chain(&likely_files).collect();
        let mut candidate = PromotionCandidate {
            id: task.id.clone(),
            why: String::new(),
            predicted_conflicts: vec![],
            overlap_checked: !paths.is_empty(),
            rank: 0,
            project: project.into(),
            priority: task.priority.clone(),
            created: task.created,
            size: task.size,
            acceptance,
            plan_required: false,
            labels: task.labels.clone(),
            depends_on: task.depends_on.clone(),
            write_scope: task.write_scope.clone(),
            likely_files: likely_files.clone(),
            compared_tasks: vec![],
            unchecked_tasks: vec![],
            overlap_evidence: vec![],
        };
        for active in tasks.iter().filter(|t| {
            t.id != task.id
                && t.project == task.project
                && (matches!(t.status, Status::Running | Status::NeedsMe) || t.launch.is_some())
        }) {
            candidate.compared_tasks.push(active.id.clone());
            let files = crate::lessons::likely_files(ctx, active)?;
            let other: BTreeSet<_> = active.write_scope.iter().chain(&files).collect();
            if paths.is_empty() || other.is_empty() {
                candidate.overlap_checked = false;
                candidate.unchecked_tasks.push(active.id.clone());
                continue;
            }
            let mut overlaps = BTreeSet::new();
            for left in &paths {
                for right in &other {
                    if scopes_overlap(left, right) {
                        overlaps.insert(format!("{}: {left} intersects {right}", active.id));
                    }
                }
            }
            if !overlaps.is_empty() {
                candidate.predicted_conflicts.push(active.id.clone());
                candidate.overlap_evidence.extend(overlaps);
            }
        }
        candidate.compared_tasks.sort();
        candidate.unchecked_tasks.sort();
        candidate.predicted_conflicts.sort();
        candidate.overlap_evidence.sort();
        if !candidate.predicted_conflicts.is_empty() {
            continue;
        }
        let overlap = if paths.is_empty() {
            "overlap check skipped: no declared or historically inferable Idea paths".into()
        } else if !candidate.unchecked_tasks.is_empty() {
            format!(
                "overlap check incomplete: no inferable paths for {}",
                candidate.unchecked_tasks.join(", ")
            )
        } else {
            format!(
                "overlap checked against {} active task(s); {} predicted conflict(s)",
                candidate.compared_tasks.len(),
                candidate.predicted_conflicts.len()
            )
        };
        candidate.why = format!(
            "Eligible: nonempty acceptance, size <= M or unset, no plan or owner-only restriction, dependencies Done; {overlap}."
        );
        out.push(candidate);
    }
    out.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then(a.created.cmp(&b.created))
            .then(conflict_risk(a).cmp(&conflict_risk(b)))
            .then(a.id.cmp(&b.id))
    });
    for (index, candidate) in out.iter_mut().enumerate() {
        candidate.rank = index.saturating_add(1);
    }
    Ok(out)
}

fn conflict_risk(candidate: &PromotionCandidate) -> u8 {
    u8::from(!candidate.overlap_checked)
}

fn priority_rank(priority: &str) -> u8 {
    match priority {
        "P0" => 0,
        "P1" => 1,
        "P2" => 2,
        "P3" => 3,
        _ => 4,
    }
}

/// Only real Markdown acceptance sections count; an empty list is not acceptance.
fn acceptance(body: &str) -> Option<String> {
    let mut section = None;
    let mut content = Vec::new();
    let mut fence = None;
    let mut comment = false;
    for line in body.lines() {
        let mut text = String::new();
        let mut rest = line;
        loop {
            if comment {
                let Some((_, after)) = rest.split_once("-->") else {
                    break;
                };
                rest = after;
                comment = false;
            } else if let Some((before, after)) = rest.split_once("<!--") {
                text.push_str(before);
                rest = after;
                comment = true;
            } else {
                text.push_str(rest);
                break;
            }
        }
        let text = text.trim();
        if text.starts_with("```") || text.starts_with("~~~") {
            let marker = &text[..3];
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(if marker == "```" { "```" } else { "~~~" });
            }
            continue;
        }
        if fence.is_none() {
            let level = text.bytes().take_while(|c| *c == b'#').count();
            if (1..=6).contains(&level) && text[level..].starts_with(char::is_whitespace) {
                if section.is_some_and(|n| level <= n) {
                    if !content.is_empty() {
                        break;
                    }
                    section = None;
                }
                let heading = text[level..]
                    .trim()
                    .trim_end_matches('#')
                    .trim()
                    .to_ascii_lowercase();
                if matches!(
                    heading.as_str(),
                    "acceptance" | "acceptance criteria" | "acceptance-criteria"
                ) {
                    section = Some(level);
                }
                continue;
            }
        }
        if section.is_some()
            && text.chars().any(|c| c.is_alphanumeric())
            && !matches!(text, "- [ ]" | "* [ ]" | "- [x]" | "* [x]")
        {
            content.push(text.to_owned());
        }
    }
    (!content.is_empty()).then(|| content.join(" "))
}

// Product automata for the same `*`/`**` syntax used by T-064. Unlike matching
// one glob as a literal path, intersection detects two overlapping wildcard scopes.
fn scopes_overlap(a: &str, b: &str) -> bool {
    let a: Vec<_> = a.trim_start_matches("./").split('/').collect();
    let b: Vec<_> = b.trim_start_matches("./").split('/').collect();
    let mut pending = vec![(0, 0)];
    let mut seen = HashSet::new();
    while let Some((i, j)) = pending.pop() {
        if !seen.insert((i, j)) {
            continue;
        }
        let (left, right) = (a.get(i), b.get(j));
        if left.is_none() && right.is_none() {
            return true;
        }
        if left == Some(&"**") {
            pending.push((i + 1, j));
        }
        if right == Some(&"**") {
            pending.push((i, j + 1));
        }
        if let (Some(left), Some(right)) = (left, right)
            && (*left == "**" || *right == "**" || segments_overlap(left, right))
        {
            pending.push((
                i + usize::from(*left != "**"),
                j + usize::from(*right != "**"),
            ));
        }
    }
    false
}

fn segments_overlap(a: &str, b: &str) -> bool {
    let a: Vec<_> = a.chars().collect();
    let b: Vec<_> = b.chars().collect();
    let mut pending = vec![(0, 0)];
    let mut seen = HashSet::new();
    while let Some((i, j)) = pending.pop() {
        if !seen.insert((i, j)) {
            continue;
        }
        let (left, right) = (a.get(i), b.get(j));
        if left.is_none() && right.is_none() {
            return true;
        }
        if left == Some(&'*') {
            pending.push((i + 1, j));
        }
        if right == Some(&'*') {
            pending.push((i, j + 1));
        }
        if let (Some(left), Some(right)) = (left, right)
            && (*left == '*' || *right == '*' || left == right)
        {
            pending.push((
                i + usize::from(*left != '*'),
                j + usize::from(*right != '*'),
            ));
        }
    }
    false
}
