use super::*;

#[cfg(test)]
#[path = "head_race_tests.rs"]
mod head_race_tests;

impl AppState {
    /// Install the head verified by this reader. Other shared readers may still need
    /// their old snapshot: detach them in the same publication before switching it.
    pub(super) fn join_head(
        &mut self,
        feed: Signal<Feed>,
        intent: &ReaderIntent,
        cache: DetailCache,
        explicit: bool,
        reread: Option<u64>,
    ) -> Vec<Outgoing> {
        let holder = intent.holder;
        let target = cache.target.clone();
        let key = detail_key(&target);
        let seq = cache.pages[0].seq;
        let shared = feed.with_untracked(|f| f.details.get(&key).cloned());
        let protected = self
            .shared_holders
            .iter()
            .filter_map(|(&other, scope)| {
                (other != holder
                    && scope == &intent.scope
                    && !self.reader_windows.contains_key(&other))
                .then_some(other)
            })
            .collect::<Vec<_>>();
        let replace = shared
            .as_ref()
            .and_then(|c| c.pages.first())
            .is_none_or(|p| {
                p.seq <= seq
                    && (p.etag != cache.pages[0].etag
                        || (!explicit
                            && shared
                                .as_ref()
                                .is_some_and(|c| c.pages.len() < cache.pages.len()))
                        || shared.as_ref().is_some_and(|c| c.error.is_some()))
            });
        let live_reread =
            feed.with_untracked(|f| f.live.get(intent.scope.as_str()).map(|live| live.reread));
        let shared_reread = self.detail_reread.get(&key).copied();
        let mut refresh = live_reread > reread && live_reread > shared_reread;
        if replace {
            // A shared conditional reply was sent against the previous cache. Retire
            // its tag before swapping caches, but replay any work this head did not cover.
            refresh |= shared_reread > reread;
            refresh |= self.transcript_deferred.remove(&key);
            for &other in &protected {
                self.reader_windows.insert(
                    other,
                    Window {
                        scope: intent.scope.clone(),
                        path: vec![Bookmark::default()],
                        ..Window::default()
                    },
                );
            }
            let superseded = self.detail_inflight.remove(&key);
            self.detail_latest.remove(&key);
            self.detail_restarts.remove(&key);
            self.detail_bytes.remove(&key);
            self.detail_pages.remove(&key);
            self.detail_reread.remove(&key);
            feed.update(|f| {
                if let Some(mut before) = shared {
                    before.loading = false;
                    for other in protected {
                        f.transcript_windows.insert(
                            other,
                            ReaderPage {
                                scope: intent.scope.clone(),
                                cache: before.clone(),
                                newer: false,
                            },
                        );
                    }
                }
                if let Some(operation) = superseded
                    .as_deref()
                    .and_then(|id| f.operations.get_mut(id))
                {
                    operation.loading = false;
                }
                f.settle_session(&target, reread);
                f.details.insert(key, cache);
            });
        } else if shared.as_ref().is_some_and(|c| c.error.is_some()) {
            // The shared recovery surface is newer than this reply. It is not
            // safe to clear its error with a response from an older generation.
            return self.reader_failed(
                feed,
                intent,
                "history_changed",
                "The conversation changed while loading the latest messages. Try again.",
            );
        }
        self.close_reader(feed, holder);
        if refresh {
            self.refresh_transcript_part(feed, target)
        } else {
            vec![]
        }
    }
}
