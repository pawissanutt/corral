use super::*;

impl AppState {
    pub async fn request_async(
        &mut self,
        feed: Signal<Feed>,
        request: ClientRequest,
    ) -> Vec<Outgoing> {
        self.refresh_local(feed).await;
        let request = match self.device_request(feed, request) {
            Ok(outgoing) => return outgoing,
            Err(request) => *request,
        };
        match request {
            ClientRequest::PushStatus { request_id } => {
                self.operation(feed, request_id, PUSH_STATUS, json!({}))
            }
            ClientRequest::BrowserPush { request_id, .. } => {
                feed.update(|feed| {
                    trim_operations(feed);
                    feed.operations.insert(
                        Id::from(request_id),
                        Operation {
                            method: PUSH_SUBSCRIBE.into(),
                            loading: false,
                            value: None,
                            error: Some(ClientError {
                                code: "unsupported".into(),
                                message:
                                    "Push delivery requires a browser with Service Worker support."
                                        .into(),
                            }),
                        },
                    );
                });
                vec![]
            }
            #[cfg(not(target_arch = "wasm32"))]
            ClientRequest::Forget => {
                let namespace = self.namespace.clone();
                match self.forget_local(feed).await {
                    Ok(()) => {
                        if let Some((store, _)) = namespace {
                            let removed =
                                blocking(move || super::super::credentials::remove(&store))
                                    .await
                                    .map_err(|error| error.to_string())
                                    .and_then(|result| result);
                            if let Err(problem) = removed {
                                feed.update(|feed| {
                                    feed.device = crate::model::devices::DeviceLink::Failed(problem)
                                });
                            }
                        }
                    }
                    Err(problem) => feed.update(|feed| feed.problem = Some(problem)),
                }
                vec![]
            }
            ClientRequest::RetryTopic(key) => self.retry_topic(feed, &key),
            ClientRequest::Terminal { session, open } => self.terminal_request(feed, session, open),
            ClientRequest::DetailWatch { target, open } => {
                self.watch_detail(feed, target, open);
                vec![]
            }
            ClientRequest::SaveDraft { thread, text } => {
                let result = match self.outbox.as_mut() {
                    Some(outbox) => {
                        outbox
                            .update_async(|saved| {
                                if text.is_empty() {
                                    saved.drafts.remove(&thread);
                                } else {
                                    saved.drafts.insert(thread.clone(), text.clone());
                                }
                            })
                            .await
                    }
                    None => Err("Pair/enrol before saving a device draft.".to_owned()),
                };
                match result {
                    Ok(()) => feed.update(|feed| {
                        if text.is_empty() {
                            feed.drafts.remove(thread.as_str());
                        } else {
                            feed.drafts.insert(thread.into(), text);
                        }
                    }),
                    Err(problem) => feed.update(|feed| feed.problem = Some(problem)),
                }
                vec![]
            }
            ClientRequest::AssistantSend(request) => self.send_message_async(feed, request).await,
            ClientRequest::AssistantCancel(id) => self.cancel_message_async(feed, id).await,
            ClientRequest::Attach(request) => self.attach(feed, request),
            ClientRequest::AttachDiscard(upload) => self.discard_upload(feed, upload),
            #[cfg(not(target_arch = "wasm32"))]
            ClientRequest::AttachPath(request) => self.attach_path(feed, request),
            #[cfg(not(target_arch = "wasm32"))]
            ClientRequest::AttachLoaded {
                upload,
                kind,
                name,
                result,
            } => self.attach_loaded(feed, upload, kind, name, result),
            ClientRequest::ActSubmit(request) => {
                self.command(feed, ACT_SUBMIT, request.cmd_id.clone(), value(request))
                    .await
            }
            ClientRequest::PromptAnswer(request) => {
                self.command(feed, PROMPT_ANSWER, request.cmd_id.clone(), value(request))
                    .await
            }
            ClientRequest::ActResult(request) => {
                if self.ready {
                    vec![out(ACT_RESULT, value(&request), Some(request.cmd_id))]
                } else {
                    feed.update(|feed| {
                        if let Some(command) = feed.commands.get_mut(request.cmd_id.as_str()) {
                            command.error = Some(offline());
                        }
                    });
                    vec![]
                }
            }
            ClientRequest::Thread {
                scope,
                open: should_open,
            } => {
                let retry = feed.with_untracked(|feed| {
                    feed.topics
                        .get(&topic_key(Topic::Thread, Some(&scope)))
                        .is_some_and(|state| state.error.is_some())
                });
                if should_open && self.scopes.contains(&scope) && self.ready && !retry {
                    return vec![];
                }
                if should_open && !self.scopes.contains(&scope) && self.scopes.len() >= 32 {
                    feed.update(|feed| {
                        feed.problem = Some(
                            "This device already has 32 subscribed threads; close one first."
                                .into(),
                        )
                    });
                    return vec![];
                }
                if should_open {
                    self.scopes.insert(scope.clone());
                } else {
                    self.scopes.remove(&scope);
                }
                feed.update(|feed| {
                    let thread = feed.threads.entry(Id::from(&scope)).or_default();
                    thread.loading = should_open && self.ready;
                    thread.stale = true;
                    if should_open && self.ready {
                        feed.topics.insert(
                            topic_key(Topic::Thread, Some(&scope)),
                            TopicState {
                                loading: true,
                                stale: true,
                                error: None,
                            },
                        );
                    }
                });
                self.touch_thread(feed, &scope);
                self.trim_retained(feed);
                if !self.ready {
                    return vec![];
                }
                vec![if should_open {
                    open(Topic::Thread, Some(scope))
                } else {
                    out(
                        wire::SUB_CLOSE,
                        value(SubClose {
                            topic: Topic::Thread,
                            scope: Some(scope),
                        }),
                        None,
                    )
                }]
            }
            ClientRequest::Detail {
                request_id,
                request,
            } => self.detail_request(feed, request_id, request),
            ClientRequest::SessionItem { session, token } => {
                self.session_item(feed, session, token)
            }
            ClientRequest::Watch { session, on } => {
                let transition = if on {
                    let count = self.session_scopes.entry(session.clone()).or_default();
                    *count = count.saturating_add(1);
                    *count == 1
                } else if let Some(count) = self.session_scopes.get_mut(&session) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        self.session_scopes.remove(&session);
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
                if !on {
                    feed.update(|feed| {
                        feed.live.remove(session.as_str());
                        feed.topics
                            .remove(&topic_key(Topic::Session, Some(&session)));
                    });
                }
                self.trim_retained(feed);
                if !self.ready || !feed.with_untracked(|feed| feed.capable(SESSION_V1)) {
                    return vec![];
                }
                if on {
                    feed.update(|feed| {
                        feed.topics.insert(
                            topic_key(Topic::Session, Some(&session)),
                            TopicState {
                                loading: true,
                                stale: true,
                                error: None,
                            },
                        );
                    });
                    vec![open(Topic::Session, Some(session))]
                } else {
                    vec![out(
                        wire::SUB_CLOSE,
                        value(SubClose {
                            topic: Topic::Session,
                            scope: Some(session),
                        }),
                        None,
                    )]
                }
            }
            ClientRequest::ActPreview {
                request_id,
                request,
            } => self.operation(feed, request_id, ACT_PREVIEW, value(request)),
            ClientRequest::PromptPreview {
                request_id,
                request,
            } => self.operation(feed, request_id, PROMPT_PREVIEW, value(request)),
            ClientRequest::AssistantWarm {
                request_id,
                request,
            } => self.operation(feed, request_id, ASSISTANT_WARM, value(request)),
            ClientRequest::AssistantStop {
                request_id,
                request,
            } => self.operation(feed, request_id, ASSISTANT_STOP, value(request)),
            ClientRequest::AssistantSession {
                request_id,
                request,
            } => self.operation(feed, request_id, ASSISTANT_SESSION, value(request)),
            ClientRequest::ChangesPage {
                request_id,
                request,
            } => self.operation(feed, request_id, CHANGES_PAGE, value(request)),
            ClientRequest::SeenAdvance {
                request_id,
                request,
            } => self.operation(feed, request_id, SEEN_ADVANCE, value(request)),
            _ => vec![],
        }
    }
    pub(in crate::client) fn operation(
        &self,
        feed: Signal<Feed>,
        id: String,
        method: &'static str,
        params: Value,
    ) -> Vec<Outgoing> {
        feed.update(|feed| {
            trim_operations(feed);
            feed.operations.insert(
                Id::from(&id),
                Operation {
                    method: method.into(),
                    loading: self.ready,
                    value: None,
                    error: (!self.ready).then(|| offline()),
                },
            );
        });
        if self.ready {
            vec![out(method, params, Some(id))]
        } else {
            vec![]
        }
    }
    async fn command(
        &mut self,
        feed: Signal<Feed>,
        method: &'static str,
        id: String,
        params: Value,
    ) -> Vec<Outgoing> {
        trim_commands(feed);
        let prior = feed.with_untracked(|feed| feed.commands.get(id.as_str()).cloned());
        if let Some(prior) = prior {
            let previous = prior
                .request
                .get("_request_digest")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| request_digest(&prior.method, &prior.request));
            if previous != request_digest(method, &params) || prior.method != method {
                feed.update(|feed| {
                    feed.problem =
                        Some("Command identity already belongs to different input.".into())
                });
                return vec![];
            }
            let retry_refused = prior.status == CommandStatus::Refused
                && prior.error.as_ref().is_some_and(|error| {
                    matches!(
                        error.code.as_str(),
                        "offline" | "local_storage" | "confirm_required"
                    )
                });
            if !retry_refused {
                return if self.ready
                    && prior.result.is_none()
                    && matches!(
                        prior.status,
                        CommandStatus::Pending | CommandStatus::Running | CommandStatus::Unknown
                    ) {
                    vec![recover(&id)]
                } else {
                    vec![]
                };
            }
        }
        let mut state = feed.with_untracked(|feed| {
            command_state(feed, method, params.clone(), CommandStatus::Pending)
        });
        if !self.ready {
            state.status = CommandStatus::Refused;
            state.error = Some(offline());
        } else if let Err(problem) = super::super::core::preflight_request(method, &params) {
            state.status = CommandStatus::Refused;
            state.error = Some(ClientError {
                code: "local_admission".into(),
                message: problem,
            });
        } else {
            // Persist only recovery identity, never arbitrary command bodies or credentials.
            let safe = json!({"_request_digest": request_digest(method, &params)});
            let result = match self.outbox.as_mut() {
                Some(outbox) => outbox.update_checked(|saved| {
                            if let Some(prior) = saved.commands.get(&id) {
                                if prior.method != method || prior.params != safe {
                                    return Err("Command identity already belongs to different input.".into());
                                }
                                if prior.result.is_some() {
                                    return Err("This command already has a recorded outcome; recover that outcome instead.".into());
                                }
                            }
                            if saved.commands.len() >= 256 && !saved.commands.contains_key(&id) {
                                let evict = saved
                                    .commands
                                    .iter()
                                    .find(|(_, command)| command.is_settled())
                                    .map(|(id, _)| id.clone());
                                if let Some(evict) = evict {
                                    saved.commands.remove(&evict);
                                }
                            }
                            saved.commands.insert(
                                id.clone(),
                                outbox::Command {
                                    method: method.into(),
                                    params: safe.clone(),
                                    result: None,
                                    error: None,
                                },
                            );
                            Ok(())
                }).await,
                None => Err("Device recovery storage unavailable.".to_owned()),
            };
            if let Err(problem) = result {
                state.status = CommandStatus::Refused;
                state.error = Some(local_error(problem));
            }
        }
        let send = state.status == CommandStatus::Pending;
        feed.update(|feed| {
            feed.commands.insert(Id::from(&id), state);
        });
        if send {
            vec![out(method, params, Some(id))]
        } else {
            vec![]
        }
    }
    async fn send_message_async(
        &mut self,
        feed: Signal<Feed>,
        request: AssistantSend,
    ) -> Vec<Outgoing> {
        trim_commands(feed);
        let id = request.cmd_id.clone();
        let params = value(&request);
        let existing = self
            .outbox
            .as_ref()
            .and_then(|outbox| outbox.saved.messages.get(&id));
        if existing.is_some_and(|message| message.request != request) {
            self.refuse_send(
                feed,
                &request,
                "Message identity already belongs to different text.".into(),
                false,
            )
            .await;
            return vec![];
        }
        if existing.is_some_and(|message| message.cancel) {
            return self.cancel_message_async(feed, id).await;
        }
        if let Err(problem) = super::super::core::preflight_request(ASSISTANT_SEND, &params) {
            self.refuse_send(feed, &request, problem, true).await;
            return vec![];
        }
        if !outbox::valid_message(&request)
            || (existing.is_none() && !self.ready_uploads(feed, &request))
        {
            self.refuse_send(
                feed,
                &request,
                "Message needs text and successfully uploaded attachments (maximum four).".into(),
                true,
            )
            .await;
            return vec![];
        }
        if feed.with_untracked(|feed| {
            feed.commands.get(id.as_str()).is_some_and(|state| {
                matches!(state.status, CommandStatus::Done | CommandStatus::Abandoned)
                    || (self.ready && state.status == CommandStatus::Pending)
            })
        }) {
            return vec![];
        }
        let attempted = self.ready || existing.is_some_and(|message| message.attempted);
        let mut consumed_draft = None;
        let result = match self.outbox.as_mut() {
            Some(outbox) => {
                outbox
                    .update_checked(|saved| {
                        consumed_draft = None;
                        if saved.cancelled.contains(&id) {
                            return Err("This message was withdrawn on this device.".into());
                        }
                        let first_acceptance = !saved.messages.contains_key(&id);
                        if let Some(message) = saved.messages.get_mut(&id) {
                            if message.request != request {
                                return Err(
                                    "Message identity already belongs to different text.".into()
                                );
                            }
                            if message.cancel {
                                return Err(
                                "This message has a committed cancellation; it cannot be resent."
                                    .into(),
                            );
                            }
                            message.attempted |= attempted;
                        } else {
                            saved.messages.insert(
                                id.clone(),
                                outbox::Message {
                                    request: request.clone(),
                                    attempted,
                                    cancel: false,
                                    refused: false,
                                    error: None,
                                },
                            );
                        }
                        // The debounce may have saved an earlier prefix of this composer value.
                        // A replay must never consume a draft typed after its original handoff.
                        if first_acceptance
                            && saved
                                .drafts
                                .get(&request.thread)
                                .is_some_and(|draft| request.text.trim().starts_with(draft.trim()))
                        {
                            consumed_draft = saved.drafts.remove(&request.thread);
                        }
                        Ok(())
                    })
                    .await
            }
            None => Err("Pair/enrol before queuing a message on this device.".to_owned()),
        };
        let mut state = feed.with_untracked(|feed| {
            command_state(
                feed,
                ASSISTANT_SEND,
                params.clone(),
                if self.ready {
                    CommandStatus::Pending
                } else if attempted {
                    CommandStatus::Unknown
                } else {
                    CommandStatus::Queued
                },
            )
        });
        if let Err(problem) = &result {
            state.status = CommandStatus::Refused;
            state.error = Some(local_error(problem.clone()));
        }
        let send = state.status == CommandStatus::Pending;
        feed.update(|feed| {
            feed.send_acceptance.insert(
                Id::from(&id),
                match &result {
                    Ok(()) => SendAcceptance::Accepted,
                    Err(problem) => SendAcceptance::Refused(local_error(problem.clone())),
                },
            );
            if result.is_ok()
                && consumed_draft
                    .as_ref()
                    .is_some_and(|draft| feed.drafts.get(request.thread.as_str()) == Some(draft))
            {
                feed.drafts.remove(request.thread.as_str());
            }
            feed.commands.insert(Id::from(&id), state);
        });
        if result.is_ok() {
            self.release_upload_bytes(feed, &request);
        }
        if send {
            vec![out(ASSISTANT_SEND, params, Some(id))]
        } else {
            vec![]
        }
    }

    async fn refuse_send(
        &mut self,
        feed: Signal<Feed>,
        request: &AssistantSend,
        message: String,
        quarantine: bool,
    ) {
        let error = ClientError {
            code: "local_admission".into(),
            message,
        };
        feed.update(|feed| {
            feed.problem = Some(error.message.clone());
            feed.send_acceptance.insert(
                Id::from(&request.cmd_id),
                SendAcceptance::Refused(error.clone()),
            );
            if quarantine {
                let mut state =
                    command_state(feed, ASSISTANT_SEND, value(request), CommandStatus::Refused);
                state.error = Some(error.clone());
                feed.commands.insert(Id::from(&request.cmd_id), state);
            }
        });
        if quarantine {
            // Legacy oversized entries stay editable/cancellable, but never poison replay.
            self.settled(feed, &request.cmd_id).await;
        }
    }
    #[cfg(test)]
    pub fn request(&mut self, feed: Signal<Feed>, request: ClientRequest) -> Vec<Outgoing> {
        super::super::durable::wait(self.request_async(feed, request))
    }
    #[cfg(test)]
    fn send_message(&mut self, feed: Signal<Feed>, request: AssistantSend) -> Vec<Outgoing> {
        super::super::durable::wait(self.send_message_async(feed, request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_offline_send_consumes_only_its_matching_persisted_draft() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed::default());
        let mut app = AppState::new();
        app.outbox = Some(Outbox::load("draft-send-handoff", "draft-device").unwrap());
        let save = |app: &mut AppState, scope: &str, text: &str| {
            app.request(
                feed,
                ClientRequest::SaveDraft {
                    thread: scope.into(),
                    text: text.into(),
                },
            );
        };
        let send = AssistantSend {
            cmd_id: "one-owner-intent".into(),
            thread: "A-3".into(),
            target: AskTarget::Assistant,
            text: "\towner words \n".into(),
            typed_at: "2026-11-14T10:10:00Z".into(),
            context: Vec::new(),
            attachments: Vec::new(),
        };
        save(&mut app, "A-3", "  owner");
        save(&mut app, "A-4", "another thread's draft");
        assert!(
            app.request(feed, ClientRequest::AssistantSend(send.clone()))
                .is_empty()
        );
        assert!(feed.with_untracked(|feed| !feed.drafts.contains_key("A-3")));
        assert_eq!(
            feed.with_untracked(|feed| feed.drafts["A-4"].clone()),
            "another thread's draft"
        );
        assert_eq!(
            feed.with_untracked(|feed| feed.commands["one-owner-intent"].status),
            CommandStatus::Queued
        );

        let persisted = Outbox::load("draft-send-handoff", "draft-device").unwrap();
        assert!(!persisted.saved.drafts.contains_key("A-3"));
        assert_eq!(persisted.saved.drafts["A-4"], "another thread's draft");
        assert_eq!(persisted.saved.messages.len(), 1);
        assert_eq!(persisted.saved.messages["one-owner-intent"].request, send);
        assert!(!persisted.saved.messages["one-owner-intent"].attempted);

        // A replay of this same identity must not consume a newer composer value.
        save(&mut app, "A-3", "  owner words\n");
        assert!(
            app.request(feed, ClientRequest::AssistantSend(send))
                .is_empty()
        );
        let persisted = Outbox::load("draft-send-handoff", "draft-device").unwrap();
        assert_eq!(persisted.saved.messages.len(), 1);
        assert_eq!(persisted.saved.drafts["A-3"], "  owner words\n");
        assert_eq!(
            feed.with_untracked(|feed| feed.drafts["A-3"].clone()),
            "  owner words\n"
        );

        // Validation refusal never hands the draft over to the durable message outbox.
        save(&mut app, "A-5", "keep this draft");
        let invalid = AssistantSend {
            cmd_id: "invalid-intent".into(),
            thread: "A-5".into(),
            target: AskTarget::Assistant,
            text: " \n".into(),
            typed_at: "2026-11-14T10:11:00Z".into(),
            context: Vec::new(),
            attachments: Vec::new(),
        };
        assert!(
            app.request(feed, ClientRequest::AssistantSend(invalid))
                .is_empty()
        );
        let persisted = Outbox::load("draft-send-handoff", "draft-device").unwrap();
        assert_eq!(persisted.saved.drafts["A-5"], "keep this draft");
        assert_eq!(
            feed.with_untracked(|feed| feed.drafts["A-5"].clone()),
            "keep this draft"
        );
        assert!(!persisted.saved.messages.contains_key("invalid-intent"));
    }

    #[test]
    fn full_outbox_refuses_before_retiring_the_composer_draft_and_attachments() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed::default());
        let mut app = AppState::new();
        let mut outbox = Outbox::load("full-outbox-regression", "device").unwrap();
        let request = AssistantSend {
            cmd_id: "keep-intent".into(),
            thread: QUICK_THREAD.into(),
            target: AskTarget::Assistant,
            text: "keep my newest draft".into(),
            typed_at: "2026-11-14T10:00:00Z".into(),
            context: vec![],
            attachments: vec![SendAttachment::Link {
                url: "https://example.com/owner".into(),
                title: None,
            }],
        };
        outbox
            .update(|saved| {
                saved
                    .drafts
                    .insert(QUICK_THREAD.into(), request.text.clone());
                for n in 0..128 {
                    let mut queued = request.clone();
                    queued.cmd_id = format!("queued-{n}");
                    saved.messages.insert(
                        queued.cmd_id.clone(),
                        outbox::Message {
                            request: queued,
                            attempted: false,
                            cancel: false,
                            refused: false,
                            error: None,
                        },
                    );
                }
            })
            .unwrap();
        app.outbox = Some(outbox);
        feed.update(|feed| {
            feed.drafts
                .insert(Id::from(QUICK_THREAD), request.text.clone());
        });
        assert!(app.send_message(feed, request.clone()).is_empty());
        assert!(feed.with_untracked(|feed| matches!(
            feed.send_acceptance.get("keep-intent"),
            Some(SendAcceptance::Refused(_))
        )));
        assert_eq!(
            feed.with_untracked(|feed| feed.drafts[QUICK_THREAD].clone()),
            request.text
        );
        let saved = Outbox::load("full-outbox-regression", "device").unwrap();
        assert_eq!(saved.saved.drafts[QUICK_THREAD], request.text);
        assert!(!saved.saved.messages.contains_key("keep-intent"));
        assert_eq!(
            feed.with_untracked(|feed| feed.commands["keep-intent"].request["attachments"].clone()),
            value(&request.attachments)
        );
    }
}
