//! Read-only, source-backed eligibility and ranking for Idea promotion.
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};

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
    let _span = corral_core::timing::span("promotion.shortlist");
    let snapshot = corral_core::timing::span("promotion.snapshot");
    let ctx = &Ctx {
        store: ctx.store.read_only_snapshot()?,
        ..ctx.clone()
    };
    let heads = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?;
    let tasks = heads
        .iter()
        .map(|head| head.to_shell_task())
        .collect::<Result<Vec<_>>>()?;
    let dependency_ids: Vec<_> = tasks
        .iter()
        .filter(|task| task.status == Status::Idea)
        .flat_map(|task| task.depends_on.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let dependencies = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Ids(dependency_ids))?;
    drop(snapshot);

    // An active task's paths are inferred once per shortlist, not once per compared Idea.
    let mut active_scopes: HashMap<&str, Scopes> = HashMap::new();
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
                dependencies
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
        let paths = Scopes::new(task.write_scope.iter().chain(&likely_files));
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
            likely_files,
            compared_tasks: vec![],
            unchecked_tasks: vec![],
            overlap_evidence: vec![],
        };
        let mut conflict = false;
        for active in tasks.iter().filter(|t| {
            t.id != task.id
                && t.project == task.project
                && (matches!(t.status, Status::Running | Status::NeedsMe) || t.launch.is_some())
        }) {
            candidate.compared_tasks.push(active.id.clone());
            let other = match active_scopes.entry(active.id.as_str()) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(slot) => {
                    let files = crate::lessons::likely_files(ctx, active)?;
                    slot.insert(Scopes::new(active.write_scope.iter().chain(&files)))
                }
            };
            if paths.is_empty() || other.is_empty() {
                candidate.overlap_checked = false;
                candidate.unchecked_tasks.push(active.id.clone());
                continue;
            }
            // Any predicted conflict excludes this Idea, so its evidence is never reported
            // and later pairs need no intersection.
            conflict = conflict || paths.overlaps(other);
        }
        if conflict {
            continue;
        }
        candidate.compared_tasks.sort();
        candidate.unchecked_tasks.sort();
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

/// One task's declared and inferred paths, split so wildcard-free pairs compare by hash:
/// two scopes without `*` overlap exactly when their `./`-trimmed text is equal.
struct Scopes {
    literal: HashSet<String>,
    globs: Vec<String>,
}

impl Scopes {
    fn new<'a>(paths: impl IntoIterator<Item = &'a String>) -> Self {
        let mut scopes = Self {
            literal: HashSet::new(),
            globs: Vec::new(),
        };
        for path in paths {
            if path.contains('*') {
                scopes.globs.push(path.clone());
            } else {
                scopes
                    .literal
                    .insert(path.trim_start_matches("./").to_owned());
            }
        }
        scopes
    }

    fn is_empty(&self) -> bool {
        self.literal.is_empty() && self.globs.is_empty()
    }

    /// Whether `scopes_overlap(mine, theirs)` holds for any pair.
    fn overlaps(&self, other: &Self) -> bool {
        let (fewer, more) = if self.literal.len() <= other.literal.len() {
            (&self.literal, &other.literal)
        } else {
            (&other.literal, &self.literal)
        };
        fewer.iter().any(|path| more.contains(path))
            || self.globs.iter().any(|glob| {
                other
                    .globs
                    .iter()
                    .chain(&other.literal)
                    .any(|theirs| scopes_overlap(glob, theirs))
            })
            || other
                .globs
                .iter()
                .any(|glob| self.literal.iter().any(|mine| scopes_overlap(mine, glob)))
    }
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

#[cfg(test)]
mod derived_rows_tests {
    use super::*;
    use corral_core::Store;
    use corral_core::work::{self, Counter, HotScope};

    #[test]
    fn derived_promotion_listing_ignores_terminal_tasks_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
        // This Idea is ineligible before likely_files. That separately owned FU-3 walk
        // must not obscure this regression on promotion_shortlist's listing itself.
        let mut idea = ctx
            .store
            .create("Design", Some("app"), Status::Idea, "")
            .unwrap();
        idea.design = true;
        ctx.store.fixture_put_task(&idea).unwrap();
        let measure = || {
            let _scope = HotScope::enter();
            let before = work::snapshot();
            let output = promotion_shortlist(&ctx).unwrap();
            let counts = work::snapshot().since(&before);
            counts.check_identity().unwrap();
            assert_eq!(counts.unattributed_hydrations(), 0);
            (output, counts)
        };
        let (before, base) = measure();
        for i in 0..10 {
            ctx.store
                .create(&format!("Old {i}"), Some("app"), Status::Done, "")
                .unwrap();
            ctx.store
                .log_event(None, "unrelated", serde_json::json!({"i":i}))
                .unwrap();
        }
        let (after, grown) = measure();
        assert!(before.is_empty());
        assert_eq!(before, after);
        for counter in Counter::ALL {
            assert_eq!(base.get(counter), grown.get(counter), "{counter:?}");
        }
    }
}
