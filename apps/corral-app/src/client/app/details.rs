use super::*;

const MAX_PAGES: usize = 256;
const MAX_DETAIL_BYTES: usize = 8 * 1024 * 1024;
/// How many times in a row a detail that changed between two of its pages is read again from
/// its first page before it is reported stale with the conflict.
const MAX_RESTARTS: u8 = 3;
const CHANGED: &str =
    "Detail changed while its pages were loading; refresh to fetch a current snapshot.";

mod parts;
mod transcript;
mod walked;

pub(super) use parts::PART_NAMES;
use walked::{merge_pages, walked};

impl AppState {
    pub(super) fn detail_request(
        &mut self,
        feed: Signal<Feed>,
        id: String,
        request: DetailGet,
    ) -> Vec<Outgoing> {
        self.request_detail(feed, id, request, false)
    }

    pub(super) fn latest_transcript(
        &mut self,
        feed: Signal<Feed>,
        session: String,
    ) -> Vec<Outgoing> {
        self.request_detail(
            feed,
            requests::new_id(),
            DetailGet {
                target: DetailRef::Session { id: session },
                etag: None,
                page: None,
            },
            true,
        )
    }

    fn request_detail(
        &mut self,
        feed: Signal<Feed>,
        id: String,
        request: DetailGet,
        latest_only: bool,
    ) -> Vec<Outgoing> {
        let mut request = match request {
            DetailGet {
                target: DetailRef::SessionFileChunk { .. },
                ..
            } => return vec![],
            DetailGet {
                target: DetailRef::SessionItem { id, token },
                ..
            } => {
                return self.session_item(feed, id, token);
            }
            request => request,
        };
        let key = detail_key(&request.target);
        // The finder's list is walked: each page is asked for as the owner reaches it and adds
        // to those before, rather than all of them being fetched and merged up front.
        let walked = walked(&request.target);
        let session = matches!(request.target, DetailRef::Session { .. });
        if session
            && (self.detail_latest.contains(&key)
                || (!latest_only && self.detail_inflight.contains_key(&key)))
        {
            // Background rereads must not race each other or the owner's explicit return.
            return vec![];
        }
        if session && let Some(cursor) = request.page.as_deref() {
            let current = feed.with_untracked(|feed| {
                feed.details.get(&key).is_some_and(|cache| {
                    !cache.loading
                        && !cache.error.as_ref().is_some_and(|error| error.code == transcript::HISTORY_CHANGED)
                        && matches!(cache.body(), Some(DetailBody::Session(page)) if page.older.as_deref() == Some(cursor))
                })
            });
            if !current {
                return vec![];
            }
        }
        if !latest_only && (walked || session) && request.page.is_none() && request.etag.is_none() {
            // Validate the newest held page, not the cursor of a previously read page.
            request.etag = feed.with_untracked(|feed| {
                feed.details
                    .get(&key)
                    .filter(|cache| cache.merged.is_some() && (session || cache.error.is_none()))
                    .and_then(|cache| cache.pages.first())
                    .map(|page| page.etag.clone())
            });
        }
        // One page of a paged list is shown at a time, so a new page retires its siblings.
        let paged = walked || matches!(&request.target, DetailRef::Closed { .. });
        let mut retired = if paged {
            let list = std::mem::discriminant(&request.target);
            feed.with_untracked(|feed| {
                feed.details
                    .iter()
                    .filter(|(cached_key, cache)| {
                        cached_key.as_str() != key.as_str()
                            && std::mem::discriminant(&cache.target) == list
                    })
                    .map(|(cached_key, _)| (cached_key.clone(), None))
                    .collect::<Vec<_>>()
            })
        } else {
            Vec::new()
        };
        for (retired_key, retired_id) in &mut retired {
            *retired_id = self.detail_inflight.remove(retired_key);
            self.evict_detail(feed, retired_key);
        }
        feed.update(|feed| {
            for (_, retired_id) in &retired {
                if let Some(operation) = retired_id
                    .as_ref()
                    .and_then(|id| feed.operations.get_mut(id.as_str()))
                {
                    operation.loading = false;
                    operation.value = None;
                    operation.error = Some(ClientError {
                        code: "superseded".into(),
                        message: "A different page of this list replaced this detail request."
                            .into(),
                    });
                }
            }
            let cache = feed
                .details
                .entry(key.clone())
                .or_insert_with(|| DetailCache {
                    transcript_seams: Default::default(),
                    target: request.target.clone(),
                    pages: vec![],
                    loading: false,
                    stale: true,
                    error: None,
                    merged: None,
                });
            cache.loading = self.ready;
            cache.stale = true;
            let keep_recovery = cache.error.as_ref().is_some_and(|error| {
                error.code == transcript::HISTORY_CHANGED
                    || (latest_only && error.code == "conflict")
            });
            if !keep_recovery {
                cache.error = (!self.ready).then(offline);
            }
        });
        // Refresh from the newest page, not from the page the owner last walked to.
        let remembered = DetailGet {
            page: request.page.clone().filter(|_| !walked && !session),
            ..request.clone()
        };
        self.retained.select(&remembered, &id);
        if !self.ready {
            self.touch_detail(feed, &request.target);
            self.trim_retained(feed);
            return vec![];
        }
        if let DetailRef::Session { id } = &request.target {
            if request.page.is_none()
                && let Some(reread) =
                    feed.with_untracked(|feed| feed.live.get(id.as_str()).map(|live| live.reread))
            {
                self.detail_reread.insert(key.clone(), reread);
            } else {
                self.detail_reread.remove(&key);
            }
        }
        // Only the latest request for a stable selector can publish its pages.
        if let Some(previous) = self.detail_inflight.insert(key.clone(), id.clone()) {
            feed.update(|feed| {
                if let Some(operation) = feed.operations.get_mut(previous.as_str()) {
                    operation.loading = false;
                    operation.error = Some(ClientError {
                        code: "superseded".into(),
                        message: "A newer fetch replaced this detail request.".into(),
                    });
                }
            });
        }
        if latest_only {
            self.detail_latest.insert(key.clone());
        }
        self.detail_pages.insert(key.clone(), vec![]);
        self.detail_restarts.remove(&key);
        self.detail_bytes.insert(key.clone(), 0);
        let target = request.target.clone();
        let outgoing = self.operation(feed, id.clone(), DETAIL_GET, value(request));
        self.touch_detail(feed, &target);
        self.trim_retained(feed);
        if self.detail_inflight.get(&key) == Some(&id) {
            outgoing
        } else {
            vec![]
        }
    }
    pub(super) fn detail_reply(
        &mut self,
        feed: Signal<Feed>,
        tag: Option<String>,
        params: Value,
        outcome: Result<Value, (corral_protocol::rpc::ErrorCode, String)>,
    ) -> Result<Vec<Outgoing>, String> {
        let request: DetailGet = decode(params)?;
        if matches!(&request.target, DetailRef::SessionFileChunk { .. }) {
            return Ok(vec![]);
        }
        if let DetailRef::SessionItem { id, token } = &request.target {
            return self.session_item_reply(feed, tag, id, token, outcome);
        }
        let key = detail_key(&request.target);
        let Some(id) = tag else {
            return Err("detail reply has no correlation identity".into());
        };
        if self.detail_inflight.get(&key) != Some(&id) {
            return Ok(vec![]);
        }
        if matches!(request.target, DetailRef::Session { .. }) {
            let target = request.target.clone();
            let mut outgoing = self.transcript_reply(feed, &key, id, request, outcome);
            if !self.detail_inflight.contains_key(&key) && self.transcript_deferred.remove(&key) {
                outgoing.extend(self.refresh_transcript_part(feed, target));
            }
            return Ok(outgoing);
        }
        let page: DetailReply = match outcome {
            Ok(value) => decode(value)?,
            Err((code, message)) => {
                // A continuation of a detail that changed since its first page starts again.
                if code == corral_protocol::rpc::ErrorCode::Conflict
                    && request.page.is_some()
                    && !walked(&request.target)
                {
                    return Ok(self.restart_detail(feed, &key, id, request.target, message));
                }
                let error = ClientError {
                    code: value(code).as_str().unwrap_or("unknown").into(),
                    message,
                };
                self.detail_failed(feed, &key, &id, error);
                return Ok(vec![]);
            }
        };
        if matches!(&page.body, Some(DetailBody::SessionFileChunk(_))) {
            self.detail_failed(
                feed,
                &key,
                &id,
                ClientError {
                    code: "invalid_reply".into(),
                    message: "Unexpected HTML snapshot chunk in a detail read.".into(),
                },
            );
            return Ok(vec![]);
        }
        if walked(&request.target) {
            return Ok(self.walked_reply(feed, id, key, request, page));
        }
        let changed = self
            .detail_pages
            .get(&key)
            .and_then(|pages| pages.first())
            .is_some_and(|first| first.etag != page.etag);
        if changed {
            return Ok(self.restart_detail(feed, &key, id, request.target, CHANGED.into()));
        }
        let pages = self.detail_pages.entry(key.clone()).or_default();
        if page.body.is_none() {
            if !pages.is_empty() || request.page.is_some() {
                return Err("daemon returned not-modified inside a detail continuation".into());
            }
            self.detail_unchanged(feed, &key, &id, &request.target, page)?;
            return Ok(vec![]);
        }
        let next = page.next.clone();
        let held = self.detail_pages.get(&key).map_or(0, Vec::len);
        if self.over_budget(feed, &key, &id, &page, held) {
            return Ok(vec![]);
        }
        let pages = self.detail_pages.entry(key.clone()).or_default();
        pages.push(page);
        if let Some(next) = next {
            self.touch_detail(feed, &request.target);
            self.trim_retained(feed);
            if self.detail_inflight.get(&key) != Some(&id) {
                return Ok(vec![]);
            }
            return Ok(vec![out(
                DETAIL_GET,
                value(DetailGet {
                    target: request.target,
                    etag: None,
                    page: Some(next),
                }),
                Some(id),
            )]);
        }
        let pages = self.detail_pages.remove(&key).unwrap_or_default();
        self.detail_restarts.remove(&key);
        self.publish(feed, &key, &id, &request.target, pages);
        Ok(vec![])
    }

    fn detail_unchanged(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
        target: &DetailRef,
        page: DetailReply,
    ) -> Result<(), String> {
        let valid = feed.with_untracked(|feed| {
            feed.details.get(key).is_some_and(|cache| {
                cache.merged.is_some()
                    && cache.pages.first().is_some_and(|old| old.etag == page.etag)
            })
        });
        if !valid {
            return Err("daemon returned not-modified without a matching cached detail".into());
        }
        let reread = self.detail_reread.remove(key);
        feed.update(|feed| {
            feed.settle_session(target, reread);
            if let Some(cache) = feed.details.get_mut(key) {
                cache.loading = false;
                cache.stale = false;
                cache.error = None;
                if let Some(first) = cache.pages.first_mut() {
                    first.as_of = page.as_of.clone();
                    first.seq = page.seq;
                }
            }
            if let Some(operation) = feed.operations.get_mut(id) {
                operation.loading = false;
                operation.value = Some(value(page));
                operation.error = None;
            }
        });
        self.detail_pages.remove(key);
        self.detail_restarts.remove(key);
        self.detail_inflight.remove(key);
        self.detail_latest.remove(key);
        self.detail_bytes.remove(key);
        self.touch_detail(feed, target);
        self.trim_retained(feed);
        Ok(())
    }

    /// A detail that changed between two of its pages is read again from its first page under
    /// the same request, at most `MAX_RESTARTS` times in a row; then it is stale with `message`.
    fn restart_detail(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: String,
        target: DetailRef,
        message: String,
    ) -> Vec<Outgoing> {
        let restarts = self.detail_restarts.entry(key.to_owned()).or_default();
        if *restarts >= MAX_RESTARTS {
            let error = ClientError {
                code: "conflict".into(),
                message,
            };
            self.detail_failed(feed, key, &id, error);
            return vec![];
        }
        *restarts += 1;
        self.detail_pages.insert(key.to_owned(), vec![]);
        self.detail_bytes.insert(key.to_owned(), 0);
        vec![out(
            DETAIL_GET,
            value(DetailGet {
                target,
                etag: None,
                page: None,
            }),
            Some(id),
        )]
    }

    /// Adds `page` to the bytes the request has fetched, after the `held` pages before it; once
    /// over this device's bound the request fails instead.
    fn over_budget(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
        page: &DetailReply,
        held: usize,
    ) -> bool {
        if self.count_detail_page(key, page, held) {
            return false;
        }
        self.detail_failed(
            feed,
            key,
            id,
            ClientError {
                code: "too_large".into(),
                message: "Detail exceeds this device's bounded cache (8 MiB).".into(),
            },
        );
        true
    }

    /// Accounts for one page of an operation, not the manually walked pages already held.
    fn count_detail_page(&mut self, key: &str, page: &DetailReply, held: usize) -> bool {
        let bytes = self.detail_bytes.entry(key.to_owned()).or_default();
        *bytes = bytes.saturating_add(cache::bytes(page));
        held < MAX_PAGES && *bytes <= MAX_DETAIL_BYTES
    }

    /// Publishes the request's pages, merged, as the detail's current snapshot.
    fn publish(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
        target: &DetailRef,
        mut pages: Vec<DetailReply>,
    ) {
        let seq = pages.iter().map(|page| page.seq).max().unwrap_or(0);
        let as_of = pages.last().map(|page| page.as_of.clone());
        if let Some(first) = pages.first_mut() {
            first.seq = seq;
            if let Some(as_of) = as_of {
                first.as_of = as_of;
            }
        }
        self.detail_bytes.remove(key);
        let merged = match merge_pages(&pages) {
            Ok(merged) => merged,
            Err(message) => {
                self.detail_failed(
                    feed,
                    key,
                    id,
                    ClientError {
                        code: "invalid_reply".into(),
                        message,
                    },
                );
                return;
            }
        };
        self.detail_inflight.remove(key);
        feed.update(|feed| {
            if let Some(cache) = feed.details.get_mut(key) {
                cache.pages = pages;
                cache.merged = merged;
                cache.loading = false;
                cache.stale = false;
                cache.error = None;
            }
            let response = feed
                .details
                .get(key)
                .and_then(|cache| cache.pages.first())
                .map(value);
            if let Some(operation) = feed.operations.get_mut(id) {
                operation.loading = false;
                operation.value = response;
                operation.error = None;
            }
        });
        self.touch_detail(feed, target);
        self.trim_retained(feed);
    }
    fn detail_failed(&mut self, feed: Signal<Feed>, key: &str, id: &str, error: ClientError) {
        self.detail_pages.remove(key);
        self.detail_restarts.remove(key);
        self.detail_inflight.remove(key);
        self.detail_bytes.remove(key);
        self.detail_reread.remove(key);
        let latest_failed = self.detail_latest.remove(key);
        feed.update(|feed| {
            if let Some(cache) = feed.details.get_mut(key) {
                cache.loading = false;
                cache.stale = true;
                let recovery = cache.error.as_ref().filter(|prior| {
                    latest_failed && matches!(prior.code.as_str(), transcript::HISTORY_CHANGED | "conflict")
                });
                // A failed explicit return is retryable without losing the kept transcript.
                cache.error = Some(recovery.map_or_else(|| error.clone(), |prior| ClientError {
                    code: prior.code.clone(),
                    message: format!("Could not load latest messages: {}. Still showing the previous transcript.", error.message),
                }));
            }
            if let Some(operation) = feed.operations.get_mut(id) {
                operation.loading = false;
                operation.error = Some(error);
            }
        });
        let target = self.retained_target(feed, key);
        if let Some(target) = target {
            self.touch_detail(feed, &target);
        }
        self.trim_retained(feed);
    }
    pub(super) fn refresh_details(&mut self, feed: Signal<Feed>) -> Vec<Outgoing> {
        let targets = feed.with_untracked(|feed| {
            feed.details
                .values()
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
    fn retained_target(&self, feed: Signal<Feed>, key: &str) -> Option<DetailRef> {
        feed.with_untracked(|feed| feed.details.get(key).map(|cache| cache.target.clone()))
    }
}

#[cfg(test)]
#[path = "closed_tests.rs"]
mod closed_tests;
