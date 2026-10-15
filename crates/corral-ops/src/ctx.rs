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
        let session = std::env::var("CORRAL_HERDR_SESSION")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| store.config.herdr.session.clone())
            .or_else(|| {
                (std::env::var("HERDR_ENV").as_deref() != Ok("1")).then(|| "corral".into())
            });
        Ctx {
            herdr: Herdr::new(session),
            store,
        }
    }
}
