//! `[daemon]`: workspace daemon switches.
use serde::{Deserialize, Serialize};

/// `[daemon]` in `corral.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonCfg {
    /// Whether `corrald` executes owner/agent commands (`cmd.submit`); `false` refuses them,
    /// so every client runs its commands locally.
    #[serde(default = "yes")]
    pub commands: bool,
}

fn yes() -> bool {
    true
}

impl Default for DaemonCfg {
    fn default() -> Self {
        Self { commands: true }
    }
}

impl DaemonCfg {
    /// Omitted from serialized configuration while it holds the defaults.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
