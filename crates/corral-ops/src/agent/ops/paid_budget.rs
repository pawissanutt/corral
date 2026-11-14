//! Financial adapter used only inside the ordinary authenticated native receipt commit.
//! A host cannot name an episode, allowance, connection, epoch or replacement identity.
use anyhow::{Context, Result, ensure};
use corral_core::{
    Store,
    agent::{AgentRunState, ContainerRef},
    connection::ConnectionBinding,
    paid_episode::{PaidBinding, PaidEpisode, PaidLink},
};

pub(super) struct PaidBudget {
    pub episode: PaidEpisode,
    pub owner: PaidBinding,
    pub link: PaidLink,
}
impl PaidBudget {
    pub fn load(
        store: &Store,
        container: &ContainerRef,
        state: &AgentRunState,
        binding: Option<&ConnectionBinding>,
    ) -> Result<Option<Self>> {
        let Some(binding) = binding.filter(|b| b.connection.api_key()) else {
            return Ok(None);
        };
        let link = state
            .paid_episode
            .clone()
            .context("paid_admission_required: no canonical predecessor/episode binding")?;
        let episode = store.paid_episode(&link.episode)?;
        let owner = PaidBinding {
            container: container.clone(),
            generation: state.binding.generation,
            binding: binding.clone(),
        };
        episode.owner(&link, &owner)?;
        ensure!(
            episode.approval.policy_digest
                == store.config_layers()?.config()?.profile_setup_revision()?,
            "paid policy changed; explicit approval is required"
        );
        Ok(Some(Self {
            episode,
            owner,
            link,
        }))
    }
    pub fn save(&self, store: &Store) -> Result<()> {
        store.save_paid_episode(&self.episode)
    }
}
