pub use corral_types::*;

/// Attribution only. Unknown or non-Unicode environment values mean owner.
pub fn actor_from_env() -> Actor {
    match std::env::var("CORRAL_ACTOR").as_deref() {
        Ok("agent") => Actor::Agent,
        Ok("reviewer") => Actor::Reviewer,
        Ok("assistant") => Actor::Assistant,
        Ok("auto") => Actor::Auto,
        Ok("extension") => Actor::Extension,
        _ => Actor::Owner,
    }
}
