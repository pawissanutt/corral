use super::*;

impl AppState {
    fn clear_local_projection(&mut self, feed: Signal<Feed>) {
        self.outbox = None;
        self.namespace = None;
        self.offline = super::super::offline::Cache::default();
        feed.update(|feed| {
            feed.mark_stale();
            feed.link = Link::Down;
            feed.capabilities.clear();
            feed.store = None;
            feed.workspace = None;
            feed.board = Board::default();
            feed.inbox.clear();
            feed.plans.clear();
            feed.builds.clear();
            feed.projects.clear();
            feed.topics.remove("projects");
            feed.health = None;
            feed.drafts.clear();
            feed.commands.clear();
            feed.cancelling.clear();
            feed.threads.clear();
            feed.details.clear();
        });
    }
    pub async fn boot(
        &mut self,
        feed: Signal<Feed>,
        expected_device: Option<&str>,
    ) -> Result<(), String> {
        if expected_device.is_some_and(|expected| {
            self.namespace
                .as_ref()
                .is_some_and(|(_, device)| device != expected)
        }) {
            self.clear_local_projection(feed);
        }
        if let Some((namespace, cache)) =
            super::super::offline::Cache::restore(feed, expected_device).await?
        {
            self.outbox = Some(Outbox::load_async(&namespace.0, &namespace.1).await?);
            self.namespace = Some(namespace);
            self.offline = cache;
            self.project_local(feed);
        }
        Ok(())
    }

    pub async fn forget_local(&mut self, feed: Signal<Feed>) -> Result<(), String> {
        if let Some((store, device)) = &self.namespace {
            super::super::offline::Cache::forget_namespace(store, device).await?;
        } else {
            super::super::offline::Cache::forget().await?;
        }
        *self = Self::new();
        feed.update(|feed| *feed = Feed::default());
        Ok(())
    }

    /// Poll committed revisions on both carriers, including while no transport exists.
    pub async fn refresh_local(&mut self, feed: Signal<Feed>) {
        if let Some(outbox) = &mut self.outbox {
            match outbox.refresh().await {
                Ok(true) => self.project_local(feed),
                Ok(false) => {}
                Err(problem) => feed.update(|feed| feed.problem = Some(problem)),
            }
        }
    }

    pub(super) fn project_local(&self, feed: Signal<Feed>) {
        let Some(outbox) = &self.outbox else {
            return;
        };
        feed.update(|feed| {
            feed.drafts = outbox
                .saved
                .drafts
                .iter()
                .map(|(key, value)| (Id::from(key), value.clone()))
                .collect();
            feed.commands.retain(|id, state| {
                state.method != ASSISTANT_SEND
                    || outbox.saved.messages.contains_key(id.as_str())
                    || matches!(state.status, CommandStatus::Done | CommandStatus::Abandoned)
            });
            for (id, command) in &outbox.saved.commands {
                let mut state = command_state(
                    feed,
                    &command.method,
                    command.params.clone(),
                    CommandStatus::Unknown,
                );
                state.result = command.result.clone();
                state.error = command.error.as_ref().map(|(code, message)| ClientError {
                    code: code.clone(),
                    message: message.clone(),
                });
                if let Some(result) = &state.result {
                    state.status = result_status(result);
                } else if let Some(error) = &state.error {
                    state.status = error_status(error);
                }
                feed.commands.entry(Id::from(id)).or_insert(state);
            }
            for (id, message) in &outbox.saved.messages {
                let mut state = command_state(
                    feed,
                    ASSISTANT_SEND,
                    value(&message.request),
                    if message.attempted {
                        CommandStatus::Unknown
                    } else {
                        CommandStatus::Queued
                    },
                );
                if let Some((code, explanation)) = &message.error {
                    state.error = Some(ClientError {
                        code: code.clone(),
                        message: explanation.clone(),
                    });
                    state.status = if message.refused {
                        CommandStatus::Refused
                    } else {
                        CommandStatus::Unknown
                    };
                }
                let state = feed.commands.entry(Id::from(id)).or_insert(state);
                if message.cancel {
                    feed.cancelling.insert(Id::from(id));
                    state.status = CommandStatus::Unknown;
                }
            }
            for id in &outbox.saved.cancelled {
                feed.cancelling.remove(id.as_str());
                if let Some(state) = feed.commands.get_mut(id.as_str()) {
                    state.status = CommandStatus::Abandoned;
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
