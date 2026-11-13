//! Which corral-pi assets an assistant session runs (T-653). A generation's contract and tool
//! descriptions are fixed when it launches, from the assets its bootstrap named; the owner-action
//! kinds are decided by the installed `corral` at every call. This readback sets the two side by
//! side, so a session that predates the installed assets says so instead of guessing.
use anyhow::Result;
use corral_core::Store;
use corral_core::agent::AgentRunState;
use serde::Serialize;

use crate::agent::bridge::assets::asset_digest;
use crate::agent::install;
use crate::assistant::owner_action::kinds::KINDS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostAssets {
    /// The session's current generation; 0 before its first.
    pub generation: u32,
    /// The asset digest that generation launched with; none when it predates the record.
    pub assets_sha256: Option<String>,
    /// The assets a generation opened now runs: `[agent] assets_dir`, else the installation.
    pub installed_sha256: Option<String>,
    /// Whether the generation runs those assets; none while either digest is unknown.
    pub current: Option<bool>,
    /// Why the installed digest is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_error: Option<String>,
    /// The owner-action kinds this `corral` accepts, whatever the generation's tool lists.
    pub owner_actions: Vec<&'static str>,
}

fn installed(store: &Store) -> Result<Option<String>> {
    match &store.config.agent.assets_dir {
        Some(dir) => asset_digest(&std::path::absolute(dir)?).map(Some),
        None => Ok(install::read_record(&install::data_root()?)?.map(|record| record.digest)),
    }
}

/// The current generation's assets beside the ones a new generation would run.
pub fn host_assets(store: &Store, state: &AgentRunState) -> HostAssets {
    let generation = state.binding.generation;
    let assets_sha256 = (state.generations.iter())
        .find(|g| g.n == generation)
        .and_then(|g| g.assets_sha256.clone());
    let (installed_sha256, installed_error) = match installed(store) {
        Ok(digest) => (digest, None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    let current = match (&assets_sha256, &installed_sha256) {
        (Some(ran), Some(now)) => Some(ran == now),
        _ => None,
    };
    HostAssets {
        generation,
        assets_sha256,
        installed_sha256,
        current,
        installed_error,
        owner_actions: KINDS.iter().map(|spec| spec.kind).collect(),
    }
}

fn short(digest: &str) -> &str {
    digest.get(..12).unwrap_or(digest)
}

impl HostAssets {
    /// One readback line for session `id`, naming the supported way to a current generation
    /// when this one may predate the installed assets.
    pub fn line(&self, id: &str) -> String {
        let ran = match &self.assets_sha256 {
            Some(digest) => format!("assets {}", short(digest)),
            None => "unrecorded assets".to_owned(),
        };
        let installed = match (&self.installed_sha256, &self.installed_error) {
            (Some(digest), _) => format!("installed {}", short(digest)),
            (None, Some(error)) => format!("installed assets unknown ({error})"),
            (None, None) => "no corral-pi installation".to_owned(),
        };
        if self.generation == 0 {
            return format!("host: no generation yet; {installed}");
        }
        let g = self.generation;
        match self.current {
            Some(true) => format!("host: g{g} runs the installed {ran}"),
            _ if self.installed_sha256.is_some() => format!(
                "host: g{g} runs {ran}, {installed}: its contract and tool list may predate this \
                 corral, whose owner actions below still apply; a new generation loads the \
                 current ones (`corral pi --resume {id}` once its host stopped, or a new \
                 `corral pi --assistant`)"
            ),
            _ => format!("host: g{g} runs {ran}; {installed}"),
        }
    }
}
