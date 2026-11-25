//! Typed workspace Pi hosted tools preference, without exposing the preference document.
use std::path::Path;

use anyhow::Result;
use corral_engine::pi_hosted_tools;
use corral_protocol::config::{PiHostedToolsConfig, PiHostedToolsRequest};

pub(crate) fn handle(root: &Path, request: PiHostedToolsRequest) -> Result<PiHostedToolsConfig> {
    let preference = match request.change {
        Some(change) => {
            pi_hosted_tools::save(root, &change.revision, change.web_search, change.image_gen)
        }
        None => pi_hosted_tools::read(root),
    }?;
    Ok(PiHostedToolsConfig {
        revision: preference.revision,
        web_search: preference.web_search,
        image_gen: preference.image_gen,
    })
}
