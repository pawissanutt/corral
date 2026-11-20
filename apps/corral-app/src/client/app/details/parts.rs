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
    /// change (`parts`), with its etag; a part nobody read is not fetched. A part still loading
    /// is read again too: its pages may predate the change, and the new read supersedes them.
    pub(in super::super) fn session_parts(
        &mut self,
        feed: Signal<Feed>,
        scope: &str,
        parts: &BTreeSet<String>,
    ) -> Vec<Outgoing> {
        let targets = feed.with_untracked(|feed| {
            feed.details
                .values()
                .filter(|cache| part_of(&cache.target, scope, parts))
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

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::detail::{ModelChoice, SessionModels};

    fn models_page(favorite: bool) -> Value {
        value(DetailReply {
            etag: format!("v-{favorite}"),
            as_of: "now".into(),
            seq: 1,
            next: None,
            body: Some(DetailBody::SessionModels(SessionModels {
                current: None,
                default: None,
                effort: None,
                models: vec![ModelChoice {
                    id: "anthropic/claude".into(),
                    provider: "anthropic".into(),
                    name: None,
                    context_window: None,
                    effort_levels: vec![],
                    scoped: false,
                    favorite,
                    note: None,
                }],
                refreshed_at: None,
                refreshing: false,
            })),
        })
    }

    #[test]
    fn a_part_announced_while_its_read_is_in_flight_is_read_again_and_the_older_reply_never_publishes()
     {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed::default());
        let mut app = AppState::new();
        app.ready = true;
        let target = DetailRef::SessionModels { id: "B-1".into() };
        let mut first = app.detail_request(
            feed,
            "read".into(),
            DetailGet {
                target: target.clone(),
                etag: None,
                page: None,
            },
        );
        let (_, old_params, old_tag) = first.remove(0);
        // The host announces `models` before the first read's (older) page arrives.
        let parts = BTreeSet::from(["models".to_owned()]);
        let mut again = app.session_parts(feed, "B-1", &parts);
        assert_eq!(again.len(), 1, "the in-flight part is read again");
        let (_, params, tag) = again.remove(0);
        app.detail_reply(feed, old_tag, old_params, Ok(models_page(false)))
            .unwrap();
        app.detail_reply(feed, tag, params, Ok(models_page(true)))
            .unwrap();
        feed.with_untracked(|feed| {
            let cache = &feed.details[&detail_key(&target)];
            let Some(DetailBody::SessionModels(models)) = &cache.merged else {
                panic!("models published");
            };
            assert!(models.models[0].favorite);
        });
    }
}
