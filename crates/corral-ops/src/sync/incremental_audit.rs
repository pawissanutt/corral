//! Audit mode's race check: what changed between a pass's input sample and the moment a step
//! is decided. A skipped step that acts while its own rule fires on that change raced the
//! pass's sample (the next pass's feed delta starts at the same sample, so it would run the
//! step); only an action with no such change is a wrong skip.
use std::path::PathBuf;

use corral_core::backend::ChangeSeq;
use corral_core::{Config, ConfigLayer, Store};

use super::{FEED_LIMIT, Tokens};
use crate::feed_delta::FeedDelta;
use crate::git_watch::stat_fingerprint;

type Stat = (bool, u64, i128, u64);

pub(super) struct AuditProbe {
    store: Store,
    /// The feed position the pass's tokens were classified through.
    sample: ChangeSeq,
    /// The workspace and user config layers.
    config_paths: Vec<PathBuf>,
    /// Layer stats at the last check, and whether the parsed config differed from the sample.
    config: Option<(Vec<Stat>, bool)>,
    /// Tokens of `(sample, seq]` and the config verdict they were built with.
    cached: Option<(ChangeSeq, bool, Tokens)>,
}

impl AuditProbe {
    pub(super) fn new(store: &Store, sample: ChangeSeq) -> Self {
        let mut config_paths = vec![store.config_path()];
        config_paths.extend(
            store
                .config_layers()
                .ok()
                .and_then(|layers| layers.path(ConfigLayer::User).ok()),
        );
        Self {
            store: store.clone(),
            sample,
            config_paths,
            config: None,
            cached: None,
        }
    }

    /// A `hot_passes` repeat classified the feed through `sample`.
    pub(super) fn restart(&mut self, sample: ChangeSeq) {
        self.sample = sample;
        self.cached = None;
    }

    /// Tokens of every change after the sample up to now, with the pass's other observations;
    /// `None` when the store cannot be read (the action then stays a wrong skip).
    pub(super) fn since_sample(
        &mut self,
        current: &Tokens,
        sampled: Option<&Config>,
    ) -> Option<&Tokens> {
        let config_changed = self.config_changed(sampled);
        let view = self.store.view().ok()?;
        let seq = view.seq();
        let stale = self
            .cached
            .as_ref()
            .is_none_or(|(at, config, _)| *at != seq || *config != config_changed);
        if stale {
            let feed = FeedDelta::since(view.as_ref(), self.sample, FEED_LIMIT);
            let mut tokens = Tokens::new(
                feed,
                current.git.clone(),
                current.launch_exits,
                current.config_changed || config_changed,
            );
            tokens.inventory = current.inventory;
            tokens.resources = current.resources;
            tokens.in_pass = current.in_pass;
            tokens.panes = current.panes.clone();
            self.cached = Some((seq, config_changed, tokens));
        }
        self.cached.as_ref().map(|(_, _, tokens)| tokens)
    }

    /// Whether the config layers now parse to another config than the pass sampled. Parsed
    /// once per pass and again only after a layer file's stat changes.
    fn config_changed(&mut self, sampled: Option<&Config>) -> bool {
        let stats: Vec<Stat> = self
            .config_paths
            .iter()
            .map(|path| stat_fingerprint(path))
            .collect();
        if let Some((seen, changed)) = &self.config
            && *seen == stats
        {
            return *changed;
        }
        let now = self
            .store
            .config_layers()
            .and_then(|layers| layers.config());
        // An unreadable or invalid edit leaves the effective config unchanged.
        let changed = match (now, sampled) {
            (Ok(now), Some(sampled)) => now != *sampled,
            _ => false,
        };
        self.config = Some((stats, changed));
        changed
    }
}
