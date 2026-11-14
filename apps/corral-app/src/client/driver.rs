use super::{
    core::Event,
    credentials, map,
    requests::{self, ClientRequest},
    session::Session,
};
use crate::model::{
    daemon::Feed,
    devices::{DeviceLink, Pairing},
};
use corral_protocol::{device::*, rpc::ErrorCode, subscription::Topic};
use nsh_core::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

type Outgoing = (&'static str, Value);

pub(super) fn start(feed: Signal<Feed>, socket: Result<PathBuf, String>) {
    let (sender, receiver) = std::sync::mpsc::channel();
    requests::install(sender);
    spawn(async move {
        feed.update(|feed| feed.host = hostname());
        let socket = match socket {
            Ok(socket) => socket,
            Err(problem) => {
                feed.update(|feed| feed.link_lost(problem));
                return;
            }
        };
        let mut failures = 0usize;
        let mut suspended = false;
        let mut enrol = None;
        loop {
            if suspended {
                // Revoked devices stay quiet until the owner explicitly enrols again.
                while enrol.is_none() {
                    while let Ok(request) = receiver.try_recv() {
                        if matches!(request, ClientRequest::Enrol { .. }) {
                            enrol = Some(request);
                        }
                    }
                    tick(feed);
                    if enrol.is_none() {
                        timer(Duration::from_millis(250)).await;
                    }
                }
                suspended = false;
            }
            tick(feed);
            feed.update(|feed| feed.connecting(socket.to_string_lossy().into_owned()));
            let path = socket.clone();
            let enrolling = enrol.is_some();
            let connected = blocking(move || {
                let (mut session, welcome) = Session::connect(&path)?;
                session.open(Topic::Health)?;
                let capable = welcome.caps.iter().any(|cap| cap == DEVICE_V1);
                let saved = if capable && !enrolling {
                    credentials::load(&welcome.daemon.store)
                } else {
                    Ok(None)
                };
                Ok::<_, String>((session, welcome, capable, saved))
            })
            .await;
            let connected = connected
                .unwrap_or_else(|error| Err(format!("daemon connection worker closed: {error}")));
            match connected {
                Ok((mut session, welcome, capable, saved)) => {
                    let store = welcome.daemon.store.clone();
                    feed.update(|feed| {
                        feed.welcome_received(welcome.daemon.store, map::health(welcome.health))
                    });
                    let mut current = None;
                    let mut outgoing = Vec::new();
                    let mut open_board = false;
                    if !capable {
                        feed.update(|feed| feed.device = DeviceLink::Failed("corrald is too old for device enrolment; upgrade corral and restart the daemon.".into()));
                    } else {
                        match saved {
                            Ok(Some(id)) => {
                                outgoing.push((DEVICE_ATTACH, json!({"device": id})));
                                current = Some(id);
                                feed.update(|feed| feed.device = DeviceLink::Busy);
                            }
                            Ok(None) => feed.update(|feed| feed.device = DeviceLink::Unenrolled),
                            Err(problem) => {
                                feed.update(|feed| feed.device = DeviceLink::Failed(problem))
                            }
                        }
                    }
                    let mut idle = 0u8;
                    loop {
                        if capable {
                            for request in enrol.take().into_iter().chain(receiver.try_iter()) {
                                if matches!(request, ClientRequest::Enrol { .. }) {
                                    feed.update(|feed| feed.device = DeviceLink::Busy);
                                }
                                if let Some(request) = encode_request(request) {
                                    outgoing.push(request);
                                }
                            }
                        } else {
                            for _ in receiver.try_iter() {}
                        }
                        let sends = std::mem::take(&mut outgoing);
                        let opens = std::mem::take(&mut open_board);
                        let next = blocking(move || {
                            let result = (|| {
                                if opens {
                                    session.open(Topic::Board)?;
                                }
                                for (method, params) in sends {
                                    session.request(method, params)?;
                                }
                                Ok::<_, String>(session.next())
                            })();
                            (session, result.unwrap_or_else(Event::Closed))
                        })
                        .await;
                        let (returned, event) = match next {
                            Ok(next) => next,
                            Err(error) => {
                                feed.update(|feed| {
                                    feed.link_lost(format!("daemon session worker closed: {error}"))
                                });
                                break;
                            }
                        };
                        session = returned;
                        tick(feed);
                        match event {
                            Event::Health(health) => {
                                feed.update(|feed| feed.health_received(map::health(health)))
                            }
                            Event::Board {
                                epoch,
                                rows,
                                left,
                                replacement,
                            } => {
                                failures = 0;
                                feed.update(|feed| {
                                    feed.board_received(
                                        epoch,
                                        rows.into_iter().map(map::task).collect(),
                                        &left,
                                        replacement,
                                    )
                                });
                            }
                            Event::Idle => {
                                idle = idle.wrapping_add(1);
                                if idle.is_multiple_of(4) {
                                    if let Some(pair_id) =
                                        feed.with_untracked(|feed| match &feed.pairing {
                                            Pairing::Offered { pair_id, .. }
                                            | Pairing::Claimed { pair_id, .. } => {
                                                Some(pair_id.clone())
                                            }
                                            _ => None,
                                        })
                                    {
                                        outgoing.push((
                                            PAIR_STATUS,
                                            json!({"pair_id": pair_id, "wait_ms": 0}),
                                        ));
                                    }
                                }
                            }
                            Event::Closed(problem) => {
                                feed.update(|feed| feed.link_lost(problem));
                                break;
                            }
                            Event::Notice { method, params } => {
                                if method == corral_protocol::BYE
                                    && params["reason"] == CLOSE_REVOKED
                                {
                                    if let Some(id) = current.clone() {
                                        feed.update(|feed| feed.revoked(id));
                                    }
                                    suspended = true;
                                    break;
                                }
                            }
                            Event::Reply { method, outcome } => {
                                match reply(feed, &store, &mut current, method, outcome).await {
                                    Ok(effects) => {
                                        outgoing.extend(effects.outgoing);
                                        open_board |= effects.open_board;
                                        if effects.revoked {
                                            suspended = true;
                                            break;
                                        }
                                    }
                                    Err(problem) => {
                                        feed.update(|feed| feed.link_lost(problem));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(problem) => feed.update(|feed| feed.link_lost(problem)),
            }
            if !suspended {
                let delay = match failures {
                    0 => 1,
                    1 => 2,
                    2 => 5,
                    _ => 10,
                };
                failures = failures.saturating_add(1);
                timer(Duration::from_secs(delay)).await;
            }
        }
    });
}

fn encode_request(input: ClientRequest) -> Option<Outgoing> {
    Some(match input {
        ClientRequest::Enrol { name } => (
            DEVICE_ENROL,
            json!({"name": name, "app_version": env!("CARGO_PKG_VERSION")}),
        ),
        ClientRequest::Revoke { device } => (DEVICE_REVOKE, json!({"device": device})),
        ClientRequest::PairBegin => (PAIR_BEGIN, json!({})),
        ClientRequest::PairConfirm { pair_id } => (PAIR_CONFIRM, json!({"pair_id": pair_id})),
        ClientRequest::PairCancel { pair_id } => (PAIR_CANCEL, json!({"pair_id": pair_id})),
        ClientRequest::Pair { .. } | ClientRequest::Forget => return None,
    })
}

#[derive(Default)]
struct Effects {
    outgoing: Vec<Outgoing>,
    open_board: bool,
    revoked: bool,
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("invalid device reply: {error}"))
}

async fn reply(
    feed: Signal<Feed>,
    store: &str,
    current: &mut Option<String>,
    method: &'static str,
    outcome: Result<Value, (ErrorCode, String)>,
) -> Result<Effects, String> {
    let mut effects = Effects::default();
    let result = match outcome {
        Ok(result) => result,
        Err((code, message)) => {
            if method == DEVICE_ATTACH && code == ErrorCode::Revoked {
                let id = current.clone().unwrap_or_default();
                feed.update(|feed| feed.revoked(id));
                effects.revoked = true;
            } else if method == DEVICE_ATTACH && code == ErrorCode::NotFound {
                let store = store.to_owned();
                let removed = blocking(move || credentials::remove(&store))
                    .await
                    .map_err(|error| error.to_string())?;
                feed.update(|feed| {
                    feed.device = match removed {
                        Ok(()) => DeviceLink::Unenrolled,
                        Err(problem) => DeviceLink::Failed(problem),
                    }
                });
                *current = None;
            } else if [DEVICE_ATTACH, DEVICE_ENROL].contains(&method) {
                feed.update(|feed| feed.device = DeviceLink::Failed(message));
            } else if [PAIR_BEGIN, PAIR_STATUS, PAIR_CONFIRM, PAIR_CANCEL].contains(&method) {
                feed.update(|feed| feed.pairing = Pairing::Ended { line: message });
            } else {
                feed.update(|feed| feed.problem = Some(message));
            }
            return Ok(effects);
        }
    };
    match method {
        DEVICE_ATTACH | DEVICE_ENROL => {
            let info: DeviceInfo = decode(result)?;
            *current = Some(info.id.clone());
            if method == DEVICE_ENROL {
                let store = store.to_owned();
                let id = info.id.clone();
                let saved = blocking(move || credentials::save(&store, &id))
                    .await
                    .map_err(|error| error.to_string())?;
                if let Err(problem) = saved {
                    feed.update(|feed| {
                        feed.device = DeviceLink::Failed(format!(
                            "Enrolled as {}, but the id could not be saved: {problem}",
                            info.id
                        ))
                    });
                    return Ok(effects);
                }
            }
            feed.update(|feed| {
                feed.device = DeviceLink::Enrolled {
                    id: info.id,
                    name: info.name,
                }
            });
            effects.open_board = true;
            effects.outgoing.push((DEVICE_LIST, json!({})));
        }
        DEVICE_LIST => {
            let rows: Vec<DeviceInfo> = decode(result)?;
            feed.update(|feed| feed.devices_received(rows.into_iter().map(map::device).collect()));
        }
        DEVICE_REVOKE => effects.outgoing.push((DEVICE_LIST, json!({}))),
        PAIR_BEGIN => {
            let offer: PairOffer = decode(result)?;
            feed.update(|feed| {
                feed.pairing = Pairing::Offered {
                    pair_id: offer.pair_id,
                    code: offer.code,
                    url: offer.url,
                    expires_at: map::timestamp(Some(&offer.expires_at)).unwrap_or(0),
                }
            });
        }
        PAIR_STATUS => {
            let status: PairStatus = decode(result)?;
            if matches!(status.state, PairState::Paired { .. }) {
                effects.outgoing.push((DEVICE_LIST, json!({})));
            }
            feed.update(|feed| {
                let pairing = map::pairing(status, &mut feed.pairing);
                feed.pairing = pairing;
            });
        }
        PAIR_CONFIRM | PAIR_CANCEL => {
            if let Some(pair_id) = feed.with_untracked(|feed| match &feed.pairing {
                Pairing::Offered { pair_id, .. } | Pairing::Claimed { pair_id, .. } => {
                    Some(pair_id.clone())
                }
                _ => None,
            }) {
                effects
                    .outgoing
                    .push((PAIR_STATUS, json!({"pair_id": pair_id, "wait_ms": 0})));
            }
        }
        _ => {}
    }
    Ok(effects)
}

fn tick(feed: Signal<Feed>) {
    let local = jiff::Zoned::now();
    let seconds = u64::try_from(local.timestamp().as_second()).unwrap_or(0);
    if feed.with_untracked(|feed| feed.now != seconds) {
        feed.update(|feed| {
            feed.tick(
                seconds,
                format!("{:02}:{:02}", local.hour(), local.minute()),
            )
        });
    }
}

#[cfg(unix)]
fn hostname() -> Option<String> {
    let system = rustix::system::uname();
    let name = system.nodename().to_str().ok()?;
    (!name.is_empty()).then(|| name.to_owned())
}
#[cfg(not(unix))]
fn hostname() -> Option<String> {
    None
}
