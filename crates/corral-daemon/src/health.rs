//! Health is daemon memory plus a probe of the configured runner.
use corral_engine::config::RunnerConfig;
use corral_protocol::{DaemonState, Health};

pub(crate) fn initial() -> Health {
    Health {
        daemon: DaemonState::Degraded,
        runner_reachable: false,
        last_sync_at: None,
        last_sync_error: None,
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

pub(crate) fn herdr(config: &RunnerConfig) -> corral_herdr::Herdr {
    corral_herdr::Herdr::new(config.herdr_session.clone().filter(|name| !name.is_empty()))
}

pub(crate) fn runner_reachable(config: &RunnerConfig) -> bool {
    match config.mode.as_str() {
        "process" => true,
        "herdr" => herdr(config).workspace_list().is_ok(),
        _ => false,
    }
}
