use std::path::Path;

use anyhow::Result;
use corral_core::Store;
use corral_herdr::Herdr;

/// Everything an operation needs.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub store: Store,
    pub herdr: Herdr,
}

impl Ctx {
    /// Discovers the store from `start` and builds the herdr client. Session: $CORRAL_HERDR_SESSION
    /// (non-empty) overrides `config.herdr.session`. Outside herdr, defaults to "corral".
    pub fn open(start: &Path) -> Result<Ctx> {
        let store = Store::discover(start)?;
        Ok(Ctx::from_store(store))
    }

    pub fn from_store(store: Store) -> Ctx {
        let session = session(&store);
        Ctx {
            herdr: Herdr::new(session),
            store,
        }
    }

    /// Reloads changed config while retaining the client's explicit executable.
    /// The environment/session preference is reapplied, including settings edits.
    pub fn reload_config(&mut self) -> Result<bool> {
        let changed = self.store.reload_config_if_changed()?;
        if changed {
            self.herdr.set_session(session(&self.store));
        }
        Ok(changed)
    }
}

fn session(store: &Store) -> Option<String> {
    std::env::var("CORRAL_HERDR_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| store.config.herdr.session.clone())
        .or_else(|| (std::env::var("HERDR_ENV").as_deref() != Ok("1")).then(|| "corral".into()))
}
