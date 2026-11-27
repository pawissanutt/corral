//! One retention budget for scoped snapshots, detail copies and transcript continuations.
//! Mounted readers and unresolved owner sends are not disposable cache entries.
//! The 128-entry/16-MiB budget counts pages, merged bodies, reply copies and assemblies
//! together. Pins may exceed it; in that case no inactive entry is retained.
use super::*;
use crate::model::session::transcript::ItemFetch;

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Session(String),
    Detail(String),
    Item(String),
}

#[derive(Default)]
pub(super) struct RetainedCache {
    clock: u64,
    entries: BTreeMap<Key, (u64, usize)>,
    pins: BTreeMap<String, usize>,
    requests: BTreeMap<String, DetailGet>,
    /// Continuations referenced by the current shared or detached pages for a scope.
    /// An open session pins these tokens, not every item ever expanded there.
    item_refs: BTreeMap<String, BTreeSet<String>>,
    operations: BTreeMap<String, BTreeSet<String>>,
    /// At most one search is in flight, even if its picker has since closed.
    released_search: Option<String>,
}

impl RetainedCache {
    fn touch(&mut self, key: Key, bytes: usize) {
        self.clock = self.clock.saturating_add(1);
        self.entries.insert(key, (self.clock, bytes));
    }

    pub(super) fn select(&mut self, request: &DetailGet, id: &str) {
        let key = detail_key(&request.target);
        self.requests.insert(key.clone(), request.clone());
        self.operations.entry(key).or_default().insert(id.into());
    }

    pub(super) fn selected(&self, target: DetailRef) -> DetailGet {
        let mut request = self
            .requests
            .get(&detail_key(&target))
            .cloned()
            .unwrap_or(DetailGet {
                target,
                etag: None,
                page: None,
            });
        // An etag validates the latest page, never an explicitly selected historical cursor.
        request.etag = None;
        request
    }

    fn forget_detail(&mut self, key: &str) {
        self.requests.remove(key);
        self.operations.remove(key);
        if self.released_search.as_deref() == Some(key) {
            self.released_search = None;
        }
    }
}

// Count encoded payloads without allocating another copy of a potentially multi-MiB body.
#[derive(Default)]
struct Counter(usize);
impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn bytes(value: &impl Serialize) -> usize {
    let mut count = Counter::default();
    match serde_json::to_writer(&mut count, value) {
        Ok(()) => count.0,
        Err(_) => MAX_BYTES.saturating_add(1),
    }
}

fn cache_bytes(cache: &DetailCache) -> usize {
    bytes(&cache.pages)
        + bytes(&cache.merged)
        + bytes(&cache.target)
        + bytes(&cache.transcript_seams)
}

fn item_refs(cache: &DetailCache) -> impl Iterator<Item = &str> {
    crate::model::session::transcript::page(Some(cache))
        .into_iter()
        .flat_map(|page| &page.messages)
        .flat_map(|message| &message.items)
        .filter_map(|item| match item {
            SessionItem::Continuation { token, .. } => Some(token.as_str()),
            _ => None,
        })
}

impl AppState {
    pub(super) fn watch_detail(&mut self, feed: Signal<Feed>, target: DetailRef, open: bool) {
        let key = detail_key(&target);
        if open {
            let count = self.retained.pins.entry(key).or_default();
            *count = count.saturating_add(1);
            self.touch_detail(feed, &target);
        } else if let Some(count) = self.retained.pins.get_mut(&key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.retained.pins.remove(&key);
                // Only Now's open Closed section reads a Closed page, and a kept one is
                // refetched on every board change; folding, paging or leaving drops it.
                // An unpinned diagram comparison resolves a moving head or candidate: a new
                // choice must ask again rather than pin a kept answer.
                if matches!(target, DetailRef::SessionFileSearch { .. }) {
                    if self.detail_inflight.contains_key(&key) {
                        // Keep the single-flight slot until the response arrives, but do not
                        // let this activation's reply satisfy a picker reopened in the meantime.
                        self.retained.released_search = Some(key);
                    } else {
                        self.evict_detail(feed, &key);
                    }
                } else if matches!(
                    target,
                    DetailRef::Closed { .. }
                        | DetailRef::ArtifactDiagramCompare {
                            target: corral_protocol::detail::CompareTarget::ObservedHead
                                | corral_protocol::detail::CompareTarget::Candidate,
                            ..
                        }
                ) {
                    self.evict_detail(feed, &key);
                }
            }
        }
        self.trim_retained(feed);
    }

    pub(super) fn discard_released_search_reply(&mut self, feed: Signal<Feed>, key: &str) -> bool {
        if self.retained.released_search.as_deref() != Some(key) {
            return false;
        }
        self.evict_detail(feed, key);
        true
    }

    pub(super) fn touch_detail(&mut self, feed: Signal<Feed>, target: &DetailRef) {
        let key = detail_key(target);
        feed.with_untracked(|feed| {
            if let Some(ids) = self.retained.operations.get_mut(&key) {
                ids.retain(|id| feed.operations.contains_key(id.as_str()));
            }
        });
        let size = feed.with_untracked(|feed| {
            let cache = feed.details.get(&key);
            let operations = self.retained.operations.get(&key).into_iter().flatten();
            let reader_bytes = if let DetailRef::Session { id } = target {
                let windows = feed.transcript_windows.values().filter(|w| w.scope == *id);
                let refs: BTreeSet<String> = cache
                    .into_iter()
                    .chain(windows.clone().map(|w| &w.cache))
                    .flat_map(item_refs)
                    .map(str::to_owned)
                    .collect();
                let refs_size = refs.iter().map(String::len).sum::<usize>();
                self.retained.item_refs.insert(id.clone(), refs);
                self.reader_staged_bytes(target)
                    + refs_size
                    + windows.map(|w| cache_bytes(&w.cache)).sum::<usize>()
            } else {
                0
            };
            key.len()
                + reader_bytes
                + cache.map_or(0, cache_bytes)
                + self.detail_bytes.get(&key).copied().unwrap_or(0)
                + self.retained.requests.get(&key).map_or(0, bytes)
                + operations
                    .filter_map(|id| feed.operations.get(id.as_str()))
                    .map(|operation| bytes(&operation.value))
                    .sum::<usize>()
        });
        self.retained.touch(Key::Detail(key), size);
    }

    /// A staged continuation has not changed the held snapshot or operation reply. Add its
    /// bytes without serializing the entire displayed history on every continuation.
    pub(super) fn touch_detail_continuation(
        &mut self,
        feed: Signal<Feed>,
        target: &DetailRef,
        added: usize,
    ) {
        let key = Key::Detail(detail_key(target));
        if let Some(&(_, size)) = self.retained.entries.get(&key) {
            self.retained.touch(key, size.saturating_add(added));
        } else {
            self.touch_detail(feed, target);
        }
    }

    pub(super) fn touch_session(&mut self, feed: Signal<Feed>, scope: &str) {
        let size = feed.with_untracked(|feed| {
            scope.len()
                + feed.live.get(scope).map_or(0, |live| {
                    live.tools
                        .iter()
                        .map(|tool| {
                            tool.call.len()
                                + tool.tool.len()
                                + tool.args.as_ref().map_or(0, String::len)
                                + tool.partial.as_ref().map_or(0, String::len)
                        })
                        .sum::<usize>()
                        + live
                            .ended
                            .as_ref()
                            .and_then(|end| end.error.as_ref())
                            .map_or(0, String::len)
                        + bytes(&live.fleet)
                })
        });
        self.retained.touch(Key::Session(scope.into()), size);
    }

    pub(super) fn touch_item(&mut self, feed: Signal<Feed>, session: &str, token: &str) {
        let size = feed.with_untracked(|feed| {
            token.len()
                + session.len()
                + match feed.items.get(token) {
                    Some(ItemFetch::Ready(item)) => bytes(item),
                    Some(ItemFetch::Failed(reason)) => reason.len(),
                    _ => 0,
                }
        }) + self
            .item_fetches
            .values()
            .filter(|assembly| assembly.root() == token)
            .map(|assembly| assembly.retained_bytes())
            .sum::<usize>();
        self.retained.touch(Key::Item(token.into()), size);
    }

    /// A session is pinned while a view watches it or this device has a message to it that
    /// corrald has not answered.
    fn session_pinned(&self, session: &str) -> bool {
        self.session_scopes.contains_key(session)
            || self.transcript_readers.contains_key(session)
            || self.terminal_scopes.contains_key(session)
            || self
                .retained
                .pins
                .contains_key(&detail_key(&DetailRef::Session { id: session.into() }))
            || self.outbox.as_ref().is_some_and(|outbox| {
                outbox
                    .saved
                    .messages
                    .values()
                    .any(|message| message.request.id == session)
            })
    }

    fn retained_pinned(&self, key: &Key, feed: &Feed) -> bool {
        match key {
            Key::Session(scope) => self.session_pinned(scope),
            Key::Detail(key) => {
                self.retained.pins.contains_key(key)
                    || feed
                        .details
                        .get(key)
                        .is_some_and(|cache| match &cache.target {
                            DetailRef::Session { id } | DetailRef::Terminal { id } => {
                                self.session_pinned(id)
                            }
                            // The pending search is the composer's single-flight marker.
                            DetailRef::SessionFileSearch { .. } => cache.loading,
                            _ => false,
                        })
            }
            Key::Item(token) => {
                self.item_fetches
                    .values()
                    .any(|fetch| fetch.root() == token)
                    || self
                        .retained
                        .item_refs
                        .iter()
                        .any(|(scope, refs)| refs.contains(token) && self.session_pinned(scope))
            }
        }
    }

    pub(super) fn trim_retained(&mut self, feed: Signal<Feed>) {
        let mut count = self.retained.entries.len();
        let mut size = self
            .retained
            .entries
            .values()
            .map(|(_, bytes)| bytes)
            .sum::<usize>();
        while count > MAX_ENTRIES || size > MAX_BYTES {
            let oldest = feed.with_untracked(|feed| {
                self.retained
                    .entries
                    .iter()
                    .filter(|(key, _)| !self.retained_pinned(key, feed))
                    .min_by_key(|(_, (stamp, _))| *stamp)
                    .map(|(key, (_, bytes))| (key.clone(), *bytes))
            });
            let Some((key, bytes)) = oldest else {
                break;
            };
            self.evict_retained(feed, &key);
            count = count.saturating_sub(1);
            size = size.saturating_sub(bytes);
        }
    }

    fn evict_retained(&mut self, feed: Signal<Feed>, key: &Key) {
        self.retained.entries.remove(key);
        match key {
            Key::Session(scope) => feed.update(|feed| {
                feed.live.remove(scope.as_str());
                feed.topics.remove(&topic_key(Topic::Session, Some(scope)));
            }),
            Key::Detail(key) => {
                self.detail_pages.remove(key);
                self.detail_bytes.remove(key);
                self.detail_reread.remove(key);
                self.detail_latest.remove(key);
                self.transcript_auto_depth.remove(key);
                self.transcript_deferred.remove(key);
                self.detail_inflight.remove(key);
                feed.update(|feed| {
                    if let Some(cache) = feed.details.remove(key)
                        && let DetailRef::Session { id } = cache.target
                    {
                        self.retained.item_refs.remove(&id);
                        self.retire_reader_scope(&id);
                        feed.transcript_windows
                            .retain(|_, window| window.scope != id);
                    }
                    if let Some(ids) = self.retained.operations.get(key) {
                        for id in ids {
                            if let Some(operation) = feed.operations.get_mut(id.as_str()) {
                                operation.loading = false;
                                operation.value = None;
                            }
                        }
                    }
                });
                self.retained.forget_detail(key);
            }
            Key::Item(token) => {
                self.item_fetches
                    .retain(|_, assembly| assembly.root() != token);
                feed.update(|feed| {
                    feed.items.remove(token);
                });
            }
        }
    }
    pub(super) fn evict_detail(&mut self, feed: Signal<Feed>, key: &str) {
        self.evict_retained(feed, &Key::Detail(key.into()));
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
