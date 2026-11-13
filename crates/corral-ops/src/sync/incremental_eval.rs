//! Pure evaluation helpers of the incremental planner: per-task dirtiness, clock dues, record
//! key task tokens and the pane-inventory fingerprint.
use std::hash::{Hash, Hasher};

use corral_herdr::PaneInfo;
use jiff::{SignedDuration, Timestamp};

use super::rules::{self, Rule};
use super::{PassMemory, StepId, TaskMemo, Tokens};

/// The task is in `D` (A.1.2).
pub(super) fn task_dirty(
    tokens: &Tokens,
    memory: &PassMemory,
    memo: &TaskMemo,
    id: &str,
    now: Timestamp,
) -> bool {
    let feed = &tokens.feed;
    memo.last_run.is_none()
        || feed.tasks.contains(&memo.num)
        || feed.task_ids.contains(id)
        || tokens.keyed.contains(&memo.num.0)
        || tokens.all_tasks
        || (tokens.pm && (memo.pm_reader || memory.native.contains(id)))
        || (tokens.inventory && memo.pane_ref)
        || tokens
            .panes
            .as_ref()
            .is_some_and(|panes| memo.live_panes.iter().any(|p| !panes.contains(p)))
        || tokens.git.tasks.contains(id)
        || memo.due.next(memo.last_run).is_some_and(|at| at <= now)
        || memory.task_sticky.contains(id)
        || (memo.dependency_waits && tokens.tasks_changed())
}

/// The earliest clock due of a global step: `every`, settle after acting, the movement quiet
/// window after the newest approval, and the UTC day boundary. Dues a run already passed are
/// consumed.
pub(super) fn step_due(memory: &PassMemory, step: StepId, rule: &Rule) -> Option<Timestamp> {
    let last = memory.last_run.get(&step).copied();
    let open = |at: Timestamp| last.is_none_or(|run| at > run);
    [
        rule.every
            .map(|every| last.map_or(Timestamp::MIN, |run| add(run, every))),
        rule.settle
            .and_then(|settle| memory.acted_at.get(&step).map(|at| add(*at, settle)))
            .filter(|at| open(*at)),
        rule.approved
            .and_then(|quiet| memory.approved_at.map(|at| add(at, quiet)))
            .filter(|at| open(*at)),
        rule.utc_day
            .then(|| last.map_or(Some(Timestamp::MIN), next_utc_day))
            .flatten(),
    ]
    .into_iter()
    .flatten()
    .min()
}

/// The minimum of every step and task due strictly after `now`. Overdue entries are not
/// repeated: their step was not reached this pass, and the paced wait bounds their delay.
pub(super) fn next_due(memory: &PassMemory, now: Timestamp) -> Option<Timestamp> {
    let steps = rules::STEPS.iter().filter_map(|step| {
        let rule = (memory.rules)(*step);
        (!rule.per_task)
            .then(|| step_due(memory, *step, &rule))
            .flatten()
    });
    let task_steps = memory.task_run.iter().filter_map(|((step, _), at)| {
        let rule = (memory.rules)(*step);
        rule.per_task
            .then_some(rule.every.map(|every| add(*at, every)))
            .flatten()
    });
    let tasks = memory
        .tasks
        .values()
        .filter_map(|memo| memo.due.next(memo.last_run));
    steps
        .chain(task_steps)
        .chain(tasks)
        .filter(|at| *at > now)
        .min()
}

fn next_utc_day(at: Timestamp) -> Option<Timestamp> {
    let utc = jiff::tz::TimeZone::UTC;
    let tomorrow = at.to_zoned(utc.clone()).date().tomorrow().ok()?;
    tomorrow.to_zoned(utc).ok().map(|day| day.timestamp())
}

pub(super) fn add(at: Timestamp, by: SignedDuration) -> Timestamp {
    at.saturating_add(by).unwrap_or(Timestamp::MAX)
}

/// `a*` matches by prefix, anything else exactly.
pub(super) fn area_matches(pattern: &str, area: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => area.starts_with(prefix),
        None => area == pattern,
    }
}

/// Task numbers of every `T-<digits>` token in a record area or key (`autopilot-T-042`,
/// `T-7/run`, `inbox/T-7_R1/messages`).
pub(super) fn task_tokens(key: &str) -> impl Iterator<Item = u64> + '_ {
    let bytes = key.as_bytes();
    (0..bytes.len()).filter_map(move |i| {
        let starts = bytes[i] == b'T'
            && bytes.get(i + 1) == Some(&b'-')
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric());
        if !starts {
            return None;
        }
        let digits: &str = {
            let rest = &key[i + 2..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            &rest[..end]
        };
        digits.parse().ok()
    })
}

/// Hash of `(pane_id, workspace_id, agent, status, cwd)` per pane, sorted. Titles churn and
/// are left out on purpose.
pub(super) fn inventory_fingerprint(panes: &[PaneInfo]) -> u64 {
    let mut rows: Vec<_> = panes
        .iter()
        .map(|p| {
            (
                &p.pane_id,
                &p.workspace_id,
                &p.agent,
                p.status.as_str(),
                &p.cwd,
            )
        })
        .collect();
    rows.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rows.hash(&mut hasher);
    hasher.finish()
}
