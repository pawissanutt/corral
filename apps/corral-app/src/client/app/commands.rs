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
                            refusal: None,
                        },
                    );
                });
                vec![]
            }
            #[cfg(unix)]
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
            ClientRequest::SessionSend(request) => self.send_message_async(feed, request).await,
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
            ClientRequest::Session {
                request_id,
                method,
                params,
            } => self.operation(feed, request_id, method, params),
            ClientRequest::OpSubmit {
                request_id,
                request,
            } => self.operation(feed, request_id, OP_SUBMIT, value(request)),
            ClientRequest::ConfigGet { request_id } => {
                self.operation(feed, request_id, CONFIG_GET, json!({}))
            }
            ClientRequest::ConfigPut {
                request_id,
                request,
            } => self.operation(feed, request_id, CONFIG_PUT, value(request)),
            ClientRequest::PiStartup {
                request_id,
                request,
            } => self.operation(
                feed,
                request_id,
                corral_protocol::config::PI_STARTUP,
                value(request),
            ),
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
            ClientRequest::AssistantWarm {
                request_id,
                request,
            } => self.operation(feed, request_id, ASSISTANT_WARM, value(request)),
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
                    refusal: None,
                },
            );
        });
        if self.ready {
            vec![out(method, params, Some(id))]
        } else {
            vec![]
        }
    }
    /// Takes a message into this device's outbox, in the same durable write that retires the
    /// session's saved draft its words came from, then sends it at once when the connection is
    /// up (it waits otherwise, and goes on the next connection under the same id). The reply is
    /// read under its command id; one this device refuses is written there with why, so the
    /// session model puts the words back in the draft and says why.
    async fn send_message_async(
        &mut self,
        feed: Signal<Feed>,
        request: SessionSend,
    ) -> Vec<Outgoing> {
        trim_commands(feed);
        let id = request.cmd_id.clone();
        let params = value(&request);
        if feed.with_untracked(|feed| feed.commands.contains_key(id.as_str())) {
            return vec![];
        }
        if let Err(problem) = super::super::core::preflight_request(SESSION_SEND, &params) {
            self.refuse_send(feed, &request, problem);
            return vec![];
        }
        if !outbox::valid_message(&request) || !self.ready_uploads(feed, &request) {
            self.refuse_send(
                feed,
                &request,
                "a message needs words or an attachment, and every attachment uploaded (four at most)"
                    .into(),
            );
            return vec![];
        }
        let attempted = self.ready;
        let mut consumed_draft = None;
        let result = match self.outbox.as_mut() {
            Some(outbox) => {
                outbox
                    .update_checked(|saved| {
                        consumed_draft = None;
                        if saved.messages.contains_key(&id) {
                            return Err("this message was already taken on this device".into());
                        }
                        saved.messages.insert(
                            id.clone(),
                            outbox::Message {
                                request: request.clone(),
                                attempted,
                            },
                        );
                        // The pause before saving may have kept an earlier prefix of these words.
                        if saved
                            .drafts
                            .get(&request.id)
                            .is_some_and(|draft| request.text.trim().starts_with(draft.trim()))
                        {
                            consumed_draft = saved.drafts.remove(&request.id);
                        }
                        Ok(())
                    })
                    .await
            }
            None => Err("pair or enrol this device before sending; nothing was sent".to_owned()),
        };
        if let Err(problem) = result {
            self.refuse_send(feed, &request, problem);
            return vec![];
        }
        let status = if attempted {
            CommandStatus::Pending
        } else {
            CommandStatus::Queued
        };
        feed.update(|feed| {
            if consumed_draft
                .as_ref()
                .is_some_and(|draft| feed.drafts.get(request.id.as_str()) == Some(draft))
            {
                feed.drafts.remove(request.id.as_str());
            }
            let state = command_state(feed, SESSION_SEND, params.clone(), status);
            feed.commands.insert(Id::from(&id), state);
            if attempted {
                trim_operations(feed);
                feed.operations.insert(
                    Id::from(&id),
                    Operation {
                        method: SESSION_SEND.into(),
                        loading: true,
                        value: None,
                        error: None,
                        refusal: None,
                    },
                );
            }
        });
        self.release_upload_bytes(feed, &request);
        if attempted {
            vec![out(SESSION_SEND, params, Some(id))]
        } else {
            vec![]
        }
    }

    /// This device did not take the message: it never left, and its operation says why.
    fn refuse_send(&mut self, feed: Signal<Feed>, request: &SessionSend, message: String) {
        let error = local_error(message);
        feed.update(|feed| {
            let mut state =
                command_state(feed, SESSION_SEND, value(request), CommandStatus::Refused);
            state.error = Some(error.clone());
            feed.commands.insert(Id::from(&request.cmd_id), state);
            trim_operations(feed);
            feed.operations.insert(
                Id::from(&request.cmd_id),
                Operation {
                    method: SESSION_SEND.into(),
                    loading: false,
                    value: None,
                    error: Some(error),
                    refusal: None,
                },
            );
        });
    }
    #[cfg(test)]
    pub fn request(&mut self, feed: Signal<Feed>, request: ClientRequest) -> Vec<Outgoing> {
        super::super::durable::wait(self.request_async(feed, request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full outbox refuses the message before anything is retired: the saved draft stays,
    /// nothing is kept, and the send's operation says why, so its words go back to the draft.
    #[test]
    fn a_full_outbox_refuses_before_retiring_the_draft() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed::default());
        let mut app = AppState::new();
        let mut outbox = Outbox::load("full-outbox-regression", "device").unwrap();
        let request = SessionSend {
            id: "quick".into(),
            cmd_id: "keep-intent".into(),
            text: "keep my newest draft".into(),
            policy: corral_protocol::session_state::SendPolicy::FollowUp,
            to: None,
            typed_at: Some("2026-11-14T10:00:00Z".into()),
            attachments: vec![SendAttachment::Link {
                url: "https://example.com/owner".into(),
                title: None,
            }],
        };
        outbox
            .update(|saved| {
                saved.drafts.insert("quick".into(), request.text.clone());
                for n in 0..128 {
                    let mut queued = request.clone();
                    queued.cmd_id = format!("queued-{n}");
                    saved.messages.insert(
                        queued.cmd_id.clone(),
                        outbox::Message {
                            request: queued,
                            attempted: false,
                        },
                    );
                }
            })
            .unwrap();
        app.outbox = Some(outbox);
        feed.update(|feed| {
            feed.drafts.insert(Id::from("quick"), request.text.clone());
        });
        assert!(
            app.request(feed, ClientRequest::SessionSend(request.clone()))
                .is_empty()
        );
        assert!(feed.with_untracked(|feed| {
            feed.operations["keep-intent"].error.is_some()
                && !feed.operations["keep-intent"].loading
        }));
        assert_eq!(
            feed.with_untracked(|feed| feed.drafts["quick"].clone()),
            request.text
        );
        let saved = Outbox::load("full-outbox-regression", "device").unwrap();
        assert_eq!(saved.saved.drafts["quick"], request.text);
        assert!(!saved.saved.messages.contains_key("keep-intent"));
    }
}
