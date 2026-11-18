//! Test-only schema generator; no separately maintained wire descriptions.
use schemars::{JsonSchema, SchemaGenerator, generate::SchemaSettings};
use serde_json::{Value, json};
use std::collections::BTreeSet;

use crate::{rpc, subscription::*, *};

const REGENERATE: &str = "CORRAL_UPDATE_SCHEMA=1 cargo test -p corral-protocol schema::checked_in_schema_is_current -- --exact";

fn method<P: JsonSchema, R: JsonSchema>(
    generator: &mut SchemaGenerator,
    name: &str,
    notification: bool,
    errors: &[rpc::ErrorCode],
) -> Value {
    let schema = serde_json::to_value(generator.root_schema_for::<P>()).unwrap();
    let params = parameter_descriptors(&schema);
    let mut entry = json!({
        "name": name,
        "params": params,
        "x-wire-errors": errors.iter().map(|code| json!({"code": code, "message": code})).collect::<Vec<_>>(),
        "x-notification": notification,
        "x-params-schema": generator.subschema_for::<P>(),
    });
    if !notification {
        entry["result"] = json!({"name": "result", "schema": generator.subschema_for::<R>()});
    }
    entry
}

fn parameter_descriptors(schema: &Value) -> Vec<Value> {
    let branches = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .map_or_else(|| std::slice::from_ref(schema), Vec::as_slice);
    let names: BTreeSet<_> = branches
        .iter()
        .filter_map(|branch| branch.get("properties").and_then(Value::as_object))
        .flat_map(|properties| properties.keys())
        .collect();
    names
        .into_iter()
        .map(|name| {
            let mut alternatives = Vec::new();
            let mut required_in = Vec::new();
            for (index, branch) in branches.iter().enumerate() {
                if let Some(property) = branch
                    .get("properties")
                    .and_then(|properties| properties.get(name))
                    && !alternatives.contains(&property)
                {
                    alternatives.push(property);
                }
                if branch
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|required| required.iter().any(|field| field == name))
                {
                    required_in.push(index);
                }
            }
            let schema = if alternatives.len() == 1 {
                (*alternatives[0]).clone()
            } else {
                json!({"anyOf": alternatives})
            };
            let required = required_in.len() == branches.len();
            let mut descriptor = json!({"name": name, "required": required, "schema": schema});
            if !required && !required_in.is_empty() {
                // Indices refer to the branches in x-params-schema. The standard
                // boolean is true only when every alternative requires this field.
                descriptor["x-required-in"] = json!(required_in);
            }
            descriptor
        })
        .collect()
}

fn document() -> Value {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|settings| settings.definitions_path = "/components/schemas".into())
        .into_generator();
    let common = [
        rpc::ErrorCode::InvalidParams,
        rpc::ErrorCode::Unauthenticated,
        rpc::ErrorCode::Internal,
    ];
    use device::*;
    use rpc::ErrorCode::{
        Busy, Conflict, Expired, Internal, InvalidParams, NotFound, Refused, Revoked, Unavailable,
    };
    let mut op_submit = method::<op::OpSubmit, op::OpReply>(
        &mut generator,
        op::OP_SUBMIT,
        false,
        &[InvalidParams, Refused, Unavailable, Internal],
    );
    op_submit["x-error-data-schema"] =
        json!({"refused": generator.subschema_for::<schema_types::Refusal>()});
    let methods = vec![
        method::<Hello, Welcome>(
            &mut generator,
            HELLO,
            false,
            &[
                rpc::ErrorCode::UnsupportedVersion,
                rpc::ErrorCode::InvalidParams,
            ],
        ),
        method::<Ping, Pong>(&mut generator, PING, false, &common),
        method::<Bye, Bye>(&mut generator, BYE, false, &common),
        method::<Nudge, NudgeAck>(&mut generator, NUDGE, false, &common),
        method::<HealthGet, Health>(&mut generator, HEALTH_GET, false, &common),
        method::<Stop, Stopped>(&mut generator, DAEMON_STOP, false, &common),
        op_submit,
        method::<config::ConfigGet, config::ConfigDocument>(
            &mut generator,
            config::CONFIG_GET,
            false,
            &common,
        ),
        method::<config::ConfigDocument, ()>(&mut generator, config::CONFIG_PUT, false, &common),
        method::<pane::PaneFocus, ()>(
            &mut generator,
            pane::PANE_FOCUS,
            false,
            &[InvalidParams, NotFound, Unavailable, Internal],
        ),
        method::<pane::PaneKeys, ()>(
            &mut generator,
            pane::PANE_KEYS,
            false,
            &[InvalidParams, NotFound, Unavailable, Internal],
        ),
        method::<SubOpen, ()>(&mut generator, SUB_OPEN, false, &common),
        method::<SubDelta, ()>(&mut generator, SUB_DELTA, true, &[]),
        method::<SubReset, ()>(&mut generator, SUB_RESET, true, &[]),
        method::<SubAck, ()>(&mut generator, SUB_ACK, false, &common),
        method::<SubClose, ()>(&mut generator, SUB_CLOSE, false, &common),
        method::<DeviceEnrol, DeviceInfo>(
            &mut generator,
            DEVICE_ENROL,
            false,
            &[InvalidParams, Conflict, Refused, Unavailable, Internal],
        ),
        method::<DeviceAttach, DeviceInfo>(
            &mut generator,
            DEVICE_ATTACH,
            false,
            &[
                InvalidParams,
                NotFound,
                Revoked,
                Conflict,
                Unavailable,
                Internal,
            ],
        ),
        method::<DeviceList, Vec<DeviceInfo>>(
            &mut generator,
            DEVICE_LIST,
            false,
            &[InvalidParams, Unavailable, Internal],
        ),
        method::<DeviceRevoke, DeviceInfo>(
            &mut generator,
            DEVICE_REVOKE,
            false,
            &[InvalidParams, NotFound, Conflict, Unavailable, Internal],
        ),
        method::<PairBegin, PairOffer>(
            &mut generator,
            PAIR_BEGIN,
            false,
            &[InvalidParams, Busy, Unavailable, Internal],
        ),
        method::<PairStatusQuery, PairStatus>(
            &mut generator,
            PAIR_STATUS,
            false,
            &[InvalidParams, NotFound, Expired, Unavailable, Internal],
        ),
        method::<PairConfirm, DeviceInfo>(
            &mut generator,
            PAIR_CONFIRM,
            false,
            &[
                InvalidParams,
                NotFound,
                Expired,
                Conflict,
                Refused,
                Unavailable,
                Internal,
            ],
        ),
        method::<PairCancel, ()>(
            &mut generator,
            PAIR_CANCEL,
            false,
            &[
                InvalidParams,
                NotFound,
                Expired,
                Conflict,
                Unavailable,
                Internal,
            ],
        ),
        method::<PairClaim, PairClaimed>(
            &mut generator,
            PAIR_CLAIM,
            false,
            &[
                InvalidParams,
                NotFound,
                Expired,
                Conflict,
                Unavailable,
                Internal,
            ],
        ),
        method::<PairDone, ()>(&mut generator, PAIR_DONE, true, &[]),
        method::<PairRefused, ()>(&mut generator, PAIR_REFUSED, true, &[]),
        method::<app::AppSessionRequest, app::AppSession>(
            &mut generator,
            app::APP_SESSION,
            false,
            &common,
        ),
        method::<detail::DetailGet, detail::DetailReply>(
            &mut generator,
            app::DETAIL_GET,
            false,
            &[
                InvalidParams,
                NotFound,
                Refused,
                Expired,
                Unavailable,
                Internal,
            ],
        ),
        method::<app::ChangesQuery, app::ChangesPage>(
            &mut generator,
            app::CHANGES_PAGE,
            false,
            &[InvalidParams, Unavailable, Internal],
        ),
        method::<app::SeenAdvance, app::SeenCursor>(
            &mut generator,
            app::SEEN_ADVANCE,
            false,
            &[InvalidParams, Unavailable, Internal],
        ),
        method::<app::AppSessionRequest, PushStatus>(&mut generator, PUSH_STATUS, false, &common),
        method::<PushSubscribe, PushStatus>(&mut generator, PUSH_SUBSCRIBE, false, &common),
        method::<app::AppSessionRequest, PushStatus>(
            &mut generator,
            PUSH_UNSUBSCRIBE,
            false,
            &common,
        ),
        method::<assistant::AssistantSend, assistant::SendAck>(
            &mut generator,
            assistant::ASSISTANT_SEND,
            false,
            &[
                InvalidParams,
                Refused,
                Conflict,
                Busy,
                Unavailable,
                Internal,
            ],
        ),
        method::<session::AssistantCancelRequest, assistant::CancelAck>(
            &mut generator,
            assistant::ASSISTANT_CANCEL,
            false,
            &[
                InvalidParams,
                Refused,
                Conflict,
                Busy,
                Unavailable,
                Internal,
            ],
        ),
        method::<attachment::AttachmentBegin, attachment::UploadAck>(
            &mut generator,
            attachment::ATTACHMENT_BEGIN,
            false,
            &[
                InvalidParams,
                Refused,
                Conflict,
                Busy,
                Unavailable,
                Internal,
            ],
        ),
        method::<attachment::AttachmentPut, attachment::UploadAck>(
            &mut generator,
            attachment::ATTACHMENT_PUT,
            false,
            &[
                InvalidParams,
                Refused,
                Conflict,
                Busy,
                Unavailable,
                Internal,
            ],
        ),
        method::<attachment::AttachmentDiscard, ()>(
            &mut generator,
            attachment::ATTACHMENT_DISCARD,
            false,
            &[InvalidParams, Refused, Unavailable, Internal],
        ),
        method::<assistant::AssistantWarm, ()>(
            &mut generator,
            assistant::ASSISTANT_WARM,
            false,
            &[InvalidParams, Refused, Busy, Unavailable, Internal],
        ),
        method::<assistant::AssistantStop, assistant::StopAck>(
            &mut generator,
            assistant::ASSISTANT_STOP,
            false,
            &[
                InvalidParams,
                NotFound,
                Refused,
                Conflict,
                Unavailable,
                Internal,
            ],
        ),
        method::<assistant::AssistantSessionOp, rows::AssistantRow>(
            &mut generator,
            assistant::ASSISTANT_SESSION,
            false,
            &[
                InvalidParams,
                NotFound,
                Refused,
                Conflict,
                Busy,
                Unavailable,
                Internal,
            ],
        ),
        method::<session::PromptAnswerRequest, session::SessionPromptAnswered>(
            &mut generator,
            assistant::PROMPT_ANSWER,
            false,
            &[
                InvalidParams,
                NotFound,
                Refused,
                Conflict,
                Expired,
                Unavailable,
                Internal,
            ],
        ),
        method::<session::SessionCancel, session::SessionCancelReply>(
            &mut generator,
            session::SESSION_CANCEL,
            false,
            &common,
        ),
        method::<session::SessionTarget, session::SessionPauseReply>(
            &mut generator,
            session::SESSION_PAUSE,
            false,
            &common,
        ),
        method::<session::SessionTarget, session::SessionResumeReply>(
            &mut generator,
            session::SESSION_RESUME,
            false,
            &common,
        ),
        method::<session::SessionTarget, ()>(
            &mut generator,
            session::SESSION_TERMINATE,
            false,
            &common,
        ),
        method::<session::SessionTarget, session::SessionRelaunched>(
            &mut generator,
            session::SESSION_RELAUNCH,
            false,
            &common,
        ),
        method::<session::SessionRewind, session::RewindReply>(
            &mut generator,
            session::SESSION_REWIND,
            false,
            &common,
        ),
        method::<DevicePageQuery, DevicePage>(
            &mut generator,
            DEVICE_PAGE,
            false,
            &[InvalidParams, Refused, Unavailable, Internal],
        ),
    ];
    // Include envelopes and framing alongside every reachable method payload.
    generator.subschema_for::<rpc::Request>();
    generator.subschema_for::<rpc::Notification>();
    generator.subschema_for::<rpc::Response>();
    generator.subschema_for::<frame::Frame>();
    generator.subschema_for::<rpc::ErrorCode>();
    json!({
        "openrpc": "1.3.2",
        "info": {"title": "Corral daemon protocol", "version": format!("{PROTO_MAJOR}.{PROTO_MINOR}")},
        "methods": methods,
        "components": {"schemas": generator.take_definitions(true)},
        "x-error-code-schema": {"$ref": "#/components/schemas/ErrorCode"},
        "x-framing": {"control_max": frame::CONTROL_MAX, "data_max": frame::DATA_MAX, "app_control_max": app::APP_CONTROL_MAX},
    })
}

// Released-schema compatibility starts when the first released schema exists.
#[test]
fn checked_in_schema_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/corral.v1.json");
    let generated = format!("{}\n", serde_json::to_string_pretty(&document()).unwrap());
    if std::env::var_os("CORRAL_UPDATE_SCHEMA").as_deref() == Some(std::ffi::OsStr::new("1")) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &generated).unwrap();
    }
    let checked_in = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        checked_in, generated,
        "wire schema is stale; regenerate with: {REGENERATE}"
    );
}

#[test]
fn sub_delta_catalog_preserves_union_parameters_and_requiredness() {
    let document = document();
    let delta = document["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|method| method["name"] == SUB_DELTA)
        .unwrap();
    let params = delta["params"].as_array().unwrap();
    let param = |name: &str| params.iter().find(|param| param["name"] == name).unwrap();
    assert_eq!(param("topic")["required"], true);
    let schema = schemars::schema_for!(SubDelta);
    let value = serde_json::to_value(schema).unwrap();
    let branches = value["oneOf"].as_array().unwrap();
    let requiring = |field: &str| -> Vec<usize> {
        branches
            .iter()
            .enumerate()
            .filter_map(|(index, branch)| {
                branch["required"]
                    .as_array()
                    .is_some_and(|fields| fields.iter().any(|value| value == field))
                    .then_some(index)
            })
            .collect()
    };
    for field in ["rows", "cursor", "epoch", "left", "more", "scope"] {
        assert_eq!(param(field)["required"], false, "{field}");
        assert_eq!(
            param(field)["x-required-in"],
            json!(requiring(field)),
            "{field}"
        );
    }
    let rows = param("rows")["schema"]["anyOf"].as_array().unwrap();
    assert!(
        rows.len() > 2,
        "app topic rows must remain distinct alternatives"
    );
}
