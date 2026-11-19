//! A session's host-served parts, revalidated when the host announces a change of them.

use super::*;

/// Every session part the host announces a change of (`live_part`), by its wire name.
pub(in super::super) const PART_NAMES: &[&str] = &[
    "cards", "children", "jobs", "changes", "usage", "plan", "context", "board", "record",
    "models", "commands",
];

/// Whether the cached detail `target` is a part of session `scope` named in `parts`: each part
/// detail of the session, and for `children` the children's own transcripts.
fn part_of(target: &DetailRef, scope: &str, parts: &BTreeSet<String>) -> bool {
    let named = |part: &str| parts.contains(part);
    match target {
        DetailRef::SessionCards { id } => id == scope && named("cards"),
        DetailRef::SessionModels { id } => id == scope && named("models"),
        DetailRef::SessionChild { id, .. } => id == scope && named("children"),
        DetailRef::Session { id } => {
            named("children")
                && id
                    .strip_prefix(scope)
                    .is_some_and(|rest| rest.starts_with("/child/"))
        }
        DetailRef::SessionJobs { id } | DetailRef::SessionJobOutput { id, .. } => {
            id == scope && named("jobs")
        }
        DetailRef::SessionChanges { id, .. } | DetailRef::SessionChangeFile { id, .. } => {
            id == scope && named("changes")
        }
        DetailRef::SessionUsage { id } => id == scope && named("usage"),
        DetailRef::SessionContext { id } => id == scope && named("context"),
        DetailRef::SessionPlan { id } => id == scope && named("plan"),
        DetailRef::SessionCommands { id } => id == scope && named("commands"),
        DetailRef::BuildBoard { id } | DetailRef::BuildPlanTask { id, .. } => {
            id == scope && named("board")
        }
        DetailRef::BuildRecord { id } => id == scope && named("record"),
        _ => false,
    }
}

impl AppState {
    /// Revalidates every part of session `scope` this device holds whose host announced a
    /// change (`parts`), with its etag; a part nobody read is not fetched.
    pub(in super::super) fn session_parts(
        &mut self,
        feed: Signal<Feed>,
        scope: &str,
        parts: &BTreeSet<String>,
    ) -> Vec<Outgoing> {
        let targets = feed.with_untracked(|feed| {
            feed.details
                .values()
                .filter(|cache| !cache.loading && part_of(&cache.target, scope, parts))
                .map(|cache| cache.target.clone())
                .collect::<Vec<_>>()
        });
        let mut outgoing = vec![];
        for target in targets {
            outgoing.extend(self.detail_request(
                feed,
                requests::new_id(),
                self.retained.selected(target),
            ));
        }
        outgoing
    }
}
