use std::path::PathBuf;

use anyhow::Result;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Machine-wide admission thresholds. Byte sizes use the artifact size syntax.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourcesCfg {
    pub max_load_per_core: f64,
    pub min_mem_available: String,
    pub min_disk_free: String,
}

impl Default for ResourcesCfg {
    fn default() -> Self {
        Self {
            max_load_per_core: 1.0,
            min_mem_available: "8GB".into(),
            min_disk_free: "50GB".into(),
        }
    }
}

impl ResourcesCfg {
    pub fn min_mem_bytes(&self) -> Result<u64> {
        crate::parse_bytes(&self.min_mem_available)
    }

    pub fn min_disk_bytes(&self) -> Result<u64> {
        crate::parse_bytes(&self.min_disk_free)
    }

    pub(crate) fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.max_load_per_core.is_finite() || self.max_load_per_core <= 0.0 {
            out.push("resources.max_load_per_core must be finite and > 0".into());
        }
        for (name, result) in [
            ("min_mem_available", self.min_mem_bytes()),
            ("min_disk_free", self.min_disk_bytes()),
        ] {
            if let Err(error) = result {
                out.push(format!("resources.{name}: {error:#}"));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskSpace {
    pub path: PathBuf,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub load_per_core: f64,
    pub load_one: f64,
    pub cores: u32,
    pub mem_available: u64,
    pub cpu_psi: Option<f64>,
    pub mem_psi: Option<f64>,
    pub disks: Vec<DiskSpace>,
}

/// Persisted observation and admission latch. PSI is diagnostic only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceState {
    pub snapshot: ResourceSnapshot,
    pub reason: Option<String>,
    pub severe: bool,
    pub held: bool,
    pub observed_at: Timestamp,
}

#[cfg(test)]
#[path = "resource_tests.rs"]
mod tests;
