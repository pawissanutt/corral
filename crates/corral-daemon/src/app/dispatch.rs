//! The UID-checked Unix socket and authenticated Noise sessions serve the same owner API.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use corral_engine::{Runner, refuse};
use corral_protocol::{
    self as wire, app, assistant as chat, attachment, build, config as settings, device, op, pane,
    rpc::{self, ErrorCode},
    session as control, session_host as host,
};
use corral_types::{Refusal, SessionRef};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    assistant, builds, config,
    conn::{self, Conn, Peer},
    devices::{DeviceError, SessionGuard},
    health, ops,
    sessions::{self, forward},
    state::Shared,
    topics::Subscriptions,
    views,
};

#[path = "pane_keys.rs"]
mod pane_keys;

pub(crate) fn welcome(shared: &Shared, hello: wire::Hello) -> wire::Welcome {
    let chat = hello.caps.iter().any(|cap| {
        matches!(
            cap.as_str(),
            wire::ASSISTANT_V1 | attachment::ASSISTANT_ATTACH_V1
        )
    }) && assistant::configured(shared).unwrap_or(false);
    let system = hello.caps.iter().any(|cap| cap == app::SYSTEM_V1);
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
                        | app::SESSION_V1
                        | app::SESSION_FLEET_V1
                        | app::SESSION_STATE_V1
                        | app::SESSION_TRANSCRIPT_V1
                        | app::BUILD_V1
                        | app::SYSTEM_V1
                        | wire::host::HOST_V1
                        | wire::detail::STREAM_TERMINAL_V1
                        | wire::REVIEW_V1
                        | app::GATE_ATTEMPT_V1
                        | wire::TIMELINE_V1
                        | wire::USAGE_V1
                        | wire::SETTINGS_V1
                        | wire::AUTOMATION_V1
                        | settings::PI_STARTUP_V1
                        | settings::PI_HOSTED_TOOLS_V1
                        | settings::PI_SKILLS_V1
                        | settings::PI_HOSTED_TOOL_MODELS_V1
                        | wire::CLOSED_V1
                        | app::CITATIONS_V1
                        | app::ARTIFACT_DIAGRAM_V1
                        | app::ARTIFACT_DIAGRAM_COMPARE_V1
                        | device::DEVICE_V1
                        | device::PUSH_V1
                ) || system
                    && matches!(
                        cap.as_str(),
                        app::SYSTEM_RUNTIME_V1
                            | app::SYSTEM_ENTRIES_V1
                            | app::SYSTEM_LENSES_V2
                            | app::SYSTEM_DATA_V1
                    )
                    || cfg!(target_os = "linux")
                        && matches!(
                            cap.as_str(),
                            app::SESSION_FILES_V1
                                | app::SESSION_HTML_V1
                                | app::SESSION_FILE_SEARCH_V1
                        )
                    || chat
                        && matches!(
                            cap.as_str(),
                            wire::ASSISTANT_V1 | attachment::ASSISTANT_ATTACH_V1
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
    peer: &Peer,
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
        op::OP_SUBMIT | op::OP_RESULT => {
            let queued = (|| -> Result<(), Failure> {
                let waker = topics.ensure_waker()?;
                if method == op::OP_SUBMIT {
                    let submission = decode(params)?;
                    topics
                        .operations
                        .push(id.clone(), || ops::submit(shared, submission, &waker))?;
                } else {
                    let request = decode(params)?;
                    topics
                        .operations
                        .push(id.clone(), || ops::result(shared, request, &waker))?;
                }
                Ok(())
            })();
            match queued {
                Ok(()) => Ok(()), // The connection flushes the reply when ready or timed out.
                Err(error) => respond::<()>(conn, id, Err(error)),
            }
        }
        chat::ASSISTANT_WARM => answer!(Ok(assistant::warm(shared, decode(params)?)?)),
        chat::ASSISTANT_SESSION => answer!(Ok(assistant::session(shared, decode(params)?)?)),
        build::CODE_START => answer!(Ok(builds::start_code(shared, decode(params)?)?)),
        build::CODE_RESUME => answer!({
            let target: control::SessionTarget = decode(params)?;
            Ok(builds::resume_code(shared, &target.id)?)
        }),
        build::BUILD_START => answer!(Ok(builds::start(shared, decode(params)?)?)),
        build::BUILD_REMOVE => answer!(Ok(builds::remove(shared, decode(params)?)?)),
        host::SESSION_SEND => answer!(Ok(sessions::send(
            shared,
            device.as_ref().map_or("local", |device| device.id.as_str()),
            decode(params)?,
        )?)),
        attachment::ATTACHMENT_BEGIN => answer!(Ok(assistant::attach_begin(
            shared,
            device.as_ref().map_or("local", |device| device.id.as_str()),
            decode(params)?,
        )?)),
        attachment::ATTACHMENT_PUT => answer!(Ok(assistant::attach_put(
            shared,
            device.as_ref().map_or("local", |device| device.id.as_str()),
            decode(params)?,
        )?)),
        attachment::ATTACHMENT_DISCARD => answer!(Ok(assistant::attach_discard(
            shared,
            device.as_ref().map_or("local", |device| device.id.as_str()),
            decode(params)?,
        )?)),
        control::SESSION_CANCEL => answer!(Ok(sessions::cancel(shared, decode(params)?)?)),
        control::SESSION_PAUSE => answer!({
            let target: control::SessionTarget = decode(params)?;
            Ok(sessions::pause(shared, &target.id)?)
        }),
        control::SESSION_RESUME => answer!({
            let target: control::SessionTarget = decode(params)?;
            Ok(sessions::resume(shared, &target.id)?)
        }),
        control::SESSION_TERMINATE => answer!({
            let target: control::SessionTarget = decode(params)?;
            Ok(sessions::terminate(shared, &target.id)?)
        }),
        control::SESSION_RELAUNCH => answer!({
            let target: control::SessionTarget = decode(params)?;
            Ok(sessions::relaunch(shared, &target.id)?)
        }),
        control::SESSION_REWIND => answer!(Ok(sessions::rewind(shared, decode(params)?)?)),
        host::SESSION_QUEUE_WITHDRAW => {
            answer!(Ok(forward::queue_withdraw(shared, decode(params)?)?))
        }
        host::SESSION_QUEUE_MOVE => answer!(Ok(forward::queue_move(shared, decode(params)?)?)),
        host::SESSION_MODEL => answer!(Ok(forward::model(shared, decode(params)?)?)),
        host::SESSION_CARD_ANSWER => answer!(Ok(forward::card_answer(
            shared,
            device.as_ref().map_or("local", |device| device.id.as_str()),
            decode(params)?,
        )?)),
        host::SESSION_CHILD => answer!(Ok(forward::child(shared, decode(params)?)?)),
        host::SESSION_JOB => answer!(Ok(forward::job(shared, decode(params)?)?)),
        host::SESSION_ACTION => answer!(Ok(forward::action(shared, decode(params)?)?)),
        settings::CONFIG_GET => answer!({
            let _: settings::ConfigGet = decode(params)?;
            Ok(config::get(shared)?)
        }),
        settings::CONFIG_PUT => answer!(Ok(config::put(shared, decode(params)?)?)),
        settings::PI_STARTUP => {
            answer!(config::pi_startup::handle(shared.root(), decode(params)?).map_err(invalid))
        }
        settings::PI_HOSTED_TOOLS => answer!({
            // Serde's error can echo an unexpected field name; keep this API value-free.
            let request = decode(params).map_err(|_| {
                Failure::new(ErrorCode::InvalidParams, "invalid Pi hosted tools request")
            })?;
            config::pi_hosted_tools::handle(shared.root(), request).map_err(invalid)
        }),
        settings::PI_SKILLS => {
            answer!(config::pi_skills::catalog(shared.root(), decode(params)?).map_err(invalid))
        }
        settings::PI_SKILL_GET => {
            answer!(config::pi_skills::get(shared.root(), decode(params)?).map_err(invalid))
        }
        settings::PI_SKILL_PUT => {
            answer!(config::pi_skills::put(shared.root(), decode(params)?).map_err(invalid))
        }
        settings::PI_SKILL_TOGGLE => {
            answer!(config::pi_skills::toggle(shared.root(), decode(params)?).map_err(invalid))
        }
        settings::PI_HOSTED_TOOL_MODELS => match serde_json::from_value(params) {
            Ok(request) => topics
                .model_discovery
                .start(conn, id, shared.root(), request),
            Err(_) => conn::error(
                conn,
                id,
                ErrorCode::InvalidParams,
                "invalid Pi hosted tool models request",
            ),
        },
        pane::PANE_FOCUS => answer!({
            let engine = shared.engine()?;
            let session = pane_session(&engine, &decode(params)?)?;
            Ok(engine.session_runner().focus(&session)?)
        }),
        pane::PANE_KEYS => answer!({
            let request: pane::PaneKeys = decode(params)?;
            let engine = shared.engine()?;
            let session = pane_session(&engine, &request.target)?;
            let SessionRef::Herdr { pane, .. } = sessions::available_for_pane(&session) else {
                return Err(refuse(
                    "headless process sessions have no pane to send keys to",
                    vec![],
                )
                .into());
            };
            pane_keys::send(&health::herdr(&engine.config.runner), pane, &request)?;
            Ok(())
        }),
        app::DETAIL_GET => answer!({
            use wire::detail::{DetailGet, DetailRef};
            let query: DetailGet = decode(params)?;
            if matches!(
                query.target,
                DetailRef::SystemMap { .. }
                    | DetailRef::SystemRuntime { .. }
                    | DetailRef::SystemData { .. }
                    | DetailRef::SystemLenses { .. }
                    | DetailRef::SystemLens { .. }
                    | DetailRef::SystemReview { .. }
            ) && !topics.has_system()
            {
                return Err(Failure::new(ErrorCode::InvalidParams, "system.v1 required"));
            }
            if matches!(
                query.target,
                DetailRef::SystemLens { .. } | DetailRef::SystemLenses { .. }
            ) && !topics.system_features().lenses
            {
                return Err(Failure::new(
                    ErrorCode::InvalidParams,
                    "system.lenses.v2 required",
                ));
            }
            if matches!(
                query.target,
                DetailRef::ArtifactDiagram { .. } | DetailRef::ArtifactDiagramElement { .. }
            ) && !topics.has_artifact_diagram()
            {
                return Err(Failure::new(
                    ErrorCode::InvalidParams,
                    "artifact.diagram.v1 required",
                ));
            }
            if matches!(query.target, DetailRef::ArtifactDiagramCompare { .. })
                && !topics.has_artifact_diagram_compare()
            {
                return Err(Failure::new(
                    ErrorCode::InvalidParams,
                    "artifact.diagram.v1 and artifact.diagram.compare.v1 required",
                ));
            }
            if matches!(
                query.target,
                DetailRef::SessionFiles { .. }
                    | DetailRef::SessionFile { .. }
                    | DetailRef::SessionFileChunk { .. }
                    | DetailRef::SessionFileSearch { .. }
            ) && matches!(peer, Peer::Tcp(_))
            {
                device_id()?;
            }
            Ok(views::detail_for_client(
                shared,
                query,
                topics.system_features(),
            )?)
        }),
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
                local: Some(peer.is_local()),
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
    target: &pane::PaneFocus,
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
        if let Some(refused) = error.downcast_ref::<corral_engine::runner::pi::PiRefusal>() {
            return match refused.code.as_str() {
                "stale_page" => Self::new(ErrorCode::Conflict, refused.reason.as_str()),
                "board_plan_task_missing" | "plan_mode_doc_missing" => {
                    Self::new(ErrorCode::NotFound, refused.reason.as_str())
                }
                _ => Self::from(refuse(refused.to_string(), Vec::new())),
            };
        }
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
        } else if error.is::<views::StaleTranscriptCursor>()
            || error.is::<views::StaleWorktreeFile>()
        {
            ErrorCode::Conflict
        } else if message.starts_with("invalid:") {
            ErrorCode::InvalidParams
        } else if error.is::<forward::PartTooLarge>() || message.starts_with("too_large:") {
            ErrorCode::TooLarge
        } else {
            ErrorCode::Internal
        };
        Self::new(code, message)
    }
}

pub(crate) fn respond_op(
    conn: &mut dyn Conn,
    id: Value,
    result: Result<op::OpReply>,
) -> Result<()> {
    respond(conn, id, result.map_err(Into::into))
}

fn respond<T: Serialize>(conn: &mut dyn Conn, id: Value, result: Result<T, Failure>) -> Result<()> {
    match result {
        Ok(result) => conn::reply(conn, id, result),
        Err(failure) => conn::rpc_error(conn, id, failure.0),
    }
}
