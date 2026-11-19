//! Native and browser sessions share device administration and trusted-side pairing.
use super::super::map;
use super::*;
use crate::model::devices::Pairing;
use corral_protocol::device::{
    PAIR_BEGIN, PAIR_CANCEL, PAIR_CONFIRM, PAIR_STATUS, PairOffer, PairState, PairStatus,
};

impl AppState {
    pub(super) fn device_request(
        &mut self,
        feed: Signal<Feed>,
        request: ClientRequest,
    ) -> Result<Vec<Outgoing>, Box<ClientRequest>> {
        let (id, method, params) = match request {
            ClientRequest::DeviceRevoke {
                request_id,
                request,
            } => (request_id, DEVICE_REVOKE, value(request)),
            ClientRequest::Revoke { device } => {
                (requests::new_id(), DEVICE_REVOKE, json!({"device": device}))
            }
            ClientRequest::PairBegin => (requests::new_id(), PAIR_BEGIN, json!({})),
            ClientRequest::PairConfirm { pair_id } => (
                requests::new_id(),
                PAIR_CONFIRM,
                json!({"pair_id": pair_id}),
            ),
            ClientRequest::PairCancel { pair_id } => {
                (requests::new_id(), PAIR_CANCEL, json!({"pair_id": pair_id}))
            }
            request => return Err(Box::new(request)),
        };
        Ok(self.operation(feed, id, method, params))
    }

    pub(super) fn device_reply(
        &mut self,
        feed: Signal<Feed>,
        method: &str,
        result: Value,
    ) -> Result<Vec<Outgoing>, String> {
        match method {
            DEVICE_REVOKE => {
                feed.update(|feed| feed.registry = Registry::Unknown);
                Ok(vec![devices()])
            }
            PAIR_BEGIN => {
                let offer: PairOffer = decode(result)?;
                feed.update(|feed| {
                    feed.pairing = Pairing::Offered {
                        pair_id: offer.pair_id,
                        code: offer.code,
                        url: offer.url,
                        expires_at: map::timestamp(Some(&offer.expires_at)).unwrap_or(0),
                        reach: map::reach(offer.reach),
                    };
                });
                Ok(vec![])
            }
            PAIR_STATUS => {
                let status: PairStatus = decode(result)?;
                let paired = matches!(status.state, PairState::Paired { .. });
                feed.update(|feed| {
                    if paired {
                        feed.registry = Registry::Unknown;
                    }
                    feed.pairing = map::pairing(status, &mut feed.pairing);
                });
                Ok(if paired { vec![devices()] } else { vec![] })
            }
            PAIR_CONFIRM | PAIR_CANCEL => Ok(self.pair_poll(feed)),
            _ => Ok(vec![]),
        }
    }

    pub fn pair_poll(&self, feed: Signal<Feed>) -> Vec<Outgoing> {
        if !self.ready {
            return vec![];
        }
        let pair_id = feed.with_untracked(|feed| match &feed.pairing {
            Pairing::Offered { pair_id, .. } | Pairing::Claimed { pair_id, .. } => {
                Some(pair_id.clone())
            }
            _ => None,
        });
        pair_id.map_or_else(Vec::new, |pair_id| {
            vec![out(
                PAIR_STATUS,
                json!({"pair_id": pair_id, "wait_ms": 0}),
                None,
            )]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_device_requests_keep_operation_ids_and_pairing_reach_through_polling() {
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let feed = signal(Feed::default());
        let mut app = AppState::new();
        app.ready = true;
        let outgoing = app
            .device_request(
                feed,
                ClientRequest::DeviceRevoke {
                    request_id: "revoke-a".into(),
                    request: corral_protocol::device::DeviceRevoke {
                        device: "device-a".into(),
                    },
                },
            )
            .unwrap();
        assert_eq!(outgoing[0].0, DEVICE_REVOKE);
        assert_eq!(outgoing[0].2.as_deref(), Some("revoke-a"));
        assert!(feed.with_untracked(|feed| feed.operations["revoke-a"].loading));
        assert!(app.device_request(feed, ClientRequest::Forget).is_err());
        app.device_reply(
            feed,
            PAIR_BEGIN,
            json!({
                "pair_id":"pair-a", "code":"pair-code", "url":null,
                "expires_at":"2030-01-01T00:00:00Z",
                "reach":[{"reach":"lan", "endpoint":"wss://corral.example"}]
            }),
        )
        .unwrap();
        let polling = app.pair_poll(feed);
        assert_eq!(polling[0].0, PAIR_STATUS);
        assert_eq!(polling[0].1["pair_id"], "pair-a");
        app.device_reply(
            feed,
            PAIR_STATUS,
            json!({
                "pair_id":"pair-a", "state":{"state":"offered", "expires_at":"2030-01-01T00:00:00Z"}
            }),
        )
        .unwrap();
        assert!(feed.with_untracked(|feed| matches!(&feed.pairing,
            Pairing::Offered { reach, .. } if reach.len() == 1 && reach[0].0 == "LAN"
                && reach[0].1 == "wss://corral.example"
        )));
        let refreshed = app.device_reply(feed, DEVICE_REVOKE, json!({})).unwrap();
        assert_eq!(refreshed[0].0, DEVICE_PAGE);
        assert_eq!(feed.with_untracked(|feed| feed.registry), Registry::Unknown);
        app.ready = false;
        assert!(app.pair_poll(feed).is_empty());
    }
}
