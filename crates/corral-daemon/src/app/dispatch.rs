//! The UID-checked Unix socket and authenticated Noise sessions serve the same owner API.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use corral_engine::refuse;
use corral_protocol::{
    self as wire, app, config as settings, device, op, pane,
    rpc::{self, ErrorCode},
};
use corral_types::{Refusal, SessionRef};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    config,
    conn::{self, Conn},
    devices::{DeviceError, SessionGuard},
    health, ops,
    state::Shared,
    topics::Subscriptions,
    views,
};

#[path = "pane_keys.rs"]
mod pane_keys;

pub(crate) fn welcome(shared: &Shared, hello: wire::Hello) -> wire::Welcome {
    wire::Welcome {
        proto: wire::Proto {
            major: wire::PROTO_MAJOR,
            minor: wire::PROTO_MINOR,
        },
        caps: hello
            .caps
            .into_iter()
            .filter(|cap| {
                matches!(
                    cap.as_str(),
                    wire::TASKS_V1
                        | wire::CHANGES_V1
                        | wire::PLANS_V1
                        | wire::PROJECTS_V1
                        | wire::AGENTS_V1
                        | wire::REVIEW_V1
                        | wire::TIMELINE_V1
                        | wire::USAGE_V1
                        | wire::SETTINGS_V1
                        | wire::CLOSED_V1
                        | device::DEVICE_V1
                        | device::PUSH_V1
                )
            })
            .collect(),
        daemon: wire::DaemonInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            store: shared.devices().store_name(),
            epoch: shared.epoch(),
        },
        health: shared.health(),
    }
}

pub(crate) fn serve(
    conn: &mut dyn Conn,
    shared: &Arc<Shared>,
    request: rpc::Request,
    session: Option<&SessionGuard>,
    topics: &mut Subscriptions<'_>,
) -> Result<()> {
    let rpc::Request {
        id, method, params, ..
    } = request;
    let devices = shared.devices();
    let device = if let Some(session) = session {
        match devices.authenticated(&session.device) {
            Ok(device) => Some(device),
            Err(error) => return respond::<()>(conn, id, Err(error.into())),
        }
    } else {
        None
    };
    let device_id = || {
        session
            .map(|guard| guard.device.as_str())
            .ok_or_else(|| Failure::new(ErrorCode::Unauthenticated, "device_attach_required"))
    };
    macro_rules! answer {
        ($operation:expr) => {
            respond(conn, id, (|| -> Result<_, Failure> { $operation })())
        };
    }
    match method.as_str() {
        wire::PING => conn::reply(conn, id, wire::Pong {}),
        wire::HEALTH_GET => conn::reply(conn, id, shared.health()),
        wire::NUDGE => answer!({
            let nudge: wire::Nudge = decode(params)?;
            let wait = nudge.wait_ms.map(|ms| Duration::from_millis(u64::from(ms)));
            Ok(shared.nudge(wait))
        }),
        wire::DAEMON_STOP => answer!({
            let stop: wire::Stop = decode(params)?;
            shared.stop();
            if stop.drain && !shared.wait_closed(Duration::from_secs(600)) {
                return Err(Failure::new(ErrorCode::Busy, "daemon drain timed out"));
            }
            Ok(wire::Stopped {})
        }),
        wire::SUB_OPEN => answer!(topics.open(decode(params)?).map_err(invalid)),
        wire::SUB_ACK => answer!(topics.ack(decode(params)?).map_err(invalid)),
        wire::SUB_CLOSE => answer!({
            topics.close(decode(params)?);
            Ok(())
        }),
        op::OP_SUBMIT => answer!(Ok(ops::submit(shared, decode(params)?)?)),
        settings::CONFIG_GET => answer!({
            let _: settings::ConfigGet = decode(params)?;
            Ok(config::get(shared)?)
        }),
        settings::CONFIG_PUT => answer!(Ok(config::put(shared, decode(params)?)?)),
        pane::PANE_FOCUS => answer!({
            let engine = shared.engine()?;
            let session = pane_session(&engine, decode(params)?)?;
            Ok(engine.runner.focus(&session)?)
        }),
        pane::PANE_KEYS => answer!({
            let request: pane::PaneKeys = decode(params)?;
            let engine = shared.engine()?;
            let SessionRef::Herdr { pane, .. } = pane_session(&engine, request.target)? else {
                return Err(refuse(
                    "headless process sessions have no pane to send keys to",
                    vec![],
                )
                .into());
            };
            pane_keys::send(
                &health::herdr(&engine.config.runner),
                &pane,
                None,
                &request.keys,
            )?;
            Ok(())
        }),
        app::DETAIL_GET => answer!(Ok(views::detail(shared, decode(params)?)?)),
        app::CHANGES_PAGE => answer!({
            let seen = match session {
                Some(session) => devices.seen(&session.device)?,
                None => app::SeenCursor::default(),
            };
            Ok(views::changes(
                shared,
                views::ChangesRequest {
                    query: decode(params)?,
                    seen,
                },
            )?)
        }),
        app::SEEN_ADVANCE => answer!({
            let request: app::SeenAdvance = decode(params)?;
            if request.through > shared.engine()?.store.latest_seq()? {
                return Err(Failure::new(ErrorCode::InvalidParams, "seen_ahead_of_head"));
            }
            Ok(devices.advance_seen(device_id()?, request.through)?)
        }),
        app::APP_SESSION => answer!({
            let _: app::AppSessionRequest = decode(params)?;
            let seen = devices.seen(device_id()?)?;
            let device = device.ok_or_else(|| {
                Failure::new(ErrorCode::Unauthenticated, "device_attach_required")
            })?;
            Ok(app::AppSession {
                device,
                seen,
                server_time: jiff::Timestamp::now().to_string(),
            })
        }),
        device::DEVICE_LIST => answer!({
            let _: device::DeviceList = decode(params)?;
            Ok(devices.list()?)
        }),
        device::DEVICE_PAGE => answer!({
            let request: device::DevicePageQuery = decode(params)?;
            if request.limit == 0 || request.limit > device::DEVICE_PAGE_LIMIT_MAX {
                return Err(Failure::new(
                    ErrorCode::InvalidParams,
                    "invalid_device_page_limit",
                ));
            }
            let mut rows = devices.list()?;
            rows.sort_unstable_by(|left, right| left.id.cmp(&right.id));
            rows.retain(|row| request.after.as_ref().is_none_or(|after| &row.id > after));
            let more = rows.len() > usize::from(request.limit);
            rows.truncate(usize::from(request.limit));
            let next = if more {
                rows.last().map(|row| row.id.clone())
            } else {
                None
            };
            Ok(device::DevicePage {
                devices: rows,
                next,
            })
        }),
        device::DEVICE_REVOKE => answer!({
            let request: device::DeviceRevoke = decode(params)?;
            let info = devices.revoke(&request.device)?;
            shared.publish();
            Ok(info)
        }),
        device::PAIR_BEGIN => answer!({
            let _: device::PairBegin = decode(params)?;
            Ok(devices.pair_begin(Instant::now())?)
        }),
        device::PAIR_STATUS => answer!({
            let request: device::PairStatusQuery = decode(params)?;
            let state = devices
                .pairings
                .status(
                    &request.pair_id,
                    Instant::now(),
                    Duration::from_millis(u64::from(request.wait_ms.min(25_000))),
                )
                .map_err(DeviceError::from)?;
            Ok(device::PairStatus {
                pair_id: request.pair_id,
                state,
            })
        }),
        device::PAIR_CONFIRM => answer!({
            let request: device::PairConfirm = decode(params)?;
            let device = devices.pair_confirm(&request.pair_id)?;
            shared.publish();
            Ok(device)
        }),
        device::PAIR_CANCEL => answer!({
            let request: device::PairCancel = decode(params)?;
            devices
                .pairings
                .cancel(&request.pair_id, Instant::now())
                .map_err(DeviceError::from)?;
            Ok(())
        }),
        device::PUSH_STATUS => answer!({
            let _: app::AppSessionRequest = decode(params)?;
            Ok(devices.push_status(device_id()?)?)
        }),
        device::PUSH_SUBSCRIBE => {
            answer!(Ok(devices.push_subscribe(device_id()?, decode(params)?)?))
        }
        device::PUSH_UNSUBSCRIBE => answer!({
            let _: app::AppSessionRequest = decode(params)?;
            Ok(devices.push_unsubscribe(device_id()?)?)
        }),
        _ => conn::error(conn, id, ErrorCode::MethodNotFound, "method_not_found"),
    }
}

fn pane_session(
    engine: &corral_engine::Engine,
    target: pane::PaneFocus,
) -> Result<SessionRef, Failure> {
    engine
        .store
        .get_run(&target.task, &target.run)?
        .filter(|run| run.ended.is_none())
        .and_then(|run| run.session)
        .ok_or_else(|| Failure::new(ErrorCode::NotFound, "live_session_not_found"))
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, Failure> {
    serde_json::from_value(value)
        .map_err(|error| Failure::new(ErrorCode::InvalidParams, error.to_string()))
}

fn invalid(error: anyhow::Error) -> Failure {
    Failure::new(ErrorCode::InvalidParams, format!("{error:#}"))
}

struct Failure(rpc::Error);

impl Failure {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(rpc::Error {
            code,
            message: message.into(),
            data: None,
        })
    }
}

impl From<DeviceError> for Failure {
    fn from(error: DeviceError) -> Self {
        let (code, message) = error.wire();
        Self::new(code, message)
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        let error = match error.downcast::<Refusal>() {
            Ok(refusal) => {
                return match rpc::Error::refused(refusal) {
                    Ok(error) => Self(error),
                    Err(error) => {
                        Self::new(ErrorCode::Internal, format!("encode refusal: {error}"))
                    }
                };
            }
            Err(error) => error,
        };
        let message = format!("{error:#}");
        let code = if error.is::<ops::WorkerBusy>() {
            ErrorCode::Busy
        } else if error.is::<views::NotFound>() {
            ErrorCode::NotFound
        } else if message.starts_with("invalid:") {
            ErrorCode::InvalidParams
        } else if message.starts_with("too_large:") {
            ErrorCode::TooLarge
        } else {
            ErrorCode::Internal
        };
        Self::new(code, message)
    }
}

fn respond<T: Serialize>(conn: &mut dyn Conn, id: Value, result: Result<T, Failure>) -> Result<()> {
    match result {
        Ok(result) => conn::reply(conn, id, result),
        Err(failure) => conn::rpc_error(conn, id, failure.0),
    }
}
