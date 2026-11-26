//! Read the newest transcript page first. An explicit cursor adds one older page. A changed
//! snapshot is revalidated from its newest page, never spliced into an old branch.

use super::*;
use crate::model::session::transcript as replay;
use corral_protocol::rpc::ErrorCode;

/// The replacement could not retain the reading surface. The old snapshot remains available
/// until the reader explicitly returns to the newest page.
pub(super) const HISTORY_CHANGED: &str = "history_changed";

pub(in crate::client::app) fn session(page: &DetailReply) -> Option<&SessionPage> {
    match &page.body {
        Some(DetailBody::Session(body)) => Some(body),
        _ => None,
    }
}

/// A branch scan can advance without returning any messages. Keep its opaque
/// continuation on the previous payload page, not an empty slot in the reader's
/// window. An empty head is retained for its session metadata and etag.
pub(in crate::client::app) fn retain_page(pages: &mut Vec<DetailReply>, page: DetailReply) {
    if let Some(empty) = session(&page).filter(|p| p.messages.is_empty())
        && let Some(DetailBody::Session(edge)) = pages.last_mut().and_then(|p| p.body.as_mut())
    {
        edge.older.clone_from(&empty.older);
    } else {
        pages.push(page);
    }
}

/// Pages arrive newest first, each in chronological order. The older page may end with
/// the beginning of a record whose remainder is at the start of the newer page.
pub(in crate::client::app) fn prepend(newer: &mut SessionPage, mut older: SessionPage) {
    if let (Some(first), Some(last)) = (newer.messages.first_mut(), older.messages.last_mut())
        && !first.id.is_empty()
        && first.id == last.id
    {
        last.items.append(&mut first.items);
        last.attachments.append(&mut first.attachments);
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
    // Match facts newest-first as well. The old preview may contain an attachment prefix,
    // while the new snapshot starts with a suffix and needs attachment-only pages to reach it.
    let mut attachments = first.attachments.iter().rev().peekable();
    let mut items = 0;
    for message in fragments {
        items += message.items.iter().filter(ordinary).count();
        for fact in message.attachments.iter().rev() {
            let _ = attachments.next_if(|wanted| *wanted == fact);
        }
    }
    if items < wanted || attachments.peek().is_some() {
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
            Err((ErrorCode::Conflict, message)) if request.page.is_some() => {
                if feed.with_untracked(|f| {
                    f.capable(SESSION_TRANSCRIPT_V1)
                        && f.details
                            .get(key)
                            .is_some_and(|cache| cache.transcript_seams.mirror.active)
                }) {
                    let still_at_edge = feed.with_untracked(|f| {
                        f.details
                            .get(key)
                            .and_then(|cache| replay::page(Some(cache)))
                            .is_some_and(|held| held.older == request.page)
                    });
                    if still_at_edge {
                        self.detail_failed(
                            feed,
                            key,
                            &id,
                            ClientError {
                                code: HISTORY_CHANGED.into(),
                                message,
                            },
                        );
                    } else {
                        // A stream reset already moved the branch while this request was out.
                        self.detail_inflight.remove(key);
                        feed.update(|f| {
                            if let Some(cache) = f.details.get_mut(key) {
                                cache.loading = false;
                            }
                            if let Some(operation) = f.operations.get_mut(id.as_str()) {
                                operation.loading = false;
                            }
                        });
                    }
                    return vec![];
                }
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
        if feed.with_untracked(|f| {
            f.capable(SESSION_TRANSCRIPT_V1)
                && f.details
                    .get(key)
                    .is_some_and(|cache| cache.transcript_seams.mirror.active)
        }) {
            feed.update(|f| {
                if let Some(cache) = f.details.get_mut(key) {
                    // The latest page, asked for because the cursor before the held
                    // messages reads nothing any more: its cursor replaces that one.
                    let lost = request.page.is_none()
                        && cache
                            .error
                            .as_ref()
                            .is_some_and(|error| error.code == HISTORY_CHANGED);
                    if lost {
                        replay::mirror::rejoin(cache, current);
                        cache.pages = vec![page.clone()];
                    } else if let Some(DetailBody::Session(held)) = &mut cache.merged {
                        if request.page.is_some() {
                            if request.page == held.older {
                                let mirror = &mut cache.transcript_seams.mirror;
                                mirror.remember_edge(held);
                                if let Some(first) = held.messages.first() {
                                    mirror.joined_fragment(&first.id);
                                }
                                prepend(held, current.clone());
                                mirror.remember_edge(held);
                                retain_page(&mut cache.pages, page.clone());
                            }
                        } else {
                            let mut head = current.clone();
                            let first = held.messages.first().map(|m| m.id.as_str());
                            if let Some(at) = head
                                .messages
                                .iter()
                                .position(|m| Some(m.id.as_str()) == first)
                            {
                                head.messages.truncate(at);
                                if cache.pages.is_empty() && held.older.is_some() {
                                    prepend(held, head);
                                }
                            }
                            let mut metadata = current.clone();
                            metadata.older = held.older.take();
                            metadata.messages = std::mem::take(&mut held.messages);
                            *held = metadata;
                            if cache.pages.is_empty() {
                                cache.pages.push(page.clone());
                            }
                        }
                    }
                    cache.loading = false;
                    cache.stale = false;
                    cache.error = None;
                    replay::mirror::bound(cache);
                }
                if let Some(operation) = f.operations.get_mut(id.as_str()) {
                    operation.loading = false;
                    operation.value = serde_json::to_value(&page).ok();
                    operation.error = None;
                }
            });
            self.detail_inflight.remove(key);
            self.detail_latest.remove(key);
            self.detail_reread.remove(key);
            self.detail_restarts.remove(key);
            self.detail_bytes.remove(key);
            self.detail_pages.remove(key);
            self.touch_detail(feed, &request.target);
            self.trim_retained(feed);
            return vec![];
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
        if next.is_some()
            && (next == request.page
                || (walking
                    && current.messages.is_empty()
                    && feed.with_untracked(|f| {
                        next.as_ref().is_some_and(|n| {
                            f.details
                                .get(key)
                                .is_some_and(|c| c.transcript_seams.empty_cursors.contains(n))
                        })
                    })))
        {
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
        let active = matches!(&request.target, DetailRef::Session { id } if self.shared_transcript_reading(id));
        let restarted = self.detail_restarts.contains_key(key);
        let auto_depth = self.transcript_auto_depth.contains(key);
        // Explicit scrollback stays loaded. An active view also protects its first page,
        // even if it hasn't scrolled far enough to request an older cursor. A window grown
        // only to protect such a reader may roll forward when nobody is reading back.
        let reading_history = depth > 0 && (active || restarted || (depth > 1 && !auto_depth));
        // A full replacement cannot fit if we already walk farther than the budget. An
        // exhausted new chain can still replace it if it covers all the anchors.
        if reading_history && next.is_some() && depth > self.reader_limit() {
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
        let resident = if walking {
            feed.with_untracked(|feed| {
                feed.details.get(key).map_or((0, 0), |cache| {
                    (
                        cache.pages.len(),
                        cache.pages.iter().map(cache::bytes).sum::<usize>(),
                    )
                })
            })
        } else {
            (0, 0)
        };
        if held + resident.0 >= self.reader_limit()
            || prior_bytes
                .saturating_add(resident.1)
                .saturating_add(cache::bytes(&page))
                > MAX_DETAIL_BYTES
            || !self.count_detail_page(key, &page, held)
        {
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
            if pages.len() == self.reader_limit() {
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
        let response = pages.last().map(value);
        let scanned = pages.len();
        let mut resident = Vec::with_capacity(scanned);
        for page in pages {
            retain_page(&mut resident, page);
        }
        let pages = resident;
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
                        last.attachments.extend(first.attachments.iter().cloned());
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
        replacement.transcript_seams.skipped = scanned - replacement.pages.len();
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
            if let Some(cache) = feed.details.get_mut(key) {
                if matches!(publication, Publication::Prepend) {
                    if let (Some(DetailBody::Session(held)), Some(DetailBody::Session(older))) =
                        (&mut cache.merged, replacement.merged)
                    {
                        if older.messages.is_empty() {
                            cache
                                .transcript_seams
                                .empty_cursors
                                .extend(held.older.clone());
                        } else {
                            cache.transcript_seams.empty_cursors.clear();
                        }
                        prepend(held, older);
                    }
                    let before = cache.pages.len();
                    for page in replacement.pages {
                        retain_page(&mut cache.pages, page);
                    }
                    cache.transcript_seams.skipped += before + scanned - cache.pages.len();
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
        if matches!(target, DetailRef::Session { id } if id == QUICK_THREAD) {
            self.retire_quick_readers(feed);
        }
        self.touch_detail(feed, target);
        self.trim_retained(feed);
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
pub(in crate::client::app) mod tests;
