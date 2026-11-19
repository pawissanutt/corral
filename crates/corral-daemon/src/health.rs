//! Daemon lifecycle observations, runner reachability, and dispatch occupancy.
use std::collections::BTreeMap;

use anyhow::Result;
use corral_engine::{Engine, config::RunnerConfig};
use corral_protocol::{
    DaemonState, Health,
    health::{SlotCount, Slots},
};

pub(crate) fn initial() -> Health {
    Health {
        daemon: DaemonState::Degraded,
        runner_reachable: false,
        last_sync_at: None,
        last_sync_error: None,
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
        slots: None,
    }
}

pub(crate) fn slots(engine: &Engine) -> Result<Slots> {
    engine.store.read_snapshot(|| {
        let global = SlotCount {
            used: engine.store.occupied_slots(None)? as u64,
            limit: engine.config.limits.running as u64,
        };
        let mut projects = BTreeMap::new();
        for (id, project) in &engine.config.projects {
            if let Some(limit) = project.running {
                projects.insert(
                    id.clone(),
                    SlotCount {
                        used: engine.store.occupied_slots(Some(id))? as u64,
                        limit: limit as u64,
                    },
                );
            }
        }
        Ok(Slots { global, projects })
    })
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
