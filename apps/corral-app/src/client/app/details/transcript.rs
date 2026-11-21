//! Read the newest transcript page first. An explicit cursor adds one older page. A changed
//! snapshot is revalidated from its newest page, never spliced into an old branch.

use super::*;
use corral_protocol::rpc::ErrorCode;

/// Older cursors are scoped to a daemon snapshot. Bound automatic recovery after the snapshot
/// changes; the owner can still walk as far back as desired, one request per page.
const REFRESH_PAGES: usize = 8;
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
            !oldest.id.is_empty()
                && pages
                    .iter()
                    .filter_map(session)
                    .flat_map(|page| &page.messages)
                    .filter(|message| message.id == oldest.id)
                    .map(|message| message.items.len())
                    .sum::<usize>()
                    < oldest.items.len()
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
            Err((ErrorCode::Conflict, message)) if request.page.is_some() => {
                return self.restart_detail(feed, key, id, request.target, message);
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
            return self.restart_detail(feed, key, id, request.target, CHANGED.into());
        }
        if next.is_some() && next == request.page {
            return self.invalid_transcript(
                feed,
                key,
                &id,
                "Transcript cursor did not advance.".into(),
            );
        }
        let held = chain.len();
        if self.over_budget(feed, key, &id, &page, held) {
            return vec![];
        }
        let mut pages = self.detail_pages.remove(key).unwrap_or_default();
        pages.push(page);
        let missing = !walking
            && next.is_some()
            && feed.with_untracked(|feed| {
                feed.details.get(key).is_some_and(|cache| {
                    pages
                        .first()
                        .and_then(session)
                        .is_some_and(|current| history_missing(cache, &pages, current))
                })
            });
        if missing
            && pages.len() < REFRESH_PAGES
            && let Some(next) = next
        {
            self.detail_pages.insert(key.into(), pages);
            self.touch_detail(feed, &request.target);
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
        self.publish_transcript(feed, key, &id, &request.target, pages, walking, missing);
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

    #[allow(clippy::too_many_arguments)]
    fn publish_transcript(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
        target: &DetailRef,
        pages: Vec<DetailReply>,
        walking: bool,
        missing: bool,
    ) {
        let Some(mut newest) = pages.first().and_then(session).cloned() else {
            self.invalid_transcript(feed, key, id, "Missing transcript page.".into());
            return;
        };
        for older in pages.iter().skip(1).filter_map(session) {
            prepend(&mut newest, older.clone());
        }
        self.detail_inflight.remove(key);
        self.detail_restarts.remove(key);
        self.detail_bytes.remove(key);
        let reread = self.detail_reread.remove(key);
        feed.update(|feed| {
            feed.settle_session(target, reread);
            let response = pages.last().map(value);
            if let Some(cache) = feed.details.get_mut(key) {
                if walking {
                    if let Some(DetailBody::Session(held)) = &mut cache.merged {
                        prepend(held, newest);
                    }
                    cache.pages.extend(pages);
                } else {
                    cache.pages = pages;
                    cache.merged = Some(DetailBody::Session(newest));
                }
                cache.loading = false;
                cache.stale = false;
                cache.error = missing.then(|| ClientError {
                    code: HISTORY_CHANGED.into(),
                    message:
                        "The transcript changed. Use Show earlier messages to reload more history."
                            .into(),
                });
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
