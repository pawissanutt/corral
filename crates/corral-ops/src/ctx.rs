use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use corral_core::Store;
use corral_herdr::Herdr;

/// Everything an operation needs.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub store: Store,
    pub herdr: Herdr,
    pub resource_probe: Arc<dyn crate::resources::ResourceProbe>,
}

impl Ctx {
    /// Discovers the store from `start` and builds the herdr client. Session: $CORRAL_HERDR_SESSION
    /// (non-empty) overrides `config.herdr.session`. Outside herdr, defaults to "corral".
    pub fn open(start: &Path) -> Result<Ctx> {
        let store = Store::discover(start)?;
        Ok(Ctx::from_store(store))
    }

    /// Foreign hooks and syncs must not observe config edits before ownership is checked.
    pub fn open_for_sync(start: &Path) -> Result<Ctx> {
        let store = Store::open_unobserved(&Store::find_root(start)?)?;
        Ok(Ctx::from_store(store))
    }

    pub fn from_store(store: Store) -> Ctx {
        let session = session(&store);
        Ctx {
            herdr: Herdr::new(session),
            store,
            resource_probe: Arc::new(crate::resources::SystemResourceProbe),
        }
    }

    /// Reloads config without retargeting an explicit client on unrelated edits.
    /// A changed session preference still applies, including settings edits.
    pub fn reload_config(&mut self) -> Result<bool> {
        let previous = session(&self.store);
        let changed = self.store.reload_config_if_changed()?;
        let selected = session(&self.store);
        if changed && selected != previous {
            self.herdr.set_session(selected);
        }
        Ok(changed)
    }
}

fn session(store: &Store) -> Option<String> {
    std::env::var("CORRAL_HERDR_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| store.config.herdr.session.clone())
        .or_else(|| {
            let inherited = std::env::var("HERDR_ENV").as_deref() == Ok("1")
                || ["HERDR_SOCKET_PATH", "HERDR_SESSION"]
                    .iter()
                    .any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()));
            (!inherited).then(|| "corral".into())
        })
}
