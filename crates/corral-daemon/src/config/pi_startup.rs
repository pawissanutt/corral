//! App wire mapping for the shared workspace Pi preference.
use std::path::Path;

use anyhow::Result;
use corral_engine::pi_startup;
use corral_protocol::config::{PiStartupConfig, PiStartupRequest};

pub(crate) fn handle(root: &Path, request: PiStartupRequest) -> Result<PiStartupConfig> {
    let preference = match request.change {
        Some(change) => pi_startup::save(root, &change.revision, change.model),
        None => pi_startup::read(root),
    }?;
    Ok(PiStartupConfig {
        revision: preference.revision,
        model: preference.model,
    })
}
