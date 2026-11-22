//! Read the newest transcript page first. An explicit cursor adds one older page. A changed
//! snapshot is revalidated from its newest page, never spliced into an old branch.

use super::*;
use crate::model::session::transcript as replay;
use corral_protocol::rpc::ErrorCode;

/// The replacement could not retain the reading surface. The old snapshot remains available
/// until the reader explicitly returns to the newest page.
pub(super) const HISTORY_CHANGED: &str = "history_changed";

fn session(page: &DetailReply) -> Option<&SessionPage> {
    match &page.body {
        Some(DetailBody::Session(body)) => Some(body),
        _ => None,
    }
}

/// Pages arrive newest first, each in chronological order. The older page may end with
/// the beginning of a record whose remainder is at the start of the newer page.
fn prepend(newer: &mut SessionPage, mut older: SessionPage) {
    if let (Some(first), Some(last)) = (newer.messages.first_mut(), older.messages.last_mut())
        && !first.id.is_empty()
        && first.id == last.id
    {
        last.items.append(&mut first.items);
        newer.messages.remove(0);
    }
    older.messages.append(&mut newer.messages);
    newer.messages = older.messages;
    newer.older = older.older;
}

fn history_missing(cache: &DetailCache, pages: &[DetailReply], current: &SessionPage) -> bool {
    let Some(DetailBody::Session(held)) = cache
        .body()
        .filter(|body| matches!(body, DetailBody::Session(held) if held.id == current.id))
    else {
        return false;
    };
    // A new tail can push the prior oldest record, or part of it, out of the same page depth.
    pages.len() < cache.pages.len()
        || held.messages.first().is_some_and(|oldest| {
            if oldest.id.is_empty() {
                return false;
            }
            let mut fragments = pages
                .iter()
                .filter_map(session)
                .flat_map(|page| &page.messages)
                .filter(|message| message.id == oldest.id);
            fragments.next().is_none_or(|first| {
                first.items.len() + fragments.map(|part| part.items.len()).sum::<usize>()
                    < oldest.items.len()
            })
        })
}

impl AppState {
    pub(super) fn transcript_reply(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: String,
        request: DetailGet,
        outcome: Result<Value, (ErrorCode, String)>,
    ) -> Vec<Outgoing> {
        let page: DetailReply = match outcome {
            Ok(reply) => match decode(reply) {
                Ok(page) => page,
                Err(message) => return self.invalid_transcript(feed, key, &id, message),
            },
            Err((ErrorCode::Conflict, _)) if request.page.is_some() => {
                return self.transcript_conflict(feed, key, id, request.target);
            }
            // The alias with no current quick chat offers the composer for a new one.
            Err((ErrorCode::NotFound, _))
                if request.page.is_none()
                    && matches!(&request.target, DetailRef::Session { id } if id == QUICK_THREAD) =>
            {
                DetailReply {
                    etag: String::new(),
                    as_of: String::new(),
                    seq: 0,
                    next: None,
                    body: Some(DetailBody::Session(empty_quick_page())),
                }
            }
            Err((code, message)) => {
                self.detail_failed(
                    feed,
                    key,
                    &id,
                    ClientError {
                        code: value(code).as_str().unwrap_or("unknown").into(),
                        message,
                    },
                );
                return vec![];
            }
        };
        let latest_only = self.detail_latest.contains(key);
        if latest_only && page.body.is_none() {
            return self.invalid_transcript(
                feed,
                key,
                &id,
                "Expected the latest transcript page.".into(),
            );
        }
        if page.body.is_none() && request.page.is_none() {
            return match self.detail_unchanged(feed, key, &id, &request.target, page) {
                Ok(()) => vec![],
                Err(message) => self.invalid_transcript(feed, key, &id, message),
            };
        }
        let Some(current) = session(&page) else {
            return self.invalid_transcript(feed, key, &id, "Expected a transcript page.".into());
        };
        let chain = self.detail_pages.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let walking = request.page.is_some() && chain.is_empty();
        let next = current.older.clone();
        let matching = |previous: &DetailReply| page.etag == previous.etag;
        let changed = if walking {
            feed.with_untracked(|feed| {
                feed.details.get(key).is_none_or(|cache| {
                    !cache.pages.first().is_some_and(matching)
                        || !matches!(cache.body(), Some(DetailBody::Session(held)) if held.id == current.id)
                })
            })
        } else {
            chain.first().is_some_and(|previous| {
                !matching(previous) || session(previous).is_none_or(|body| body.id != current.id)
            })
        };
        if changed {
            return self.transcript_conflict(feed, key, id, request.target);
        }
        if next.is_some() && next == request.page {
            return self.invalid_transcript(
                feed,
                key,
                &id,
                "Transcript cursor did not advance.".into(),
            );
        }
        let depth = if walking || latest_only {
            0
        } else {
            feed.with_untracked(|feed| {
                feed.details
                    .get(key)
                    .filter(|cache| matches!(cache.body(), Some(DetailBody::Session(held)) if held.id == current.id))
                    .map_or(0, |cache| cache.pages.len())
            })
        };
        // A full replacement cannot fit if we already walk farther than the budget. An
        // exhausted new chain can still replace it if it covers all the anchors.
        if next.is_some() && depth > MAX_PAGES {
            return self.transcript_history_changed(feed, key, &id);
        }
        let held = chain.len();
        let prior_bytes = self.detail_bytes.get(key).copied().unwrap_or(0);
        if !self.count_detail_page(key, &page, held) {
            if held > 0 {
                return self.transcript_history_changed(feed, key, &id);
            }
            self.detail_failed(
                feed,
                key,
                &id,
                ClientError {
                    code: "too_large".into(),
                    message: "Transcript page exceeds this device's bounded cache (8 MiB).".into(),
                },
            );
            return vec![];
        }
        let mut pages = self.detail_pages.remove(key).unwrap_or_default();
        pages.push(page);
        // Until the reader explicitly walks back, the view follows a single rolling
        // newest page. Never reconstruct displaced history just to refresh that tail.
        let reading_history = depth > 1 || (depth == 1 && self.detail_restarts.contains_key(key));
        if reading_history
            && let Some(next) = next
            && feed.with_untracked(|feed| {
                feed.details.get(key).is_some_and(|cache| {
                    pages
                        .first()
                        .and_then(session)
                        .is_some_and(|current| history_missing(cache, &pages, current))
                })
            })
        {
            if pages.len() == MAX_PAGES {
                return self.transcript_history_changed(feed, key, &id);
            }
            self.detail_pages.insert(key.into(), pages);
            let added = self.detail_bytes[key].saturating_sub(prior_bytes);
            self.touch_detail_continuation(feed, &request.target, added);
            self.trim_retained(feed);
            if self.detail_inflight.get(key) != Some(&id) {
                return vec![];
            }
            return vec![out(
                DETAIL_GET,
                value(DetailGet {
                    target: request.target,
                    etag: None,
                    page: Some(next),
                }),
                Some(id),
            )];
        }
        self.publish_transcript(feed, key, &id, &request.target, pages, walking);
        vec![]
    }

    fn transcript_conflict(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: String,
        target: DetailRef,
    ) -> Vec<Outgoing> {
        if self.detail_pages.get(key).is_some_and(Vec::is_empty) {
            // An explicit Older read found a stale cursor. Give it one newest-first
            // reconciliation, but do not restart if that staged reconstruction is raced too.
            if let DetailRef::Session { id: scope } = &target
                && let Some(reread) = feed
                    .with_untracked(|feed| feed.live.get(scope.as_str()).map(|live| live.reread))
            {
                self.detail_reread.insert(key.into(), reread);
            }
            return self.restart_detail(feed, key, id, target, CHANGED.into());
        }
        self.detail_failed(feed, key, &id, ClientError {
            code: "conflict".into(),
            message: "The transcript changed again while checking earlier messages. Use Load latest messages to rejoin the current transcript.".into(),
        });
        vec![]
    }

    fn transcript_history_changed(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
    ) -> Vec<Outgoing> {
        self.detail_failed(feed, key, id, ClientError {
            code: HISTORY_CHANGED.into(),
            message: "Still showing the previous transcript so you don't lose your place. Use Load latest messages to rejoin the current transcript.".into(),
        });
        vec![]
    }

    fn invalid_transcript(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
        message: String,
    ) -> Vec<Outgoing> {
        self.detail_failed(
            feed,
            key,
            id,
            ClientError {
                code: "invalid_reply".into(),
                message,
            },
        );
        vec![]
    }

    fn publish_transcript(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
        target: &DetailRef,
        pages: Vec<DetailReply>,
        walking: bool,
    ) {
        let Some(mut newest) = pages.first().and_then(session).cloned() else {
            self.invalid_transcript(feed, key, id, "Missing transcript page.".into());
            return;
        };
        if pages.len() > 1 {
            // Assemble oldest first, appending at each seam instead of re-copying the growing
            // transcript for every page. Keep the newest page's session metadata.
            newest.messages.clear();
            for part in pages.iter().rev().filter_map(session) {
                let mut messages = part.messages.iter();
                if let Some(first) = messages.next() {
                    if let Some(last) = newest.messages.last_mut()
                        && !first.id.is_empty()
                        && last.id == first.id
                    {
                        last.items.extend(first.items.iter().cloned());
                    } else {
                        newest.messages.push(first.clone());
                    }
                }
                newest.messages.extend(messages.cloned());
            }
            newest.older = pages
                .last()
                .and_then(session)
                .and_then(|part| part.older.clone());
        }
        let mut replacement = DetailCache {
            target: target.clone(),
            pages,
            transcript_seams: Default::default(),
            merged: Some(DetailBody::Session(newest)),
            loading: false,
            stale: false,
            error: None,
        };
        if !walking
            && !self.detail_latest.contains(key)
            && feed.with_untracked(|feed| {
                feed.details
                    .get(key)
                    .is_some_and(|held| !replay::preserve_anchors(feed, held, &mut replacement))
            })
        {
            self.transcript_history_changed(feed, key, id);
            return;
        }
        self.detail_inflight.remove(key);
        self.detail_latest.remove(key);
        self.detail_restarts.remove(key);
        self.detail_bytes.remove(key);
        let reread = self.detail_reread.remove(key);
        feed.update(|feed| {
            feed.settle_session(target, reread);
            let response = replacement.pages.last().map(value);
            if let Some(cache) = feed.details.get_mut(key) {
                if walking {
                    if let (Some(DetailBody::Session(held)), Some(DetailBody::Session(older))) =
                        (&mut cache.merged, replacement.merged)
                    {
                        prepend(held, older);
                    }
                    cache.pages.extend(replacement.pages);
                    cache.loading = false;
                    cache.stale = false;
                    cache.error = None;
                } else {
                    *cache = replacement;
                }
            }
            if let Some(operation) = feed.operations.get_mut(id) {
                operation.loading = false;
                operation.value = response;
                operation.error = None;
            }
        });
        self.touch_detail(feed, target);
        self.trim_retained(feed);
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
