use super::*;

impl AppState {
    pub async fn event_async(
        &mut self,
        feed: Signal<Feed>,
        event: Event,
    ) -> Result<Vec<Outgoing>, String> {
        if self.ready
            && let Err(problem) = self.offline.capture(&event).await
        {
            feed.update(|feed| feed.problem = Some(problem));
        }
        match event {
            Event::Health(health) => {
                feed.update(|feed| feed.health_received(health));
                Ok(self.poll_changes(feed))
            }
            Event::Rows { delta, replacement } => self.rows(feed, delta, replacement),
            Event::Live { scope, live } => {
                self.live(feed, scope, live)?;
                Ok(vec![])
            }
            Event::TopicError {
                topic,
                scope,
                code,
                message,
            } => {
                if topic == Topic::Terminal
                    && let Some(scope) = &scope
                {
                    self.terminal_capture(feed, scope.clone(), None, Some(message));
                    return Ok(vec![]);
                }
                feed.update(|feed| {
                    let state = feed
                        .topics
                        .entry(topic_key(topic, scope.as_deref()))
                        .or_default();
                    state.error = Some(ClientError {
                        code: value(code).as_str().unwrap_or("unknown").into(),
                        message,
                    });
                    state.stale = true;
                    state.loading = false;
                    if topic == Topic::Thread
                        && let Some(scope) = scope
                    {
                        let thread = feed.threads.entry(scope.into()).or_default();
                        thread.stale = true;
                        thread.loading = false;
                    }
                });
                Ok(vec![])
            }
            Event::Reset {
                topic,
                scope,
                epoch,
            } => {
                feed.update(|feed| {
                    if topic == Topic::Tasks {
                        feed.begin_epoch(epoch);
                        feed.projection_ready = false;
                        feed.link = Link::Connecting;
                        feed.problem = Some("Waiting for the replacement task snapshot.".into());
                    }
                    let state = feed
                        .topics
                        .entry(topic_key(topic, scope.as_deref()))
                        .or_default();
                    state.loading = true;
                    state.stale = true;
                    for detail in feed.details.values_mut() {
                        if rows::topic_affects(&detail.target, topic) {
                            detail.stale = true;
                        }
                    }
                    if let Some(scope) = scope {
                        if topic == Topic::Terminal {
                            if let Some(cache) = feed
                                .details
                                .get_mut(&detail_key(&DetailRef::Terminal { id: scope.clone() }))
                            {
                                cache.stale = true;
                                cache.loading = true;
                            }
                        }
                        if topic == Topic::Session {
                            if self.session_scopes.contains_key(&scope) {
                                feed.live.entry(scope.into()).or_default().reset();
                            }
                        } else if topic == Topic::Thread {
                            let thread = feed.threads.entry(scope.into()).or_default();
                            thread.stale = true;
                            thread.loading = true;
                            thread.live = None;
                        }
                    }
                });
                Ok(vec![])
            }
            Event::Reply {
                method,
                tag,
                params,
                outcome,
                error_data,
            } => {
                self.reply(feed, method, tag, params, outcome, error_data)
                    .await
            }
            _ => Ok(vec![]),
        }
    }
    async fn reply(
        &mut self,
        feed: Signal<Feed>,
        method: &'static str,
        tag: Option<String>,
        params: Value,
        outcome: Result<Value, (corral_protocol::rpc::ErrorCode, String)>,
        error_data: Option<Value>,
    ) -> Result<Vec<Outgoing>, String> {
        if method == DETAIL_GET {
            return self.detail_reply(feed, tag, params, outcome);
        }
        if matches!(
            method,
            ATTACHMENT_BEGIN | ATTACHMENT_PUT | ATTACHMENT_DISCARD
        ) {
            return self.upload_reply(feed, method, tag, params, outcome, error_data);
        }
        if method == ASSISTANT_CANCEL {
            return self
                .cancel_reply(feed, tag, params, outcome, error_data)
                .await;
        }
        if method == CHANGES_PAGE {
            self.changes_inflight = false;
        }
        let id = tag.or_else(|| params["cmd_id"].as_str().map(str::to_owned));
        if method == ASSISTANT_SEND
            && id.as_ref().is_some_and(|id| {
                feed.with_untracked(|feed| {
                    feed.commands.get(id.as_str()).is_some_and(|state| {
                        matches!(state.status, CommandStatus::Done | CommandStatus::Abandoned)
                    })
                })
            })
        {
            return Ok(vec![]);
        }
        let result = match outcome {
            Ok(value) => value,
            Err((code, message)) => {
                if method == APP_SESSION {
                    // A valid RPC failure must not strand a heartbeat-healthy bootstrap.
                    return Err(format!("App session initialization failed: {message}"));
                }
                let outer_code = value(code);
                let error = ClientError {
                    code: error_data
                        .as_ref()
                        .and_then(|data| data["code"].as_str())
                        .or_else(|| outer_code.as_str())
                        .unwrap_or("unknown")
                        .into(),
                    message,
                };
                // A failed first page, or a later page of the walk in progress, leaves the
                // registry unlisted on this connection.
                let walk_failed = method == DEVICE_PAGE
                    && (params["after"].is_null()
                        || self
                            .device_walk
                            .as_ref()
                            .is_some_and(|(current, _)| id.as_ref() == Some(current)));
                if walk_failed && !params["after"].is_null() {
                    self.device_walk = None;
                }
                // The engine's refusal travels in the error's data, with the steps past it.
                let refusal = (method == OP_SUBMIT
                    && code == corral_protocol::rpc::ErrorCode::Refused)
                    .then(|| {
                        error_data
                            .clone()
                            .and_then(|data| serde_json::from_value::<Refusal>(data).ok())
                    })
                    .flatten();
                let mut conflict = false;
                feed.update(|feed| {
                    if let Some(id) = &id {
                        if let Some(state) = feed.commands.get_mut(id.as_str()) {
                            state.status = if method == ASSISTANT_SEND {
                                if self
                                    .outbox
                                    .as_ref()
                                    .and_then(|outbox| outbox.saved.messages.get(id))
                                    .is_some_and(|message| message.cancel)
                                {
                                    CommandStatus::Unknown
                                } else if matches!(
                                    code,
                                    corral_protocol::rpc::ErrorCode::Refused
                                        | corral_protocol::rpc::ErrorCode::Conflict
                                        | corral_protocol::rpc::ErrorCode::InvalidParams
                                ) {
                                    if code == corral_protocol::rpc::ErrorCode::Conflict {
                                        CommandStatus::Conflict
                                    } else {
                                        CommandStatus::Refused
                                    }
                                } else {
                                    CommandStatus::Unknown
                                }
                            } else {
                                error_status(&error)
                            };
                            conflict = state.status == CommandStatus::Conflict;
                            state.error = Some(error.clone());
                        }
                        if let Some(operation) = feed.operations.get_mut(id.as_str()) {
                            operation.loading = false;
                            operation.error = Some(error.clone());
                            operation.refusal = refusal.clone();
                        }
                    }
                    if refusal.is_none() {
                        feed.problem = Some(error.message.clone());
                    }
                    if matches!(
                        method,
                        wire::device::PAIR_BEGIN
                            | wire::device::PAIR_STATUS
                            | wire::device::PAIR_CONFIRM
                            | wire::device::PAIR_CANCEL
                    ) {
                        feed.pairing = crate::model::devices::Pairing::Ended {
                            line: error.message.clone(),
                        };
                    }
                    if walk_failed {
                        feed.registry = Registry::Failed;
                    }
                });
                if let Some(id) = &id {
                    self.settled(feed, id).await;
                }
                return Ok(if conflict {
                    self.refresh_details(feed)
                } else {
                    vec![]
                });
            }
        };
        if let Some(id) = &id {
            feed.update(|feed| {
                if let Some(operation) = feed.operations.get_mut(id.as_str()) {
                    operation.loading = false;
                    operation.value = Some(result.clone());
                    operation.error = None;
                }
            });
        }
        match method {
            APP_SESSION => self.session(feed, decode(result)?).await,
            PROMPT_ANSWER => {
                if let Some(id) = &id {
                    feed.update(|feed| {
                        if let Some(state) = feed.commands.get_mut(id.as_str()) {
                            state.status = CommandStatus::Done;
                            state.error = None;
                        }
                    });
                    self.settled(feed, id).await;
                }
                Ok(vec![])
            }
            OP_SUBMIT => Ok(self.refresh_details(feed)),
            ASSISTANT_SEND => {
                let ack: SendAck = decode(result)?;
                if let Some(id) = id {
                    if feed.with_untracked(|feed| {
                        feed.commands
                            .get(id.as_str())
                            .is_some_and(|state| state.status == CommandStatus::Abandoned)
                    }) {
                        return Ok(vec![]);
                    }
                    let cancelling = self
                        .outbox
                        .as_ref()
                        .and_then(|outbox| outbox.saved.messages.get(&id))
                        .is_some_and(|message| message.cancel);
                    feed.update(|feed| {
                        if let Some(state) = feed.commands.get_mut(id.as_str()) {
                            state.status = if cancelling {
                                CommandStatus::Unknown
                            } else {
                                CommandStatus::Done
                            };
                            state.error = None;
                            state.entry = Some(Id::from(&ack.entry));
                        }
                    });
                    // Settle first: retiring a Done send must never skip clearing its saved copy.
                    self.settled(feed, &id).await;
                    feed.update(Feed::retire_echoed_sends);
                }
                Ok(vec![])
            }
            CHANGES_PAGE => {
                let page: ChangesPage = decode(result)?;
                let before = params["before"].as_u64();
                let after = params["after"].as_u64();
                let last = page.events.iter().map(|event| event.seq).max();
                let head = page.head;
                if before.is_none() {
                    if let Some(last) = last {
                        self.changes_cursor = Some(self.changes_cursor.unwrap_or(0).max(last));
                    }
                }
                feed.update(|feed| {
                    if page.head >= feed.changes_head
                        && feed
                            .seen
                            .as_ref()
                            .is_none_or(|seen| page.seen.through >= seen.through)
                    {
                        feed.seen_received(page.seen.clone());
                        feed.changes_unseen = page.unseen.map(|unseen| (page.seen.through, unseen));
                    }
                    for event in page.events {
                        feed.changes.insert(event.seq, event);
                    }
                    while feed.changes.len() > 10_000 {
                        if before.is_some() {
                            feed.changes.pop_last();
                        } else {
                            feed.changes.pop_first();
                        }
                    }
                    feed.changes_head = feed.changes_head.max(page.head);
                    if after.is_none() {
                        feed.changes_older = page.older;
                    }
                });
                if after.is_some() && last.is_some_and(|last| last < head) {
                    return Ok(self.poll_changes(feed));
                }
                Ok(vec![])
            }
            SEEN_ADVANCE => {
                let seen: SeenCursor = decode(result)?;
                feed.update(|feed| {
                    feed.seen_received(seen.clone());
                    if let Some(session) = &mut feed.session {
                        session.seen = seen;
                    }
                });
                Ok(vec![])
            }
            CONFIG_GET | CONFIG_PUT | PANE_FOCUS | PANE_KEYS => Ok(vec![]),
            PUSH_STATUS | PUSH_SUBSCRIBE | PUSH_UNSUBSCRIBE => {
                let status: wire::device::PushStatus = decode(result)?;
                feed.update(|feed| feed.push = Some(status));
                Ok(vec![])
            }
            DEVICE_REVOKE
            | wire::device::PAIR_BEGIN
            | wire::device::PAIR_STATUS
            | wire::device::PAIR_CONFIRM
            | wire::device::PAIR_CANCEL => self.device_reply(feed, method, result),
            DEVICE_PAGE => {
                let page: DevicePage = decode(result)?;
                let id = id.ok_or("device page has no correlation identity")?;
                if params["after"].is_null() {
                    self.device_walk = Some((id.clone(), vec![]));
                }
                let Some((current, rows)) = &mut self.device_walk else {
                    return Ok(vec![]);
                };
                if *current != id {
                    return Ok(vec![]);
                }
                rows.extend(page.devices);
                if rows.len() > 256 {
                    return Err("device enumeration exceeds the enrolled-device bound".into());
                }
                if let Some(next) = page.next {
                    if params["after"]
                        .as_str()
                        .is_some_and(|after| next.as_str() <= after)
                    {
                        return Err("device page cursor did not advance".into());
                    }
                    return Ok(vec![out(
                        DEVICE_PAGE,
                        value(DevicePageQuery {
                            after: Some(next),
                            limit: DEVICE_PAGE_LIMIT_MAX,
                        }),
                        Some(id),
                    )]);
                }
                let rows = self
                    .device_walk
                    .take()
                    .map(|(_, rows)| rows)
                    .unwrap_or_default();
                feed.update(|feed| {
                    feed.devices_received(rows.into_iter().map(super::super::map::device).collect())
                });
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
    #[cfg(test)]
    pub fn event(&mut self, feed: Signal<Feed>, event: Event) -> Result<Vec<Outgoing>, String> {
        super::super::durable::wait(self.event_async(feed, event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::assistant::{State, ThreadItem, live_items};

    fn reply(app: &mut AppState, feed: Signal<Feed>, (method, params, tag): Outgoing, ok: Value) {
        let event = Event::Reply {
            method,
            tag,
            params,
            outcome: Ok(ok),
            error_data: None,
        };
        assert!(app.event(feed, event).unwrap().is_empty());
    }

    fn thread_page(app: &mut AppState, feed: Signal<Feed>, rows: Vec<ThreadEntry>) {
        let delta = SubDelta::Thread {
            scope: "A-3".into(),
            delta: RowsDelta {
                epoch: 1,
                rows,
                left: Vec::new(),
                cursor: corral_protocol::subscription::BoardCursor::default(),
                more: false,
            },
            live: None,
        };
        let event = Event::Rows {
            delta,
            replacement: true,
        };
        app.event(feed, event).unwrap();
    }

    fn owners(feed: Signal<Feed>) -> Vec<String> {
        feed.with_untracked(|feed| live_items(&State::default(), feed, "A-3"))
            .into_iter()
            .filter_map(|item| match item {
                ThreadItem::Owner(message) => Some(message.text),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn delivered_send_stays_retired_by_its_entry_after_minutes_of_change_polls() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed::default());
        feed.update(|feed| {
            feed.capabilities.insert(CHANGES_V1.into());
        });
        let mut app = AppState::new();
        app.outbox = Some(Outbox::load("send-echo-store", "dev-4c").unwrap());
        let send = |cmd_id: &str, text: &str| {
            ClientRequest::AssistantSend(AssistantSend {
                cmd_id: cmd_id.into(),
                thread: "A-3".into(),
                target: AskTarget::Assistant,
                text: text.into(),
                typed_at: "2026-11-14T10:10:00Z".into(),
                context: Vec::new(),
                attachments: vec![SendAttachment::Link {
                    url: "https://example.invalid/opaque".into(),
                    title: None,
                }],
            })
        };
        assert!(
            app.request(feed, send("app-1-offline", "queued words"))
                .is_empty()
        );
        app.ready = true;
        // An identity ordered before every generated one, so the earliest eviction reaches it.
        let mut sent = app.request(feed, send("app-0-sent", "inspect the evidence"));
        assert_eq!(sent.len(), 1);
        let ack = json!({"entry": "M-7", "state": {"state": "sent"}, "replayed": false});
        reply(&mut app, feed, sent.remove(0), ack);
        // Every corrald sync pass republishes health, and each one polls the change feed.
        let page =
            json!({"events": [], "head": 0, "older": null, "seen": {"through": 0, "at": null}});
        for _ in 0..600 {
            for poll in app.poll_changes(feed) {
                reply(&mut app, feed, poll, page.clone());
            }
        }
        let echo: ThreadEntry = serde_json::from_value(json!({
            "id": "M-7", "rev": 2, "at": "2026-11-14T10:10:01Z", "kind": "message",
            "device": "dev-4c", "via": null, "text": "inspect the evidence",
            "state": {"state": "delivered"}, "model": null, "cost_usd": null,
            "tools": [], "links": [], "card": null
        }))
        .unwrap();
        thread_page(&mut app, feed, vec![echo]);
        assert_eq!(owners(feed), ["inspect the evidence", "queued words"]);
        // A reopened bounded page that no longer holds the echo cannot revive the local copy.
        thread_page(&mut app, feed, Vec::new());
        assert_eq!(owners(feed), ["queued words"]);
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::model::SettingsSection;
    use crate::model::devices::DeviceLink;
    use crate::model::settings::live::phone_subtitle;

    fn info(id: &str) -> Value {
        json!({
            "id": id, "name": id, "platform": "native", "app_version": "0",
            "enrolled_at": "2026-11-14T09:00:00Z", "revoked_at": null
        })
    }

    fn session(app: &mut AppState, feed: Signal<Feed>) {
        feed.update(|feed| {
            feed.store
                .get_or_insert_with(|| "/registry-session-fixture".into());
        });
        let event = Event::Reply {
            method: APP_SESSION,
            tag: None,
            params: json!({}),
            outcome: Ok(json!({
                "device": info("dev-1"),
                "seen": {"through": 0, "at": null},
                "server_time": "2026-11-14T09:00:00Z"
            })),
            error_data: None,
        };
        app.event(feed, event).unwrap();
    }

    fn page(app: &mut AppState, feed: Signal<Feed>, devices: Vec<Value>) {
        let (method, params, tag) = super::super::devices();
        let event = Event::Reply {
            method,
            tag,
            params,
            outcome: Ok(json!({"devices": devices, "next": null})),
            error_data: None,
        };
        assert!(app.event(feed, event).unwrap().is_empty());
    }

    fn state(feed: Signal<Feed>) -> (Registry, usize, String) {
        feed.with_untracked(|feed| {
            (
                feed.registry,
                feed.devices.len(),
                phone_subtitle(feed, SettingsSection::Devices),
            )
        })
    }

    #[test]
    fn device_count_is_known_only_after_a_completed_walk_on_the_current_connection() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed::default());
        feed.update(|feed| {
            feed.link = Link::Up;
            feed.device = DeviceLink::Enrolled {
                id: "dev-1".into(),
                name: "dev-1".into(),
            };
        });
        let mut app = AppState::new();
        let (registry, _, unlisted) = state(feed);
        assert_eq!(registry, Registry::Unknown);

        // The session seeds this device for the Devices page; that is not the registry.
        session(&mut app, feed);
        assert_eq!(state(feed), (Registry::Unknown, 1, unlisted.clone()));

        // A completed empty walk is a known count, distinct from an unlisted registry, and a
        // later session refresh does not reseed it.
        page(&mut app, feed, Vec::new());
        session(&mut app, feed);
        let (registry, rows, empty) = state(feed);
        assert_eq!((registry, rows), (Registry::Current, 0));
        assert_ne!(empty, unlisted);

        // A lost connection makes the retained list unknown again.
        app.disconnected(feed);
        assert_eq!(state(feed).0, Registry::Unknown);
        assert_eq!(state(feed).2, unlisted);

        // Reconnected to another store: the walk answered before the session belongs to it.
        feed.update(|feed| feed.store = Some("/elsewhere/.corral".into()));
        page(&mut app, feed, vec![info("dev-1"), info("dev-2")]);
        session(&mut app, feed);
        assert_eq!(state(feed).0, Registry::Current);
        assert_eq!(state(feed).1, 2);

        // A refused listing on a fresh connection is neither a count nor still pending.
        app.disconnected(feed);
        let (method, params, tag) = super::super::devices();
        let refused = Event::Reply {
            method,
            tag,
            params,
            outcome: Err((
                corral_protocol::rpc::ErrorCode::Unavailable,
                "device registry unavailable".into(),
            )),
            error_data: None,
        };
        assert!(app.event(feed, refused).unwrap().is_empty());
        let (registry, _, failed) = state(feed);
        assert_eq!(registry, Registry::Failed);
        assert_ne!(failed, unlisted);
        assert_ne!(failed, empty);
    }
}

#[cfg(test)]
#[path = "op_submit_tests.rs"]
mod op_submit_tests;
