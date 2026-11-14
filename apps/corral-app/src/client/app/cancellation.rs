use super::*;
use corral_protocol::rpc::ErrorCode;

impl AppState {
    pub(super) fn cancel_message(&mut self, feed: Signal<Feed>, id: String) -> Vec<Outgoing> {
        let message = self
            .outbox
            .as_ref()
            .and_then(|outbox| outbox.saved.messages.get(&id))
            .cloned();
        let Some(message) = message else {
            feed.update(|feed| {
                feed.problem = Some("That message is no longer pending on this device.".into())
            });
            return vec![];
        };
        // Only a never-attempted send or a definitive server refusal can be removed locally.
        // Only a recorded definitive RPC refusal qualifies; old transport errors do not.
        let local = message.can_remove_locally();
        let result = self
            .outbox
            .as_mut()
            .expect("message came from the outbox")
            .update(|saved| {
                if local {
                    saved.messages.remove(&id);
                } else if let Some(message) = saved.messages.get_mut(&id) {
                    message.cancel = true;
                }
            });
        if let Err(problem) = result {
            feed.update(|feed| feed.problem = Some(problem));
            return vec![];
        }
        feed.update(|feed| {
            if let Some(state) = feed.commands.get_mut(id.as_str()) {
                state.status = if local {
                    CommandStatus::Abandoned
                } else {
                    CommandStatus::Unknown
                };
                state.error = local.then(|| ClientError {
                    code: "withdrawn".into(),
                    message: "Withdrawn — never delivered.".into(),
                });
            }
            if local {
                feed.cancelling.remove(id.as_str());
            } else {
                feed.cancelling.insert(Id::from(&id));
            }
            feed.not_withdrawn.remove(id.as_str());
        });
        if local {
            return self.discard_message_uploads(feed, &message.request);
        }
        if self.ready && self.cancel_inflight.insert(id.clone()) {
            vec![out(
                ASSISTANT_CANCEL,
                value(AssistantCancel { cmd_id: id.clone() }),
                Some(id),
            )]
        } else {
            vec![]
        }
    }
    pub(super) fn cancel_reply(
        &mut self,
        feed: Signal<Feed>,
        tag: Option<String>,
        params: Value,
        outcome: Result<Value, (ErrorCode, String)>,
        error_data: Option<Value>,
    ) -> Result<Vec<Outgoing>, String> {
        let id = tag
            .or_else(|| params["cmd_id"].as_str().map(str::to_owned))
            .ok_or("Cancel reply has no command identity")?;
        self.cancel_inflight.remove(&id);
        let Some(message) = self
            .outbox
            .as_ref()
            .and_then(|outbox| outbox.saved.messages.get(&id))
            .cloned()
            .filter(|message| message.cancel)
        else {
            return Ok(vec![]);
        };
        let ack: CancelAck = match outcome {
            Ok(result) => decode(result)?,
            Err((code, message)) => {
                let outer = value(code);
                let error = ClientError {
                    code: error_data
                        .as_ref()
                        .and_then(|data| data["code"].as_str())
                        .or_else(|| outer.as_str())
                        .unwrap_or("unknown")
                        .into(),
                    message,
                };
                feed.update(|feed| {
                    if let Some(state) = feed.commands.get_mut(id.as_str()) {
                        state.status = CommandStatus::Unknown;
                        state.error = Some(error.clone());
                    }
                    feed.problem = Some(error.message);
                });
                // Keep cancel=true durably; the next explicit retry or reconnect cancels, never resends.
                return Ok(vec![]);
            }
        };
        if !ack.cancelled && (ack.entry.is_none() || ack.state.is_none()) {
            return Err(
                "A non-cancellation reply omitted the accepted message identity/state.".into(),
            );
        }
        // Persist retirement before publishing either withdrawn or already delivered.
        if let Err(problem) = self
            .outbox
            .as_mut()
            .expect("pending cancellation has an outbox")
            .update(|saved| {
                saved.messages.remove(&id);
            })
        {
            feed.update(|feed| feed.problem = Some(problem));
            return Ok(vec![]);
        }
        feed.update(|feed| {
            feed.cancelling.remove(id.as_str());
            if let Some(state) = feed.commands.get_mut(id.as_str()) {
                state.status = if ack.cancelled {
                    CommandStatus::Abandoned
                } else {
                    CommandStatus::Done
                };
                state.error = ack.cancelled.then(|| ClientError {
                    code: "withdrawn".into(),
                    message: "Withdrawn — never delivered.".into(),
                });
                if !ack.cancelled {
                    state.entry = ack.entry.as_deref().map(Id::from);
                }
            }
            if !ack.cancelled {
                feed.not_withdrawn.insert(Id::from(&id));
                feed.problem = Some("Already delivered — not withdrawn".into());
            }
            feed.retire_echoed_sends();
        });
        Ok(if ack.cancelled {
            self.discard_message_uploads(feed, &message.request)
        } else {
            vec![]
        })
    }
    fn discard_message_uploads(
        &mut self,
        feed: Signal<Feed>,
        request: &AssistantSend,
    ) -> Vec<Outgoing> {
        let mut outgoing = vec![];
        for attachment in &request.attachments {
            if let SendAttachment::Upload { upload, .. } = attachment {
                outgoing.extend(self.discard_upload(feed, upload.clone()));
            }
        }
        outgoing
    }
}
