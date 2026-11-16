//! One retention budget for scoped snapshots, detail copies and transcript continuations.
//! Mounted readers and unresolved owner sends are not disposable cache entries.
//! The 128-entry/16-MiB budget counts pages, merged bodies, reply copies and assemblies
//! together. Pins may exceed it; in that case no inactive entry is retained.
use super::*;
use crate::model::session_feed::ItemFetch;

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Thread(String),
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
    item_sessions: BTreeMap<String, String>,
    operations: BTreeMap<String, BTreeSet<String>>,
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
                if matches!(target, DetailRef::Closed { .. }) {
                    self.evict_detail(feed, &key);
                }
            }
        }
        self.trim_retained(feed);
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
            key.len()
                + cache.map_or(0, |cache| {
                    bytes(&cache.pages) + bytes(&cache.merged) + bytes(&cache.target)
                })
                + self.detail_bytes.get(&key).copied().unwrap_or(0)
                + self.retained.requests.get(&key).map_or(0, bytes)
                + operations
                    .filter_map(|id| feed.operations.get(id.as_str()))
                    .map(|operation| bytes(&operation.value))
                    .sum::<usize>()
        });
        self.retained.touch(Key::Detail(key), size);
    }

    pub(super) fn touch_thread(&mut self, feed: Signal<Feed>, scope: &str) {
        let size = feed.with_untracked(|feed| {
            scope.len()
                + feed.threads.get(scope).map_or(0, |thread| {
                    thread.entries.values().map(bytes).sum::<usize>() + bytes(&thread.live)
                })
        });
        self.retained.touch(Key::Thread(scope.into()), size);
    }

    pub(super) fn touch_session(&mut self, feed: Signal<Feed>, scope: &str) {
        let size = feed.with_untracked(|feed| {
            scope.len()
                + feed.live.get(scope).map_or(0, |live| {
                    live.messages
                        .iter()
                        .map(|message| message.text.len() + message.thinking.len())
                        .sum::<usize>()
                        + live
                            .tools
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
        self.retained
            .item_sessions
            .insert(token.into(), session.into());
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

    fn scope_pinned(&self, scope: &str, feed: &Feed) -> bool {
        self.scopes.contains(scope)
            || self.session_scopes.contains_key(scope)
            || self.outbox.as_ref().is_some_and(|outbox| {
                outbox
                    .saved
                    .messages
                    .values()
                    .any(|message| message.request.thread == scope)
            })
            || feed.commands.values().any(|command| {
                command.method == ASSISTANT_SEND
                    && command.request["thread"].as_str() == Some(scope)
                    && (command.entry.is_some()
                        || matches!(
                            command.status,
                            CommandStatus::Queued
                                | CommandStatus::Pending
                                | CommandStatus::Running
                                | CommandStatus::Unknown
                        ))
            })
    }

    fn session_pinned(&self, session: &str, feed: &Feed) -> bool {
        self.scope_pinned(session, feed)
            || self.terminal_scopes.contains_key(session)
            || self
                .retained
                .pins
                .contains_key(&detail_key(&DetailRef::Session { id: session.into() }))
    }

    fn retained_pinned(&self, key: &Key, feed: &Feed) -> bool {
        match key {
            Key::Thread(scope) => self.scope_pinned(scope, feed),
            Key::Session(scope) => self.session_pinned(scope, feed),
            Key::Detail(key) => {
                self.retained.pins.contains_key(key)
                    || feed
                        .details
                        .get(key)
                        .is_some_and(|cache| match &cache.target {
                            DetailRef::Session { id } | DetailRef::Terminal { id } => {
                                self.session_pinned(id, feed)
                            }
                            _ => false,
                        })
            }
            Key::Item(token) => self
                .retained
                .item_sessions
                .get(token)
                .is_some_and(|session| self.session_pinned(session, feed)),
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
            Key::Thread(scope) => feed.update(|feed| {
                feed.threads.remove(scope.as_str());
                feed.topics.remove(&topic_key(Topic::Thread, Some(scope)));
            }),
            Key::Session(scope) => feed.update(|feed| {
                feed.live.remove(scope.as_str());
                feed.topics.remove(&topic_key(Topic::Session, Some(scope)));
            }),
            Key::Detail(key) => {
                self.detail_pages.remove(key);
                self.detail_bytes.remove(key);
                self.detail_reread.remove(key);
                self.detail_inflight.remove(key);
                feed.update(|feed| {
                    feed.details.remove(key);
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
                self.retained.item_sessions.remove(token);
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
