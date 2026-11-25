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
        // The plan document changes with its review card.
        DetailRef::SessionCards { id } | DetailRef::SessionPlanModeDoc { id, .. } => {
            id == scope && named("cards")
        }
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
    /// change (`parts`), with its etag; a part nobody read is not fetched. Part reads are
    /// superseded, except a child transcript continues and checks the newest snapshot once
    /// more after its in-flight read settles.
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
            if matches!(target, DetailRef::Session { .. }) {
                outgoing.extend(self.refresh_transcript_part(feed, target));
            } else {
                outgoing.extend(self.detail_request(
                    feed,
                    requests::new_id(),
                    self.retained.selected(target),
                ));
            }
        }
        outgoing
    }

    pub(in crate::client::app) fn refresh_transcript_part(
        &mut self,
        feed: Signal<Feed>,
        target: DetailRef,
    ) -> Vec<Outgoing> {
        let key = detail_key(&target);
        if self.detail_inflight.contains_key(&key) {
            self.transcript_deferred.insert(key);
            return vec![];
        }
        let blocked = feed.with_untracked(|feed| {
            feed.detail(&target)
                .and_then(|cache| cache.error.as_ref())
                .is_some_and(|error| error.code == super::transcript::HISTORY_CHANGED)
        });
        if blocked {
            return vec![];
        }
        self.detail_request(feed, requests::new_id(), self.retained.selected(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::detail::{ModelChoice, SessionModels};

    #[test]
    fn child_transcript_notices_coalesce_behind_the_inflight_snapshot() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed::default());
        let mut app = AppState::new();
        app.ready = true;
        let target = DetailRef::Session {
            id: "B-1/child/a2".into(),
        };
        let get = app.detail_request(
            feed,
            "read".into(),
            DetailGet {
                target,
                etag: None,
                page: None,
            },
        );
        let [(_, params, tag)]: [Outgoing; 1] = get.try_into().unwrap();
        let parts = BTreeSet::from(["children".to_owned()]);
        assert!(app.session_parts(feed, "B-1", &parts).is_empty());
        assert!(app.session_parts(feed, "B-1", &parts).is_empty());
        // Even a failed read consumes the coalesced notices exactly once.
        let refused = || Err((corral_protocol::rpc::ErrorCode::Conflict, "raced".into()));
        let get = app.detail_reply(feed, tag, params, refused()).unwrap();
        let [(_, params, tag)]: [Outgoing; 1] = get.try_into().unwrap();
        assert!(
            app.detail_reply(feed, tag, params, refused())
                .unwrap()
                .is_empty()
        );
    }

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
