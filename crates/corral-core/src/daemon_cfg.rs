//! `[daemon]`: workspace daemon switches.
use serde::{Deserialize, Serialize};

/// `[daemon]` in `corral.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonCfg {
    /// Whether `corrald` executes owner/agent commands (`cmd.submit`); `false` refuses them,
    /// so every client runs its commands locally.
    #[serde(default = "yes")]
    pub commands: bool,
    /// How the daemon's sync passes skip work. The runtime switch files can only make it
    /// more conservative.
    #[serde(default)]
    pub pass: DaemonPass,
}

/// `[daemon] pass`, ordered from least to most conservative.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum DaemonPass {
    /// Steps and tasks run only when an input changed or came due.
    #[default]
    Incremental,
    /// Every pass is full and reports the steps an incremental pass would have wrongly skipped.
    Audit,
    /// Every pass is full; nothing is skipped.
    Full,
}

impl DaemonPass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incremental => "incremental",
            Self::Audit => "audit",
            Self::Full => "full",
        }
    }
}

fn yes() -> bool {
    true
}

impl Default for DaemonCfg {
    fn default() -> Self {
        Self {
            commands: true,
            pass: DaemonPass::Incremental,
        }
    }
}

impl DaemonCfg {
    /// Omitted from serialized configuration while it holds the defaults.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
