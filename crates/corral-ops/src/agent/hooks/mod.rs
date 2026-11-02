//! Integration seams (T-252 child integration-seams): the only places existing corral code
//! consults the corral-pi host. Every seam first asks `run_is_corral_pi` or `is_corral_pi`, a
//! pure read of the run's harness or binding, so runs of every other harness never reach a
//! hook. Each submodule names the child that fills its body; later children edit only their
//! hook module and the agent modules behind it, never the call sites.
pub mod cleanup;
pub mod delivery;
pub mod doctor;
pub mod launch;
pub mod memory;
pub mod pm;
pub mod preflight;
pub mod review;
pub mod stall;
pub mod surfaces;
pub mod sync;
#[cfg(test)]
mod sync_head_tests;
pub mod usage;

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod metered_tests;

use corral_core::{Config, Run, Task};

pub use corral_core::{CORRAL_PI_HARNESS, is_corral_pi};

/// A worker run or review child the corral-pi host owns: launched with the corral-pi harness,
/// or carrying a host binding.
pub fn run_is_corral_pi(run: &Run) -> bool {
    native_ownership(run.harness.as_deref(), run.agent_host.is_some())
}

pub(crate) fn run_head_is_corral_pi(run: &corral_core::RunHead) -> bool {
    native_ownership(run.harness.as_deref(), run.agent_host.is_some())
}

fn native_ownership(harness: Option<&str>, bound: bool) -> bool {
    bound || harness.is_some_and(is_corral_pi)
}

/// Whether any run or review child of the task is corral-pi.
pub fn task_is_corral_pi(task: &Task) -> bool {
    task.runs
        .iter()
        .any(|run| run_is_corral_pi(run) || run.review_runs.iter().any(run_is_corral_pi))
}

/// Whether the configuration names the corral-pi harness at all (doctor's gate).
pub fn configured(config: &Config) -> bool {
    config.harnesses.keys().any(|key| is_corral_pi(key))
        || config.profiles.values().any(|p| is_corral_pi(&p.harness))
}

#[cfg(test)]
thread_local! {
    static CONSULTED: std::cell::RefCell<Vec<&'static str>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

/// Records that a seam reached its hook; tests assert other harnesses never do.
#[cfg_attr(not(test), inline(always))]
pub(crate) fn consulted(_seam: &'static str) {
    #[cfg(test)]
    CONSULTED.with(|seen| seen.borrow_mut().push(_seam));
}

#[cfg(test)]
thread_local! {
    static VISITED: std::cell::RefCell<Vec<(&'static str, String)>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

/// Records that a sweep did filesystem work for one container; tests assert settled history
/// is never visited again.
#[cfg_attr(not(test), inline(always))]
pub(crate) fn visited(_sweep: &'static str, _container: String) {
    #[cfg(test)]
    VISITED.with(|seen| seen.borrow_mut().push((_sweep, _container)));
}

/// The container visits recorded on this thread since the last call.
#[cfg(test)]
pub(crate) fn take_visits() -> Vec<(&'static str, String)> {
    VISITED.with(|seen| std::mem::take(&mut *seen.borrow_mut()))
}

/// The seams consulted on this thread since the last call.
#[cfg(test)]
pub(crate) fn take_consulted() -> Vec<&'static str> {
    CONSULTED.with(|seen| std::mem::take(&mut *seen.borrow_mut()))
}

#[cfg(all(test, unix))]
#[path = "sync_integration_tests.rs"]
mod sync_integration_tests;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
