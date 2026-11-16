//! The same enrolled-device API on Unix and Noise carriers. Caller/provenance fields
//! are never deserialized here; the command service derives them from the registry.
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use corral_protocol::{
    self as wire, app, assistant, attachment, device,
    frame::{Frame, FrameKind},
    rpc,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{conn::Conn, state::Shared};

type Failure = (rpc::ErrorCode, String);
const READ_WAIT: Duration = Duration::from_millis(wire::cmd::APP_READ_WAIT_MS);

pub(crate) fn is_method(method: &str) -> bool {
    matches!(
        method,
        app::APP_SESSION
            | app::ACT_PREVIEW
            | app::ACT_SUBMIT
            | app::ACT_RESULT
            | app::DETAIL_GET
            | app::CHANGES_PAGE
            | app::SEEN_ADVANCE
            | app::DEVICE_SET_AUTHORITY
            | device::DEVICE_AUTHORITY_PREVIEW
            | device::DEVICE_REVOKE
            | device::PAIR_BEGIN
            | device::PAIR_STATUS
            | device::PAIR_CONFIRM
            | device::PAIR_CANCEL
            | assistant::ASSISTANT_SEND
            | assistant::ASSISTANT_WARM
            | assistant::ASSISTANT_STOP
            | assistant::ASSISTANT_SESSION
            | assistant::ASSISTANT_CANCEL
            | attachment::ATTACHMENT_BEGIN
            | attachment::ATTACHMENT_PUT
            | attachment::ATTACHMENT_DISCARD
            | assistant::PROMPT_PREVIEW
            | assistant::PROMPT_ANSWER
            | device::DEVICE_LIST
            | device::DEVICE_PAGE
            | device::PUSH_STATUS
            | device::PUSH_SUBSCRIBE
            | device::PUSH_UNSUBSCRIBE
    )
}

pub(crate) fn capability(shared: &Shared, cap: &str) -> bool {
    match cap {
        wire::BOARD_V1
        | device::DEVICE_V1
        | app::INBOX_V1
        | app::CHANGES_V1
        | app::PLANS_V1
        | app::BUILDS_V1
        | app::PROJECTS_V1
        | app::AGENTS_V1
        | app::SESSION_V1
        | app::SESSION_FLEET_V1
        | wire::detail::STREAM_TERMINAL_V1
        | app::FILINGS_V1
        | app::CLOSED_V1
        | app::REVIEW_V1
        | app::TIMELINE_V1
        | app::USAGE_V1
        | app::SETTINGS_V1
        | app::ACT_V1 => true,
        device::PUSH_V1 => true,
        app::ASSISTANT_V1 | attachment::ASSISTANT_ATTACH_V1 => crate::assistant::available(shared),
        _ => false,
    }
}

pub(crate) fn allowed_topic(topic: wire::subscription::Topic) -> bool {
    use wire::subscription::Topic;
    matches!(
        topic,
        Topic::Board
            | Topic::Health
            | Topic::Inbox
            | Topic::Plans
            | Topic::Builds
            | Topic::Projects
            | Topic::Agents
            | Topic::Assistant
            | Topic::Usage
            | Topic::Settings
            | Topic::Thread
            | Topic::Terminal
            | Topic::Session
    )
}

pub(crate) fn serve(
    conn: &mut dyn Conn,
    shared: &Shared,
    id: Value,
    method: &str,
    params: Value,
    session: Option<&crate::devices::SessionGuard>,
) -> Result<()> {
    // Keep the complete response envelope bounded even for hostile JSON-RPC ids.
    if serde_json::to_vec(&id)?.len() > 256 {
        return error(
            conn,
            Value::Null,
            (rpc::ErrorCode::InvalidParams, "invalid_rpc_id".into()),
        );
    }
    match dispatch(
        shared,
        method,
        params,
        session.map(|session| session.device.as_str()),
    ) {
        Ok(mut result) => {
            if method == app::APP_SESSION
                && let Some(session) = session
            {
                result["device"]["reach"] = serde_json::to_value(session.reach())?;
            }
            reply(conn, id, result)
        }
        Err(problem) => error(conn, id, problem),
    }
}

fn dispatch(
    shared: &Shared,
    method: &str,
    params: Value,
    id: Option<&str>,
) -> Result<Value, Failure> {
    let id = id.ok_or((
        rpc::ErrorCode::Unauthenticated,
        "device_attach_required".into(),
    ))?;
    let devices = shared
        .devices()
        .ok_or((rpc::ErrorCode::Unavailable, "devices_unavailable".into()))?;
    let (device, authority) = devices
        .authenticated(id)
        .map_err(|problem| problem.wire())?;
    match method {
        app::APP_SESSION => {
            let _: app::AppSessionRequest = decode(params)?;
            value(app::AppSession {
                device,
                authority,
                seen: devices.seen(id).map_err(|problem| problem.wire())?,
                server_time: corral_core::clock::now().to_string(),
            })
        }
        device::PUSH_STATUS => {
            let _: app::AppSessionRequest = decode(params)?;
            value(devices.push_status(id).map_err(|problem| problem.wire())?)
        }
        device::PUSH_SUBSCRIBE => value(
            devices
                .push_subscribe(id, decode(params)?)
                .map_err(|problem| problem.wire())?,
        ),
        device::PUSH_UNSUBSCRIBE => {
            let _: app::AppSessionRequest = decode(params)?;
            value(
                devices
                    .push_unsubscribe(id)
                    .map_err(|problem| problem.wire())?,
            )
        }
        app::ACT_PREVIEW => value(crate::commands::app::preview(shared, id, decode(params)?)?),
        app::ACT_SUBMIT => value(crate::commands::app::submit(shared, id, decode(params)?)?),
        app::ACT_RESULT => value(crate::commands::app::result(shared, id, decode(params)?)?),
        app::DETAIL_GET => {
            let receive = crate::views::detail(shared, decode(params)?).map_err(read_failure)?;
            value(receive_read(receive)?)
        }
        app::CHANGES_PAGE => {
            let receive = crate::views::changes(
                shared,
                decode(params)?,
                devices.seen(id).map_err(|problem| problem.wire())?,
            )
            .map_err(read_failure)?;
            let page = receive_read(receive)?;
            value(page)
        }
        app::SEEN_ADVANCE => {
            let request: app::SeenAdvance = decode(params)?;
            let receive = crate::views::changes(
                shared,
                app::ChangesQuery {
                    before: None,
                    after: None,
                    limit: 1,
                },
                app::SeenCursor::default(),
            )
            .map_err(read_failure)?;
            if request.through > receive_read(receive)?.head {
                return Err((rpc::ErrorCode::InvalidParams, "seen_ahead_of_head".into()));
            }
            value(
                devices
                    .advance_seen(id, request.through)
                    .map_err(|problem| problem.wire())?,
            )
        }
        device::DEVICE_AUTHORITY_PREVIEW => value(crate::commands::app::authority_preview(
            shared,
            id,
            decode(params)?,
        )?),
        app::DEVICE_SET_AUTHORITY => {
            let result = crate::commands::app::set_authority(shared, id, decode(params)?)?;
            shared.wake_projection();
            value(result)
        }
        device::DEVICE_REVOKE
        | device::PAIR_BEGIN
        | device::PAIR_STATUS
        | device::PAIR_CONFIRM
        | device::PAIR_CANCEL => {
            if !authority.admin {
                return Err((rpc::ErrorCode::Refused, "admin_only".into()));
            }
            let via = format!("app:{id}");
            let result = match method {
                device::DEVICE_REVOKE => {
                    let request: device::DeviceRevoke = decode(params)?;
                    value(
                        devices
                            .revoke(&request.device, &via)
                            .map_err(|error| error.wire())?,
                    )
                }
                device::PAIR_BEGIN => {
                    let _: device::PairBegin = decode(params)?;
                    value(
                        devices
                            .pair_begin(std::time::Instant::now())
                            .map_err(|error| error.wire())?,
                    )
                }
                device::PAIR_STATUS => {
                    let request: device::PairStatusQuery = decode(params)?;
                    let state = devices
                        .pairings
                        .status(
                            &request.pair_id,
                            std::time::Instant::now(),
                            Duration::from_millis(
                                u64::from(request.wait_ms).min(wire::cmd::APP_READ_WAIT_MS),
                            ),
                        )
                        .map_err(|error| crate::devices::DeviceError::from(error).wire())?;
                    value(device::PairStatus {
                        pair_id: request.pair_id,
                        state,
                    })
                }
                device::PAIR_CONFIRM => {
                    let request: device::PairConfirm = decode(params)?;
                    value(
                        devices
                            .pair_confirm(&request.pair_id, &via)
                            .map_err(|error| error.wire())?,
                    )
                }
                _ => {
                    let request: device::PairCancel = decode(params)?;
                    devices
                        .pairings
                        .cancel(&request.pair_id, std::time::Instant::now())
                        .map_err(|error| crate::devices::DeviceError::from(error).wire())?;
                    value(())
                }
            };
            if result.is_ok() && matches!(method, device::DEVICE_REVOKE | device::PAIR_CONFIRM) {
                shared.wake_projection();
            }
            result
        }
        device::DEVICE_LIST => {
            let _: device::DeviceList = decode(params)?;
            value(devices.list().map_err(|problem| problem.wire())?)
        }
        device::DEVICE_PAGE => {
            let request: device::DevicePageQuery = decode(params)?;
            if request.limit == 0 || request.limit > device::DEVICE_PAGE_LIMIT_MAX {
                return Err((
                    rpc::ErrorCode::InvalidParams,
                    "invalid_device_page_limit".into(),
                ));
            }
            let mut rows = devices.list().map_err(|problem| problem.wire())?;
            rows.sort_by(|left, right| left.id.cmp(&right.id));
            rows.retain(|row| request.after.as_ref().is_none_or(|after| &row.id > after));
            let more = rows.len() > usize::from(request.limit);
            rows.truncate(usize::from(request.limit));
            let next = if more {
                rows.last().map(|row| row.id.clone())
            } else {
                None
            };
            value(device::DevicePage {
                devices: rows,
                next,
            })
        }
        assistant::PROMPT_PREVIEW => value(crate::commands::app::prompt_preview(
            shared,
            id,
            decode(params)?,
        )?),
        assistant::ASSISTANT_SEND => {
            crate::assistant::request(shared, id, crate::assistant::Request::Send(decode(params)?))
        }
        assistant::ASSISTANT_CANCEL => crate::assistant::request(
            shared,
            id,
            crate::assistant::Request::Cancel(decode(params)?),
        ),
        attachment::ATTACHMENT_BEGIN => crate::assistant::request(
            shared,
            id,
            crate::assistant::Request::AttachBegin(decode(params)?),
        ),
        attachment::ATTACHMENT_PUT => crate::assistant::request(
            shared,
            id,
            crate::assistant::Request::AttachPut(decode(params)?),
        ),
        attachment::ATTACHMENT_DISCARD => crate::assistant::request(
            shared,
            id,
            crate::assistant::Request::AttachDiscard(decode(params)?),
        ),
        assistant::ASSISTANT_WARM => {
            crate::assistant::request(shared, id, crate::assistant::Request::Warm(decode(params)?))
        }
        assistant::ASSISTANT_STOP => {
            crate::assistant::request(shared, id, crate::assistant::Request::Stop(decode(params)?))
        }
        assistant::ASSISTANT_SESSION => crate::assistant::request(
            shared,
            id,
            crate::assistant::Request::Session(decode(params)?),
        ),
        assistant::PROMPT_ANSWER => value(crate::commands::app::prompt_submit(
            shared,
            id,
            decode(params)?,
        )?),
        _ => Err((rpc::ErrorCode::MethodNotFound, "method_not_found".into())),
    }
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, Failure> {
    serde_json::from_value(value)
        .map_err(|_| (rpc::ErrorCode::InvalidParams, "invalid_params".into()))
}

fn value(result: impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(result).map_err(|_| (rpc::ErrorCode::Internal, "encode_failed".into()))
}

fn receive_read<T>(receive: mpsc::Receiver<anyhow::Result<T>>) -> Result<T, Failure> {
    receive
        .recv_timeout(READ_WAIT)
        .map_err(|_| (rpc::ErrorCode::Unavailable, "read_deadline".into()))?
        .map_err(read_failure)
}

fn read_failure(problem: anyhow::Error) -> Failure {
    let message = problem.to_string();
    let code = if message.contains("queue") || message.starts_with("busy") {
        rpc::ErrorCode::Busy
    } else if message.starts_with("not_found") || message.contains("not found") {
        rpc::ErrorCode::NotFound
    } else if message.starts_with("invalid") {
        rpc::ErrorCode::InvalidParams
    } else if message.starts_with("too_large") {
        rpc::ErrorCode::TooLarge
    } else if message.starts_with("conflict") {
        rpc::ErrorCode::Conflict
    } else {
        rpc::ErrorCode::Unavailable
    };
    (code, message.chars().take(512).collect())
}

pub(crate) fn reply(conn: &mut dyn Conn, id: Value, result: impl Serialize) -> Result<()> {
    let response = rpc::Response {
        jsonrpc: rpc::JSONRPC.into(),
        id: id.clone(),
        outcome: rpc::ResponseOutcome::Result { result },
    };
    let payload = serde_json::to_vec(&response).context("encoding app reply")?;
    if payload.len() > app::APP_CONTROL_MAX {
        return error(
            conn,
            id,
            (rpc::ErrorCode::TooLarge, "app_control_limit".into()),
        );
    }
    conn.send_frame(&Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload,
    })
}

pub(crate) fn error(conn: &mut dyn Conn, id: Value, problem: Failure) -> Result<()> {
    let (code, message) = problem;
    let data = (code == rpc::ErrorCode::Refused).then(|| json!({"code": message}));
    let response: rpc::Response<Value> = rpc::Response {
        jsonrpc: rpc::JSONRPC.into(),
        id,
        outcome: rpc::ResponseOutcome::Error {
            error: rpc::Error {
                code,
                message,
                data,
            },
        },
    };
    let payload = serde_json::to_vec(&response)?;
    anyhow::ensure!(
        payload.len() <= app::APP_CONTROL_MAX,
        "app error exceeds control limit"
    );
    conn.send_frame(&Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload,
    })
}
