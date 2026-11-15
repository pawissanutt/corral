//! Acknowledged optimistic transactions shared by the platform stores.
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) type Records = BTreeMap<String, Value>;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[cfg(all(not(test), not(target_arch = "wasm32")))]
use super::durable_native as backend;
#[cfg(all(not(test), target_arch = "wasm32"))]
use super::durable_web as backend;

pub(super) async fn read() -> Result<(u64, Records), String> {
    #[cfg(not(test))]
    {
        backend::read().await
    }
    #[cfg(test)]
    {
        Ok(TEST_RECORDS.with(|records| records.borrow().clone()))
    }
}

/// The closure may run again after another process commits. It must derive every change
/// from the supplied records, never from a private stale projection.
pub(super) async fn transaction<R>(
    mut change: impl FnMut(&mut Records) -> Result<R, String>,
) -> Result<(R, u64), String> {
    loop {
        let (revision, before) = read().await?;
        let mut after = before.clone();
        let result = change(&mut after)?;
        if serde_json::to_vec(&after)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_BYTES
        {
            return Err("Local device storage is full (8 MiB across namespaces). Cancel or delete saved local drafts before retrying; undelivered messages were not pruned.".into());
        }
        let mut patch = BTreeMap::new();
        for (key, value) in &after {
            if before.get(key) != Some(value) {
                patch.insert(key.clone(), Some(value.clone()));
            }
        }
        for key in before.keys() {
            if !after.contains_key(key) {
                patch.insert(key.clone(), None);
            }
        }
        if patch.is_empty() {
            return Ok((result, revision));
        }
        #[cfg(not(test))]
        let committed = backend::compare_exchange(revision, patch).await?;
        #[cfg(test)]
        let committed = TEST_RECORDS.with(|records| {
            let mut records = records.borrow_mut();
            if records.0 != revision {
                return false;
            }
            records.0 += 1;
            records.1 = after;
            true
        });
        if committed {
            return Ok((
                result,
                revision
                    .checked_add(1)
                    .ok_or("Local storage revision overflow")?,
            ));
        }
    }
}

#[cfg(test)]
thread_local! { static TEST_RECORDS: std::cell::RefCell<(u64, Records)> = const { std::cell::RefCell::new((0, BTreeMap::new())) }; }

#[cfg(test)]
pub(super) fn wait<F: Future>(future: F) -> F::Output {
    struct Wake(std::thread::Thread);
    impl std::task::Wake for Wake {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(Wake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => return value,
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}
