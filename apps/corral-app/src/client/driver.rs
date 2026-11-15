//! Native transport for shared app state; only device administration is native-specific.
//! Reconnect and offline waits retain the same state and accept durable chat intents.
use super::{
    app::{self, AppState, Outgoing},
    core::Event,
    credentials, map,
    requests::{self, ClientRequest},
    session::Session,
};
use crate::model::{
    daemon::Feed,
    devices::{DeviceLink, Pairing, Registry},
};
use corral_protocol::{device::*, rpc::ErrorCode};
use nsh_core::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

pub(super) fn start(feed: Signal<Feed>, socket: Result<PathBuf, String>) {
    let (sender, receiver) = std::sync::mpsc::channel();
    requests::install(sender);
    spawn(async move {
        let mut state = AppState::new();
        state.disconnected(feed);
        let mut enrol = None;
        feed.update(|feed| feed.host = hostname());
        let socket = match socket {
            Ok(socket) => socket,
            Err(problem) => {
                feed.update(|feed| feed.link_lost(problem));
                loop {
                    offline_requests(feed, &mut state, &receiver, &mut enrol);
                    tick(feed);
                    timer(Duration::from_millis(250)).await;
                }
            }
        };
        let mut failures = 0usize;
        let mut suspended = false;
        loop {
            if suspended {
                // Revoked devices stay quiet until the owner explicitly enrols again.
                while enrol.is_none() {
                    offline_requests(feed, &mut state, &receiver, &mut enrol);
                    tick(feed);
                    if enrol.is_none() {
                        timer(Duration::from_millis(250)).await;
                    }
                }
                suspended = false;
            }
            offline_requests(feed, &mut state, &receiver, &mut enrol);
            tick(feed);
            feed.update(|feed| feed.connecting(socket.to_string_lossy().into_owned()));
            let path = socket.clone();
            let enrolling = enrol.is_some();
            let connected = blocking(move || {
                let (session, welcome) = Session::connect(&path)?;
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
                    state.welcomed(feed, &welcome);
                    feed.update(|feed| {
                        feed.welcome_received(welcome.daemon.store, map::health(welcome.health))
                    });
                    let mut current = None;
                    let mut outgoing = Vec::new();
                    if !capable {
                        feed.update(|feed| feed.device = DeviceLink::Failed("corrald is too old for device enrolment; upgrade corral and restart the daemon.".into()));
                    } else {
                        match saved {
                            Ok(Some(id)) => {
                                outgoing.push((DEVICE_ATTACH, json!({"device": id}), None));
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
                        for request in enrol.take().into_iter().chain(receiver.try_iter()) {
                            dispatch_request(feed, &mut state, request, capable, &mut outgoing);
                        }
                        state.filter_outgoing(&mut outgoing);
                        let sends = std::mem::take(&mut outgoing);
                        let next = blocking(move || {
                            let result = (|| {
                                for (method, params, tag) in sends {
                                    session.request_tagged(method, params, tag)?;
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
                        if app::handles(&event) {
                            if matches!(&event, Event::Board { .. }) {
                                failures = 0;
                            }
                            match state.event(feed, event) {
                                Ok(next) => outgoing.extend(next),
                                Err(problem) => {
                                    feed.update(|feed| feed.link_lost(problem));
                                    break;
                                }
                            }
                            continue;
                        }
                        match event {
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
                                            None,
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
                            Event::Reply {
                                method, outcome, ..
                            } => match reply(feed, &store, &mut current, method, outcome).await {
                                Ok(effects) => {
                                    outgoing.extend(effects.outgoing);
                                    if effects.disconnected {
                                        state.disconnected(feed);
                                    }
                                    if effects.ready {
                                        outgoing.extend(state.ready(feed));
                                    }
                                    if effects.revoked {
                                        suspended = true;
                                        break;
                                    }
                                }
                                Err(problem) => {
                                    feed.update(|feed| feed.link_lost(problem));
                                    break;
                                }
                            },
                            Event::Health(_)
                            | Event::Board { .. }
                            | Event::Rows { .. }
                            | Event::Reset { .. }
                            | Event::TopicError { .. }
                            | Event::Live { .. } => {
                                feed.update(|feed| {
                                    feed.link_lost("unhandled daemon application event".into())
                                });
                                break;
                            }
                        }
                    }
                }
                Err(problem) => feed.update(|feed| feed.link_lost(problem)),
            }
            state.disconnected(feed);
            if !suspended {
                let delay = super::requests::reconnect_delay_ms(failures);
                failures = failures.saturating_add(1);
                for _ in 0..delay.div_ceil(250) {
                    offline_requests(feed, &mut state, &receiver, &mut enrol);
                    tick(feed);
                    timer(Duration::from_millis(250)).await;
                }
            }
        }
    });
}

fn dispatch_request(
    feed: Signal<Feed>,
    state: &mut AppState,
    input: ClientRequest,
    devices: bool,
    outgoing: &mut Vec<Outgoing>,
) {
    let device = match input {
        ClientRequest::Enrol { name } => {
            if devices {
                feed.update(|feed| feed.device = DeviceLink::Busy);
            }
            (
                DEVICE_ENROL,
                json!({"name": name, "app_version": env!("CARGO_PKG_VERSION")}),
                None,
            )
        }
        ClientRequest::Revoke { device } => (DEVICE_REVOKE, json!({"device": device}), None),
        ClientRequest::PairBegin => (PAIR_BEGIN, json!({}), None),
        ClientRequest::PairConfirm { pair_id } => (PAIR_CONFIRM, json!({"pair_id": pair_id}), None),
        ClientRequest::PairCancel { pair_id } => (PAIR_CANCEL, json!({"pair_id": pair_id}), None),
        ClientRequest::Pair { .. } | ClientRequest::Forget => return,
        request => {
            outgoing.extend(state.request(feed, request));
            return;
        }
    };
    if devices {
        outgoing.push(device);
    }
}

fn offline_requests(
    feed: Signal<Feed>,
    state: &mut AppState,
    receiver: &std::sync::mpsc::Receiver<ClientRequest>,
    enrol: &mut Option<ClientRequest>,
) {
    for request in receiver.try_iter() {
        match request {
            request @ ClientRequest::Enrol { .. } => *enrol = Some(request),
            ClientRequest::Revoke { .. }
            | ClientRequest::PairBegin
            | ClientRequest::PairConfirm { .. }
            | ClientRequest::PairCancel { .. }
            | ClientRequest::Pair { .. }
            | ClientRequest::Forget => {}
            request => {
                // AppState alone decides which offline intents are durable.
                let _ = state.request(feed, request);
            }
        }
    }
}

#[derive(Default)]
struct Effects {
    outgoing: Vec<Outgoing>,
    ready: bool,
    disconnected: bool,
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
            effects.disconnected = [DEVICE_ATTACH, DEVICE_ENROL].contains(&method);
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
                feed.update(|feed| feed.device = DeviceLink::Failed(message.clone()));
                if method == DEVICE_ATTACH {
                    return Err(format!("Device attach failed: {message}"));
                }
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
                    effects.disconnected = true;
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
            effects.ready = true;
            effects.outgoing.push(app::devices());
        }
        DEVICE_REVOKE => {
            // The listed registry no longer matches corrald until the refreshed walk completes.
            feed.update(|feed| feed.registry = Registry::Unknown);
            effects.outgoing.push(app::devices());
        }
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
                feed.update(|feed| feed.registry = Registry::Unknown);
                effects.outgoing.push(app::devices());
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
                effects.outgoing.push((
                    PAIR_STATUS,
                    json!({"pair_id": pair_id, "wait_ms": 0}),
                    None,
                ));
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
            );
            feed.date = local.date().to_string();
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
