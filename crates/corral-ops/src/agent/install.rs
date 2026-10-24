//! `corral agent install`, `doctor` and `state`: the managed asset installation under
//! `$XDG_DATA_HOME/corral/pi-native/assets/<digest>/` and its readback.
//! Owner: T-252 child host-install.
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::cli::{AgentCommand, InstallArgs, StateArgs, unavailable};
use crate::Ctx;

/// The T-252 child that lands the installation.
pub const OWNER: &str = "host-install";

pub fn install(_start: &Path, _args: &InstallArgs) -> Result<()> {
    Err(unavailable(AgentCommand::Install))
}

pub fn doctor(_ctx: &Ctx, _json: bool) -> Result<()> {
    Err(unavailable(AgentCommand::Doctor))
}

pub fn state(_ctx: &Ctx, _args: &StateArgs) -> Result<()> {
    Err(unavailable(AgentCommand::State))
}

/// The managed installation dispatch uses when `[agent] assets_dir` is unset. `None` until
/// host-install records one, so a pi-native dispatch refuses `assets_unavailable`.
pub fn installed_assets(_ctx: &Ctx) -> Result<Option<PathBuf>> {
    Ok(None)
}
