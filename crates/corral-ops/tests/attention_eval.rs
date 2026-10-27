//! Offline PM-attention evaluation (T-385 Design 7, S1-S21): a fake clock through `sync_at`,
//! fake herdr engines, scripted PM policies and isolated stores. No sleep, no wall-clock read
//! and no live herdr server. Kill-barrier scenarios that need cfg(test) seams run in-crate
//! (`attention::metrics::eval`); this target covers every public-path scenario.
#![cfg(unix)]
pub use corral_ops::{Ctx, resources};

#[path = "attention_eval/delivery.rs"]
mod delivery;
#[path = "attention_handling/fixture.rs"]
mod fixture;
#[path = "attention_eval/git_fixture.rs"]
mod git_fixture;
#[path = "support/mod.rs"]
mod git_support;
#[path = "attention_eval/infra.rs"]
mod infra;
#[allow(dead_code)]
#[path = "operating_launch/fixture.rs"]
mod operating_fixture;
#[path = "attention_eval/support.rs"]
mod support;
#[path = "attention_eval/wakes.rs"]
mod wakes;

#[path = "attention_eval/ladder.rs"]
mod ladder;
#[path = "attention_eval/parking.rs"]
mod parking;

/// Child-process entry for the scripted PM: `attend` refuses worker contexts, so it runs with
/// a scrubbed environment exactly as the real CLI would.
#[test]
fn attend_subprocess() {
    support::attend_child();
}
