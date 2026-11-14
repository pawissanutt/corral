use super::*;

impl AppState {
    pub fn request(&mut self, feed: Signal<Feed>, request: ClientRequest) -> Vec<Outgoing> {
        match request {
            ClientRequest::SaveDraft { thread, text } => {
                let result = self
                    .outbox
                    .as_mut()
                    .ok_or_else(|| "Pair/enrol before saving a device draft.".to_owned())
                    .and_then(|outbox| {
                        outbox.update(|saved| {
                            if text.is_empty() {
                                saved.drafts.remove(&thread);
                            } else {
                                saved.drafts.insert(thread.clone(), text.clone());
                            }
                        })
                    });
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
            ClientRequest::AssistantSend(request) => self.send_message(feed, request),
            ClientRequest::AssistantCancel(id) => self.cancel_message(feed, id),
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
            }
            ClientRequest::PromptAnswer(request) => {
                self.command(feed, PROMPT_ANSWER, request.cmd_id.clone(), value(request))
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
            ClientRequest::DeviceSetAuthority {
                request_id,
                request,
            } => self.operation(feed, request_id, DEVICE_SET_AUTHORITY, value(request)),
            _ => vec![],
        }
    }
    pub(super) fn operation(
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
    fn command(
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
                .unwrap_or_else(|| request_digest(&prior.request));
            if previous != request_digest(&params) || prior.method != method {
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
        } else {
            // Persist only recovery identity, never arbitrary command bodies or credentials.
            let safe = json!({"_request_digest": request_digest(&params)});
            let result =
                self.outbox
                    .as_mut()
                    .ok_or_else(|| "Device recovery storage unavailable.".to_owned())
                    .and_then(|outbox| {
                        outbox.update(|saved| {
                            if saved.commands.len() >= 256 && !saved.commands.contains_key(&id) {
                                let evict = saved
                                    .commands
                                    .iter()
                                    .find(|(_, command)| {
                                        command.error.is_some()
                                            || command.result.as_ref().is_some_and(|result| {
                                                result.state != CmdState::Unknown
                                            })
                                    })
                                    .map(|(id, _)| id.clone());
                                if let Some(evict) = evict {
                                    saved.commands.remove(&evict);
                                }
                            }
                            saved.commands.insert(
                                id.clone(),
                                outbox::Command {
                                    method: method.into(),
                                    params: safe,
                                    result: None,
                                    error: None,
                                },
                            );
                        })
                    });
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
    fn send_message(&mut self, feed: Signal<Feed>, request: AssistantSend) -> Vec<Outgoing> {
        trim_commands(feed);
        let id = request.cmd_id.clone();
        let params = value(&request);
        let existing = self
            .outbox
            .as_ref()
            .and_then(|outbox| outbox.saved.messages.get(&id));
        if existing.is_some_and(|message| message.request != request) {
            feed.update(|feed| {
                feed.problem = Some("Message identity already belongs to different text.".into())
            });
            return vec![];
        }
        if existing.is_some_and(|message| message.cancel) {
            return self.cancel_message(feed, id);
        }
        if !outbox::valid_message(&request)
            || (existing.is_none() && !self.ready_uploads(feed, &request))
        {
            feed.update(|feed| {
                feed.problem = Some(
                    "Message needs text and successfully uploaded attachments (maximum four)."
                        .into(),
                )
            });
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
        let first_acceptance = existing.is_none();
        let mut consumed_draft = None;
        let result = self
            .outbox
            .as_mut()
            .ok_or_else(|| "Pair/enrol before queuing a message on this device.".to_owned())
            .and_then(|outbox| {
                outbox.update(|saved| {
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
                })
            });
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
            self.release_upload_bytes(&request);
        }
        if send {
            vec![out(ASSISTANT_SEND, params, Some(id))]
        } else {
            vec![]
        }
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
}
