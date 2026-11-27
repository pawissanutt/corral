use super::*;

impl AppState {
    pub(super) fn retry_topic(&self, feed: Signal<Feed>, key: &str) -> Vec<Outgoing> {
        if !self.ready {
            feed.update(|feed| feed.problem = Some(offline().message));
            return vec![];
        }
        let (name, scope) = key
            .split_once(':')
            .map_or((key, None), |(name, scope)| (name, Some(scope.to_owned())));
        let Ok(topic) = serde_json::from_value::<Topic>(Value::String(name.to_owned())) else {
            return vec![];
        };
        if topic == Topic::Other {
            return vec![];
        }
        feed.update(|feed| {
            let state = feed.topics.entry(key.to_owned()).or_default();
            state.loading = true;
            state.stale = true;
            state.error = None;
        });
        // A bare reopen of Tasks could take an old subscriber's late pages for the new one's
        // (`client::walks`), releasing a held move on a board taken before it.
        if topic == Topic::Tasks {
            return self.refresh_tasks();
        }
        vec![open(topic, scope)]
    }

    /// Tasks closed, then opened again: the core drops pages of the closing subscription and
    /// sends the open only once the close is answered, so the replacement is taken after it.
    pub(super) fn refresh_tasks(&self) -> Vec<Outgoing> {
        if !self.ready {
            return vec![];
        }
        vec![
            out(
                wire::SUB_CLOSE,
                value(SubClose {
                    topic: Topic::Tasks,
                    scope: None,
                }),
                None,
            ),
            open(Topic::Tasks, None),
        ]
    }

    pub(super) fn terminal_request(
        &mut self,
        feed: Signal<Feed>,
        session: String,
        on: bool,
    ) -> Vec<Outgoing> {
        let transition = if on {
            let count = self.terminal_scopes.entry(session.clone()).or_default();
            *count = count.saturating_add(1);
            *count == 1
        } else if let Some(count) = self.terminal_scopes.get_mut(&session) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.terminal_scopes.remove(&session);
                true
            } else {
                false
            }
        } else {
            false
        };
        if !transition {
            return vec![];
        }
        self.watch_detail(
            feed,
            DetailRef::Terminal {
                id: session.clone(),
            },
            on,
        );
        let streamed = feed.with_untracked(|feed| feed.capable(STREAM_TERMINAL_V1));
        if !self.ready || !streamed {
            return vec![];
        }
        let key = topic_key(Topic::Terminal, Some(&session));
        feed.update(|feed| {
            if on {
                feed.topics.insert(
                    key,
                    TopicState {
                        loading: true,
                        stale: true,
                        error: None,
                    },
                );
            } else {
                feed.topics.remove(&key);
            }
            if let Some(cache) = feed.details.get_mut(&detail_key(&DetailRef::Terminal {
                id: session.clone(),
            })) {
                cache.stale = true;
                cache.loading = on;
            }
        });
        vec![if on {
            open(Topic::Terminal, Some(session))
        } else {
            out(
                wire::SUB_CLOSE,
                value(SubClose {
                    topic: Topic::Terminal,
                    scope: Some(session),
                }),
                None,
            )
        }]
    }
}
