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
        method::<SubOpen, ()>(&mut generator, SUB_OPEN, false, &common),
        method::<SubDelta, ()>(&mut generator, SUB_DELTA, true, &[]),
        method::<SubReset, ()>(&mut generator, SUB_RESET, true, &[]),
        method::<SubAck, ()>(&mut generator, SUB_ACK, false, &common),
        method::<SubClose, ()>(&mut generator, SUB_CLOSE, false, &common),
    ];
    // Include envelopes and framing alongside every reachable method payload.
    generator.subschema_for::<rpc::Request>();
    generator.subschema_for::<rpc::Notification>();
    generator.subschema_for::<rpc::Response>();
    generator.subschema_for::<frame::Frame>();
    generator.subschema_for::<rpc::ErrorCode>();
    json!({
        "openrpc": "1.3.2",
        "info": {"title": "Corral daemon protocol", "version": "1.0"},
        "methods": methods,
        "components": {"schemas": generator.take_definitions(true)},
        "x-error-code-schema": {"$ref": "#/components/schemas/ErrorCode"},
        "x-framing": {"control_max": frame::CONTROL_MAX, "data_max": frame::DATA_MAX},
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
    let names: Vec<_> = params
        .iter()
        .map(|param| param["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["cursor", "epoch", "left", "more", "rows", "topic"]);
    let topic = params
        .iter()
        .find(|param| param["name"] == "topic")
        .unwrap();
    assert_eq!(topic["required"], true);
    assert_eq!(topic["schema"]["anyOf"].as_array().unwrap().len(), 3);
    let rows = params.iter().find(|param| param["name"] == "rows").unwrap();
    assert_eq!(rows["required"], false);
    assert_eq!(rows["x-required-in"], json!([0, 1]));
    assert_eq!(rows["schema"]["anyOf"].as_array().unwrap().len(), 2);
}
