//! Holder-local branches: the published surface owns the resident payload.
use super::{
    details::{
        MAX_DETAIL_BYTES,
        transcript::{prepend, session},
    },
    *,
};
use corral_protocol::rpc::ErrorCode;
#[path = "reader_head.rs"]
mod head;

pub(super) const PREFIX: &str = "reader:";
pub(super) const LIMIT: usize = 16;
#[derive(Default)]
pub(super) struct Window {
    scope: String,
    path: Vec<Bookmark>,
    /// Empty pages advance the cursor but have no resident payload. Reject loops until
    /// real content arrives or the reader changes direction.
    empty_cursors: BTreeSet<String>,
    pending: Option<Read>,
}
#[derive(Clone, Default, serde::Serialize)]
struct Bookmark {
    cursor: Option<String>,
    suffix: Option<(String, usize)>,
}
struct Read {
    tag: String,
    intent: ReaderIntent,
    cursor: Option<String>,
    seek: Option<Seek>,
    /// Live generation observed when this private chain requested its newest page.
    reread: Option<u64>,
}
struct Seek {
    remaining: usize,
    path: Vec<Bookmark>,
    pages: Vec<DetailReply>,
    matched: bool,
}
fn target(scope: &str) -> DetailRef {
    DetailRef::Session { id: scope.into() }
}
fn payload(pages: &[DetailReply]) -> usize {
    pages.iter().map(cache::bytes).sum()
}
fn assemble(target: DetailRef, pages: Vec<DetailReply>) -> Option<DetailCache> {
    let mut body = session(pages.first()?)?.clone();
    for older in pages.iter().skip(1) {
        prepend(&mut body, session(older)?.clone());
    }
    Some(DetailCache {
        target,
        pages,
        merged: Some(DetailBody::Session(body)),
        transcript_seams: Default::default(),
        loading: false,
        stale: false,
        error: None,
    })
}
fn body(cache: &DetailCache) -> Option<&SessionPage> {
    match cache.body()? {
        DetailBody::Session(body) => Some(body),
        _ => None,
    }
}
fn drop_newest(
    pages: &mut Vec<DetailReply>,
    suffix: Option<&(String, usize)>,
) -> Option<(String, usize)> {
    let dropped = pages.remove(0);
    let older = pages
        .iter()
        .filter_map(session)
        .find_map(|p| p.messages.last())?;
    let missing = session(&dropped).and_then(|p| p.messages.first());
    let kept_suffix = suffix
        .filter(|(id, _)| id == &older.id)
        .map_or(0, |(_, n)| *n);
    match missing {
        Some(m) if !m.id.is_empty() && m.id == older.id => Some((
            m.id.clone(),
            kept_suffix
                + m.items
                    .iter()
                    .filter(|item| !matches!(item, SessionItem::Continuation { .. }))
                    .count(),
        )),
        None if kept_suffix > 0 => Some((older.id.clone(), kept_suffix)),
        _ => None,
    }
}
fn anchor(m: &SessionMessage) -> String {
    if m.id.is_empty() {
        format!("{}|{}", m.at.as_deref().unwrap_or_default(), m.author)
    } else {
        m.id.clone()
    }
}
fn page_index(cache: &DetailCache, source: &str) -> Option<usize> {
    cache
        .pages
        .iter()
        .position(|p| session(p).is_some_and(|p| p.messages.iter().any(|m| anchor(m) == source)))
}

fn compatible_anchor(
    feed: &Feed,
    held: &DetailCache,
    next: &mut DetailCache,
    wanted: Option<&str>,
) -> bool {
    let (Some(old), Some(new)) = (body(held), body(next)) else {
        return false;
    };
    if old.messages.is_empty() {
        return true;
    }
    let same_etag = held
        .pages
        .first()
        .zip(next.pages.first())
        .is_some_and(|(a, b)| a.etag == b.etag);
    let mut rows = old.messages.iter().filter(|m| {
        let key = anchor(m);
        wanted.is_none_or(|a| a == key)
            && old.messages.iter().filter(|r| anchor(r) == key).count() == 1
    });
    if same_etag {
        // Physical overlap proves an item survives even when the two assembled cuts
        // contain different prefixes/suffixes of the same split record.
        return rows.any(|m| {
            held.pages.iter().any(|a| {
                next.pages.iter().any(|b| {
                    session(a).zip(session(b)).is_some_and(|(a, b)| {
                        a.messages
                            .iter()
                            .any(|part| anchor(part) == anchor(m) && b.messages.contains(part))
                    })
                })
            })
        });
    }
    let mut base = held.clone();
    let Some(DetailBody::Session(body)) = &mut base.merged else {
        return false;
    };
    body.messages.clear();
    let ids: BTreeSet<String> = new.messages.iter().map(anchor).collect();
    // Cross-snapshot placement is only kept for the selected source anchor, not for
    // distant rows whose disappearance is expected when sliding a bounded window.
    for m in rows {
        if !ids.contains(&anchor(m)) {
            continue;
        }
        let Some(DetailBody::Session(body)) = &mut base.merged else {
            return false;
        };
        body.messages = vec![m.clone()];
        let mut candidate = next.clone();
        if crate::model::session::transcript::preserve_anchors(feed, &base, &mut candidate) {
            *next = candidate;
            return true;
        }
    }
    false
}
impl AppState {
    pub(super) fn shared_transcript_reading(&self, scope: &str) -> bool {
        let count = self.transcript_readers.get(scope).copied().unwrap_or(0);
        let detached = self
            .shared_holders
            .iter()
            .filter(|(holder, held)| {
                held.as_str() == scope && self.reader_windows.contains_key(holder)
            })
            .count();
        count > detached
    }

    pub(super) fn retire_reader_scope(&mut self, scope: &str) {
        self.reader_windows.retain(|_, w| w.scope != scope);
    }
    pub(super) fn retire_quick_readers(&mut self, feed: Signal<Feed>) {
        let target = target(QUICK_THREAD);
        let retired = feed.with_untracked(|f| {
            f.transcript_windows
                .iter()
                .filter_map(|(&holder, page)| {
                    let held = body(&page.cache)?;
                    (page.scope == QUICK_THREAD
                        && (crate::model::assistant::ended(f, &held.id)
                            || f.detail(&target)
                                .and_then(body)
                                .is_some_and(|current| current.id != held.id)))
                    .then_some(holder)
                })
                .collect::<Vec<_>>()
        });
        for holder in retired {
            self.close_reader(feed, holder);
        }
    }
    pub(super) fn reader_limit(&self) -> usize {
        #[cfg(test)]
        if let Some(limit) = self.test_reader_limit {
            return limit;
        }
        LIMIT
    }
    pub(super) fn reader_staged_bytes(&self, target: &DetailRef) -> usize {
        let DetailRef::Session { id } = target else {
            return 0;
        };
        self.reader_windows
            .values()
            .filter(|w| &w.scope == id)
            .map(|w| {
                cache::bytes(&w.path)
                    + w.empty_cursors.iter().map(String::len).sum::<usize>()
                    + w.pending.as_ref().map_or(0, |r| {
                        r.tag.len()
                            + r.cursor.as_ref().map_or(0, String::len)
                            + r.seek
                                .as_ref()
                                .map_or(0, |s| cache::bytes(&s.path) + payload(&s.pages))
                    })
            })
            .sum()
    }
    pub(super) fn reader_accepts(&self, tag: &str) -> bool {
        self.reader_windows
            .values()
            .any(|w| w.pending.as_ref().is_some_and(|r| r.tag == tag))
    }
    pub(super) fn interrupt_readers(&mut self, feed: Signal<Feed>) {
        for w in self.reader_windows.values_mut() {
            w.pending = None;
        }
        feed.update(|f| {
            for p in f.transcript_windows.values_mut() {
                p.cache.loading = false;
                p.cache.stale = true;
            }
        });
    }
    fn close_reader(&mut self, feed: Signal<Feed>, holder: u64) {
        let Some(w) = self.reader_windows.remove(&holder) else {
            return;
        };
        feed.update(|f| {
            f.transcript_windows.remove(&holder);
        });
        self.touch_detail(feed, &target(&w.scope));
        self.trim_retained(feed);
    }
    pub(super) fn reader_step(
        &mut self,
        feed: Signal<Feed>,
        intent: ReaderIntent,
    ) -> Vec<Outgoing> {
        let holder = intent.holder;
        if !feed.with_untracked(|f| f.store == intent.store) {
            return vec![];
        }
        let detached = feed.with_untracked(|f| {
            f.transcript_windows.get(&holder).is_some_and(|p| {
                p.scope == intent.scope && body(&p.cache).is_some_and(|b| b.id == intent.resolved)
            })
        });
        if matches!(intent.step, ReaderStep::Close) {
            if detached {
                self.close_reader(feed, holder);
            }
            return vec![];
        }
        if !detached {
            self.close_reader(feed, holder);
        }
        if !self.ready {
            return vec![];
        }
        let target = target(&intent.scope);
        let Some(held) = feed.with_untracked(|f| {
            (if detached {
                f.transcript_windows.get(&holder).map(|p| &p.cache)
            } else {
                f.detail(&target)
            })
            .filter(|c| body(c).is_some_and(|b| b.id == intent.resolved))
            .cloned()
        }) else {
            return vec![];
        };
        let latest = matches!(intent.step, ReaderStep::Latest);
        if held.loading && !latest {
            return vec![];
        }
        let cursor = match &intent.step {
            ReaderStep::Older { cursor }
                if body(&held).and_then(|b| b.older.as_ref()) == Some(cursor) =>
            {
                // Leave room for a complete daemon frame before using the shared path.
                if !detached
                    && held.pages.len() < self.reader_limit()
                    && payload(&held.pages) < MAX_DETAIL_BYTES / 2
                {
                    return self.detail_request(
                        feed,
                        requests::new_id(),
                        DetailGet {
                            target,
                            etag: None,
                            page: Some(cursor.clone()),
                        },
                    );
                }
                Some(cursor.clone())
            }
            ReaderStep::Newer if detached => {
                let Some(w) = self.reader_windows.get(&holder) else {
                    return vec![];
                };
                if w.path.len() < 2 {
                    return vec![];
                }
                w.path[w.path.len() - 2].cursor.clone()
            }
            ReaderStep::Latest => None,
            _ => return vec![],
        };
        if !detached {
            self.reader_windows.insert(
                holder,
                Window {
                    scope: intent.scope.clone(),
                    path: vec![Bookmark::default()],
                    ..Window::default()
                },
            );
            feed.update(|f| {
                f.transcript_windows.insert(
                    holder,
                    ReaderPage {
                        scope: intent.scope.clone(),
                        cache: held,
                        newer: false,
                    },
                );
            });
        }
        if !self.shared_transcript_reading(&intent.scope) {
            self.transcript_auto_depth.insert(detail_key(&target));
        }
        feed.update(|f| {
            if let Some(p) = f.transcript_windows.get_mut(&holder) {
                p.cache.loading = true;
                if !latest {
                    p.cache.error = None;
                }
            }
        });
        self.send_reader(
            feed,
            Read {
                tag: String::new(),
                intent,
                cursor,
                seek: None,
                reread: None,
            },
        )
    }
    fn send_reader(&mut self, feed: Signal<Feed>, mut read: Read) -> Vec<Outgoing> {
        let target = target(&read.intent.scope);
        if read.cursor.is_none() {
            read.reread = feed.with_untracked(|f| {
                f.live
                    .get(read.intent.scope.as_str())
                    .map(|live| live.reread)
            });
        }
        read.tag = format!("{PREFIX}{}", requests::new_id());
        let holder = read.intent.holder;
        let outgoing = out(
            DETAIL_GET,
            value(DetailGet {
                target: target.clone(),
                etag: None,
                page: read.cursor.clone(),
            }),
            Some(read.tag.clone()),
        );
        if let Some(w) = self.reader_windows.get_mut(&holder) {
            w.pending = Some(read);
        }
        self.touch_detail(feed, &target);
        self.trim_retained(feed);
        if feed.with_untracked(|f| f.transcript_windows.contains_key(&holder)) {
            vec![outgoing]
        } else {
            self.reader_windows.remove(&holder);
            vec![]
        }
    }
    fn reader_failed(
        &mut self,
        feed: Signal<Feed>,
        intent: &ReaderIntent,
        code: &str,
        message: &str,
    ) -> Vec<Outgoing> {
        feed.update(|f| {
            if let Some(p) = f.transcript_windows.get_mut(&intent.holder) {
                p.cache.loading = false;
                p.cache.stale = true;
                let code = if p
                    .cache
                    .error
                    .as_ref()
                    .is_some_and(|e| e.code == "history_changed")
                {
                    "history_changed"
                } else {
                    code
                };
                p.cache.error = Some(ClientError {
                    code: code.into(),
                    message: message.into(),
                });
            }
        });
        self.touch_detail(feed, &target(&intent.scope));
        self.trim_retained(feed);
        vec![]
    }
    pub(super) fn reader_reply(
        &mut self,
        feed: Signal<Feed>,
        tag: &str,
        request: DetailGet,
        outcome: Result<Value, (ErrorCode, String)>,
    ) -> Vec<Outgoing> {
        let Some((&holder, w)) = self
            .reader_windows
            .iter_mut()
            .find(|(_, w)| w.pending.as_ref().is_some_and(|r| r.tag == tag))
        else {
            return vec![];
        };
        let Some(mut read) = w.pending.take() else {
            return vec![];
        };
        let path = w.path.clone();
        let target = target(&read.intent.scope);
        let key = detail_key(&target);
        let fail = |app: &mut Self, read: &Read| {
            app.reader_failed(feed, &read.intent, "history_changed", "Still showing the previous transcript. Load latest messages to rejoin the current transcript.")
        };
        if request.target != target || request.page != read.cursor {
            return fail(self, &read);
        }
        let held = feed.with_untracked(|f| {
            let current = f.store == read.intent.store
                && (read.intent.scope != QUICK_THREAD
                    || (!crate::model::assistant::ended(f, &read.intent.resolved)
                        && f.details
                            .get(&key)
                            .and_then(body)
                            .is_none_or(|b| b.id == read.intent.resolved)));
            if current {
                f.transcript_windows.get(&holder).map(|p| p.cache.clone())
            } else {
                None
            }
        });
        let Some(held) = held else {
            self.close_reader(feed, holder);
            return vec![];
        };
        let latest = matches!(read.intent.step, ReaderStep::Latest);
        let page: Option<DetailReply> = match outcome {
            Ok(reply) => match decode(reply) {
                Ok(p) => Some(p),
                Err(_) => return fail(self, &read),
            },
            Err((ErrorCode::Conflict, _)) if !latest => None,
            Err((code, message)) => {
                return self.reader_failed(
                    feed,
                    &read.intent,
                    value(code).as_str().unwrap_or("unknown"),
                    &message,
                );
            }
        };
        if page
            .as_ref()
            .and_then(session)
            .is_some_and(|b| b.id != read.intent.resolved)
        {
            if read.intent.scope == QUICK_THREAD {
                self.close_reader(feed, holder);
                return vec![];
            }
            return fail(self, &read);
        }
        let prior = read
            .seek
            .as_ref()
            .map_or(held.pages.as_slice(), |s| s.pages.as_slice());
        let Some(page) = page.filter(|p| {
            session(p).is_some()
                && (latest
                    || read.seek.as_ref().is_some_and(|s| s.pages.is_empty())
                    || prior.first().is_some_and(|first| first.etag == p.etag))
        }) else {
            if latest || read.seek.is_some() {
                return fail(self, &read);
            }
            read.seek = Some(Seek {
                remaining: path.len().saturating_sub(1) + held.pages.len() + LIMIT,
                path: vec![],
                pages: vec![],
                matched: false,
            });
            read.cursor = None;
            return self.send_reader(feed, read);
        };
        if cache::bytes(&page) > MAX_DETAIL_BYTES {
            return fail(self, &read);
        }
        if latest {
            let Some(cache) = assemble(target, vec![page]) else {
                return fail(self, &read);
            };
            return self.join_head(feed, &read.intent, cache, true, read.reread);
        }
        let next = session(&page).and_then(|b| b.older.clone());
        if next.is_some()
            && (next == read.cursor
                || (matches!(read.intent.step, ReaderStep::Older { .. })
                    && read.seek.is_none()
                    && path.iter().any(|b| b.cursor == next)))
        {
            return fail(self, &read);
        }
        if read.seek.is_none()
            && matches!(read.intent.step, ReaderStep::Older { .. })
            && session(&page).is_some_and(|p| p.messages.is_empty())
        {
            if let (Some(w), Some(cursor)) = (self.reader_windows.get_mut(&holder), &read.cursor) {
                w.empty_cursors.insert(cursor.clone());
                if next.as_ref().is_some_and(|n| w.empty_cursors.contains(n)) {
                    return fail(self, &read);
                }
            }
            // An empty transport page adds no rows. Move the boundary without evicting any.
            let mut replacement = held;
            if let Some(DetailBody::Session(p)) = &mut replacement.merged {
                p.older = next.clone();
            }
            if let Some(Some(DetailBody::Session(p))) =
                replacement.pages.last_mut().map(|p| p.body.as_mut())
            {
                p.older = next;
            }
            replacement.loading = false;
            replacement.stale = false;
            replacement.error = None;
            return self.publish_reader(feed, holder, path, replacement);
        }
        if let Some(w) = self.reader_windows.get_mut(&holder) {
            w.empty_cursors.clear();
        }
        let limit = self.reader_limit();
        if let Some(seek) = read.seek.as_mut().filter(|s| !s.matched) {
            if seek.path.iter().any(|b| b.cursor == read.cursor) {
                return fail(self, &read);
            }
            seek.path.push(Bookmark {
                cursor: read.cursor.clone(),
                suffix: None,
            });
            seek.pages.push(page);
            while seek.pages.len() > limit || payload(&seek.pages) > MAX_DETAIL_BYTES {
                let index = seek.path.len() - seek.pages.len();
                let (left, right) = seek.path.split_at_mut(index + 1);
                right[0].suffix = drop_newest(&mut seek.pages, left[index].suffix.as_ref());
            }
            let Some(mut replacement) = assemble(target.clone(), seek.pages.clone()) else {
                return fail(self, &read);
            };
            if feed.with_untracked(|f| {
                compatible_anchor(f, &held, &mut replacement, read.intent.anchor.as_deref())
            }) {
                seek.path.truncate(seek.path.len() - (seek.pages.len() - 1));
                seek.matched = true;
                replacement.transcript_seams.suffix =
                    seek.path.last().and_then(|b| b.suffix.clone());
                // Searching from the head can itself move the window forward. If the
                // verified source already has more newer pages above it than before,
                // use this result; another fetch would evict the row we just recovered.
                if matches!(read.intent.step, ReaderStep::Newer)
                    && read.intent.anchor.as_deref().is_some_and(|key| {
                        page_index(&held, key)
                            .zip(page_index(&replacement, key))
                            .is_some_and(|(old, new)| new > old)
                    })
                {
                    if seek.path.len() == 1 {
                        return self.join_head(feed, &read.intent, replacement, false, read.reread);
                    }
                    return self.publish_reader(feed, holder, seek.path.clone(), replacement);
                }
                read.cursor = match read.intent.step {
                    ReaderStep::Newer if seek.path.len() > 1 => {
                        seek.path[seek.path.len() - 2].cursor.clone()
                    }
                    ReaderStep::Older { .. } if next.is_some() => next,
                    ReaderStep::Newer => {
                        return self.join_head(feed, &read.intent, replacement, false, read.reread);
                    }
                    _ => return self.publish_reader(feed, holder, seek.path.clone(), replacement),
                };
                return self.send_reader(feed, read);
            }
            seek.remaining -= 1;
            if seek.remaining == 0 || next.is_none() {
                return fail(self, &read);
            }
            read.cursor = next;
            return self.send_reader(feed, read);
        }
        let (base, mut path) = if let Some(seek) = read.seek.take() {
            // This fresh window has already passed the cross-snapshot anchor check.
            // The retried directional step is relative to it, not to the stale pages.
            let Some(base) = assemble(target.clone(), seek.pages) else {
                return fail(self, &read);
            };
            (base, seek.path)
        } else {
            (held, path)
        };
        let mut pages = base.pages.clone();
        let newer = matches!(read.intent.step, ReaderStep::Newer);
        if newer {
            path.pop();
            pages.insert(0, page);
        } else {
            pages.push(page);
        }
        while pages.len() > limit || payload(&pages) > MAX_DETAIL_BYTES {
            if newer {
                pages.pop();
            } else {
                let cursor = pages
                    .first()
                    .and_then(session)
                    .and_then(|b| b.older.clone());
                let suffix = drop_newest(&mut pages, path.last().and_then(|b| b.suffix.as_ref()));
                path.push(Bookmark { cursor, suffix });
            }
        }
        let Some(mut replacement) = assemble(target, pages) else {
            return fail(self, &read);
        };
        replacement.transcript_seams.suffix = path.last().and_then(|b| b.suffix.clone());
        // In a short viewport the row at its top can be on the very page
        // Show newer intentionally removes. In that case clamp to the first
        // surviving shared row instead of reporting that the branch changed.
        let wanted = read.intent.anchor.as_deref().filter(|key| {
            !newer || page_index(&base, key).is_none() || page_index(&replacement, key).is_some()
        });
        if !feed.with_untracked(|f| compatible_anchor(f, &base, &mut replacement, wanted)) {
            return fail(self, &read);
        }
        if newer && path.len() == 1 {
            return self.join_head(feed, &read.intent, replacement, false, read.reread);
        }
        self.publish_reader(feed, holder, path, replacement)
    }
    fn publish_reader(
        &mut self,
        feed: Signal<Feed>,
        holder: u64,
        path: Vec<Bookmark>,
        cache: DetailCache,
    ) -> Vec<Outgoing> {
        let target = cache.target.clone();
        let Some(w) = self.reader_windows.get_mut(&holder) else {
            return vec![];
        };
        let newer = path.len() > 1;
        w.path = path;
        let scope = w.scope.clone();
        feed.update(|f| {
            f.transcript_windows.insert(
                holder,
                ReaderPage {
                    scope,
                    cache,
                    newer,
                },
            );
        });
        self.touch_detail(feed, &target);
        self.trim_retained(feed);
        vec![]
    }
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
