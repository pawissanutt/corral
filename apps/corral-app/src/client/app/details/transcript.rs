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
    let Some(first) = held.messages.first().filter(|m| !m.id.is_empty()) else {
        return pages.len() < cache.pages.len();
    };
    // A continuation is transport metadata, not another item. Tool results can move
    // between a standalone record and their call when the page boundary changes.
    // Check the source span here; payload/anchor compatibility is checked at publish.
    let records = || pages.iter().filter_map(session).flat_map(|p| &p.messages);
    fn result_id(item: &SessionItem) -> Option<&str> {
        match item {
            SessionItem::ToolCall {
                call_id,
                result: Some(_),
                call: None,
                paired: false,
                ..
            } if !call_id.is_empty() => Some(call_id),
            _ => None,
        }
    }
    let ordinary = |item: &&SessionItem| {
        !matches!(item, SessionItem::Continuation { .. }) && result_id(item).is_none()
    };
    let wanted = first.items.iter().filter(ordinary).count();
    let mut fragments = records().filter(|m| m.id == first.id).peekable();
    if (wanted > 0 || first.items.is_empty()) && fragments.peek().is_none() {
        return true;
    }
    if fragments.flat_map(|m| &m.items).filter(ordinary).count() < wanted {
        return true;
    }
    let mut results: BTreeSet<_> = first.items.iter().filter_map(result_id).collect();
    if !results.is_empty() {
        for item in records().flat_map(|m| &m.items) {
            if let SessionItem::ToolCall {
                call_id,
                result: Some(_),
                ..
            } = item
            {
                results.remove(call_id.as_str());
            }
        }
    }
    !results.is_empty()
}

enum Publication {
    Prepend,
    Replace { preserve: bool, auto_depth: bool },
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
        if matches!(&request.target, DetailRef::Session { id } if id == QUICK_THREAD)
            && feed.with_untracked(|feed| crate::model::assistant::ended(feed, &current.id))
        {
            // A pre-End read can resolve the old alias. It must not restore that transcript
            // under the scope where Ask will compose its next message.
            return self.restart_detail(feed, key, id, request.target, CHANGED.into());
        }
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
        let active = matches!(&request.target, DetailRef::Session { id } if self.transcript_readers.contains_key(id));
        let restarted = self.detail_restarts.contains_key(key);
        let auto_depth = self.transcript_auto_depth.contains(key);
        // Explicit scrollback stays loaded. An active view also protects its first page,
        // even if it hasn't scrolled far enough to request an older cursor. A window grown
        // only to protect such a reader may roll forward when nobody is reading back.
        let reading_history = depth > 0 && (active || restarted || (depth > 1 && !auto_depth));
        // A full replacement cannot fit if we already walk farther than the budget. An
        // exhausted new chain can still replace it if it covers all the anchors.
        if reading_history && next.is_some() && depth > MAX_PAGES {
            return self.transcript_history_changed(feed, key, &id);
        }
        if !walking && !reading_history && !chain.is_empty() {
            // The last reader released while rebuilding an automatic window. The newest
            // staged page is still good; there is no need to fit more history in the cache.
            let mut pages = self.detail_pages.remove(key).unwrap_or_default();
            pages.truncate(1);
            self.publish_transcript(
                feed,
                key,
                &id,
                &request.target,
                pages,
                Publication::Replace {
                    preserve: false,
                    auto_depth: false,
                },
            );
            return vec![];
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
        let publication = if walking {
            Publication::Prepend
        } else {
            Publication::Replace {
                preserve: reading_history,
                auto_depth: pages.len() > 1 && !restarted && (auto_depth || depth == 1),
            }
        };
        self.publish_transcript(feed, key, &id, &request.target, pages, publication);
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
        publication: Publication,
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
        if matches!(publication, Publication::Replace { preserve: true, .. })
            && feed.with_untracked(|feed| {
                feed.details
                    .get(key)
                    .is_some_and(|held| !replay::preserve_anchors(feed, held, &mut replacement))
            })
        {
            self.transcript_history_changed(feed, key, id);
            return;
        }
        if matches!(
            publication,
            Publication::Replace {
                auto_depth: true,
                ..
            }
        ) {
            self.transcript_auto_depth.insert(key.into());
        } else {
            self.transcript_auto_depth.remove(key);
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
                if matches!(publication, Publication::Prepend) {
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
