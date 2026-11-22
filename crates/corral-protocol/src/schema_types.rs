//! OpenRPC catalog references for shared serde types. These descriptors are only used
//! by the catalog, never serialized. The named `corral-types` definitions own the shape.
use corral_types::TaskDifficulty as Difficulty;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

macro_rules! shared {
    ($($(#[$attr:meta])* $name:ident: $kind:tt),+ $(,)?) => {$(
        $(#[$attr])*
        pub struct $name;

        $(#[$attr])*
        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": $kind,
                    "x-rust-type": concat!("corral_types::", stringify!($name)),
                })
            }
        }
    )+};
}

shared! {
    Artifact: "object",
    Check: "object",
    ConfigProblem: "object",
    Event: "object",
    Op: ["string", "object"],
    #[cfg(test)]
    Refusal: "object",
    Run: "object",
    SessionStatus: ["string", "object"],
    Status: "string",
    SystemAt: "object",
    SystemMap: "object",
    SystemReview: "object",
    SystemRow: "object",
    Task: "object",
    UsageBy: "string",
    UsageSummary: "object",
}

pub struct TaskDifficulty;

impl JsonSchema for TaskDifficulty {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TaskDifficulty".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "enum": Difficulty::ALL.map(Difficulty::as_str),
            "x-rust-type": "corral_types::TaskDifficulty",
        })
    }
}
