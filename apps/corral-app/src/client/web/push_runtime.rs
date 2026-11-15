//! Browser permission work stays outside the shared daemon coordinator.
use super::*;
use crate::model::daemon::ClientError;

pub(super) struct Intent {
    request_id: String,
    kinds: Vec<PushKind>,
}

impl Driver {
    pub(super) fn push_status(&mut self, request_id: String) {
        self.push_request(request_id, PUSH_STATUS, json!({}));
    }

    pub(super) async fn change_push(
        &mut self,
        request_id: String,
        enabled: bool,
        kinds: Vec<PushKind>,
    ) {
        if self.push_intent.is_some() {
            let outgoing =
                self.app
                    .operation(self.feed, request_id.clone(), PUSH_STATUS, json!({}));
            drop(outgoing);
            self.push_error(
                &request_id,
                "A browser push change is already in progress.".into(),
            );
            return;
        }
        if enabled {
            let outgoing =
                self.app
                    .operation(self.feed, request_id.clone(), PUSH_STATUS, json!({}));
            if outgoing.is_empty() {
                return;
            }
            // Prompt before the network round trip consumes transient user activation.
            if let Err(problem) = push::request_permission().await {
                self.push_error(&request_id, problem);
                return;
            }
            self.push_intent = Some(Intent {
                request_id: request_id.clone(),
                kinds,
            });
            if !self.flush_push(outgoing) {
                self.push_intent = None;
            }
        } else {
            self.push_request(request_id, PUSH_UNSUBSCRIBE, json!({}));
        }
    }

    fn push_request(
        &mut self,
        request_id: String,
        method: &'static str,
        params: serde_json::Value,
    ) -> bool {
        let outgoing = self.app.operation(self.feed, request_id, method, params);
        self.flush_push(outgoing)
    }

    fn flush_push(&mut self, outgoing: Vec<app::Outgoing>) -> bool {
        if outgoing.is_empty() {
            return false;
        }
        let Some(mut conn) = self.connection.take() else {
            return false;
        };
        match conn.flush_app(outgoing, self.now()) {
            Ok(()) => {
                self.connection = Some(conn);
                true
            }
            Err(problem) => {
                self.lost(problem);
                false
            }
        }
    }

    fn push_error(&mut self, request_id: &str, message: String) {
        self.feed.update(|feed| {
            if let Some(operation) = feed.operations.get_mut(request_id) {
                operation.loading = false;
                operation.error = Some(ClientError {
                    code: "browser_push".into(),
                    message,
                });
            }
        });
    }

    pub(super) async fn push_event(
        &mut self,
        conn: &mut Connection,
        event: &Event,
        now: u64,
    ) -> Result<bool, Failure> {
        let Event::Reply {
            method,
            tag,
            outcome,
            ..
        } = event
        else {
            return Ok(false);
        };
        if !matches!(*method, PUSH_STATUS | PUSH_SUBSCRIBE | PUSH_UNSUBSCRIBE) {
            return Ok(false);
        }
        let pending = tag.as_ref().is_some_and(|id| {
            self.push_intent
                .as_ref()
                .is_some_and(|intent| intent.request_id == *id)
        });
        let status: PushStatus = match outcome {
            Ok(value) => serde_json::from_value(value.clone()).map_err(|error| {
                Failure::Fatal(format!("corrald sent invalid push settings: {error}"))
            })?,
            Err(_) => {
                if pending {
                    self.push_intent = None;
                }
                return Ok(false);
            }
        };
        let vapid = status.vapid_public.clone();
        let available = status.available;
        self.feed.update(|feed| feed.push = Some(status));
        if *method == PUSH_STATUS && pending {
            let Some(intent) = self.push_intent.take() else {
                return Ok(false);
            };
            let result = async {
                if !available {
                    return Err("This daemon has no Web Push delivery configured.".into());
                }
                let vapid = vapid
                    .as_deref()
                    .ok_or("The daemon has no Web Push application key.")?;
                let subscription = push::enable(vapid).await?;
                serde_json::to_value(PushSubscribe {
                    subscription,
                    kinds: intent.kinds,
                })
                .map_err(|error| error.to_string())
            }
            .await;
            match result {
                Ok(params) => {
                    conn.core.request_tagged(
                        PUSH_SUBSCRIBE,
                        params,
                        Some(intent.request_id),
                        now,
                    )?;
                    conn.flush()?;
                }
                Err(problem) => self.push_error(&intent.request_id, problem),
            }
            return Ok(true);
        }
        if *method == PUSH_UNSUBSCRIBE {
            if let Err(problem) = push::disable().await {
                if let Some(id) = tag {
                    self.push_error(id, problem);
                }
                return Ok(true);
            }
        }
        // The shared coordinator completes the tagged operation after projection.
        Ok(false)
    }
}
