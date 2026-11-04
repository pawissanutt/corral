use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use corral_core::{
    Store,
    backend::{FeedError, ViewError},
};
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

    /// Evaluate one idempotent read step on a short snapshot. Only expiry retries the read;
    /// callers must keep external effects outside this closure. Explicit snapshots remain
    /// pinned and fail closed rather than silently observing another committed state.
    pub fn read_step<T>(&self, mut read: impl FnMut(&Ctx) -> Result<T>) -> Result<T> {
        loop {
            let ctx = Ctx {
                store: self.store.read_only_snapshot()?,
                ..self.clone()
            };
            let result = read(&ctx).and_then(|value| {
                ctx.store.view()?.ensure_fresh()?;
                Ok(value)
            });
            match result {
                Err(error)
                    if !self.store.is_snapshot()
                        && (matches!(
                            error.downcast_ref::<ViewError>(),
                            Some(ViewError::Expired { .. })
                        ) || matches!(
                            error.downcast_ref::<FeedError>(),
                            Some(FeedError::View(ViewError::Expired { .. }))
                        )) => {}
                result => return result,
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::backend::{BackendConfig, ManualClock, ViewError, fake::FakeBackend};
    use std::time::Duration;

    #[test]
    fn read_steps_retry_expiry_but_never_revive_explicit_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::new(jiff::Timestamp::UNIX_EPOCH));
        let ctx = Ctx::from_store(Store::init(dir.path()).unwrap().with_backend(Box::new(
            FakeBackend::new(BackendConfig::default(), clock.clone(), Arc::default()),
        )));
        let task = ctx
            .store
            .create("Read", None, corral_core::Status::Ready, "")
            .unwrap();
        let mut reads = 0;
        let title = ctx
            .read_step(|read| {
                reads += 1;
                if reads == 1 {
                    clock.advance(Duration::from_secs(31));
                }
                read.store
                    .view()?
                    .changes(corral_core::backend::ChangeSeq(0), 1)?;
                Ok(read.store.head(&task.id)?.title.clone())
            })
            .unwrap();
        assert_eq!((title.as_str(), reads), ("Read", 2));
        let frozen = Ctx {
            store: ctx.store.read_only_snapshot().unwrap(),
            ..ctx.clone()
        };
        clock.advance(Duration::from_secs(31));
        let mut reads = 0;
        let error = frozen
            .read_step(|read| {
                reads += 1;
                read.store.head(&task.id)
            })
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ViewError>(),
            Some(ViewError::Expired { .. })
        ));
        assert_eq!(reads, 1);
    }
}
