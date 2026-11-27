//! Wire mapping for the workspace Pi startup preferences.
use std::path::Path;

use anyhow::Result;
use corral_engine::pi_startup;
use corral_protocol::config::{
    PiAppStartupConfig, PiAppStartupRequest, PiAppStartupSelection, PiStartupConfig,
    PiStartupRequest,
};

use crate::{assistant, state::Shared};

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

pub(crate) fn handle_app(
    shared: &Shared,
    request: PiAppStartupRequest,
) -> Result<PiAppStartupConfig> {
    use pi_startup::AppStartupChange;

    let preference = if let Some(change) = request.change {
        let selection = match change.selection {
            PiAppStartupSelection::AskProfile { profile } => {
                if let Some(name) = &profile {
                    pi_startup::check_ask_profile_name(name)?;
                    assistant::profile(&assistant::config(shared)?, name)?;
                }
                AppStartupChange::AskProfile(profile)
            }
            PiAppStartupSelection::BuildModel { model } => AppStartupChange::BuildModel(model),
        };
        let saved = pi_startup::save_app(shared.root(), &change.revision, selection)?;
        shared.publish();
        saved
    } else {
        pi_startup::read_app(shared.root())?
    };
    Ok(PiAppStartupConfig {
        revision: preference.revision,
        ask_profile: preference.ask_profile,
        build_model: preference.build_model,
        build_inherited: preference.build_inherited,
        ask_problem: preference.ask_problem.map(str::to_owned),
        build_problem: preference.build_problem.map(str::to_owned),
    })
}
