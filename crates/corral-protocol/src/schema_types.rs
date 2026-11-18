//! OpenRPC catalog references for shared serde types. These descriptors are only used
//! by the catalog, never serialized. The named `corral-types` definitions own the shape.
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
    Event: "object",
    Op: ["string", "object"],
    #[cfg(test)]
    Refusal: "object",
    Run: "object",
    SessionStatus: ["string", "object"],
    Status: "string",
    Task: "object",
    UsageBy: "string",
    UsageSummary: "object",
}
