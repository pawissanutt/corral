use std::{fs, path::PathBuf};

use crate::{
    Engine, Runner, SessionRef, checkout::Checkout, runner::UncertainWriterLaunch,
    takeover::CheckoutGuard,
};

#[derive(Default)]
pub(crate) struct PendingLaunch {
    // Keep the lease for the checkout chosen inside the transaction through commit or rollback.
    _checkout_writer: Option<CheckoutGuard>,
    pub(super) checkout: Option<(Checkout, bool)>,
    pub(super) dir: Option<PathBuf>,
    pub(super) session: Option<SessionRef>,
}

impl PendingLaunch {
    pub(crate) fn reserve_checkout(&mut self, writer: CheckoutGuard) {
        self._checkout_writer = Some(writer);
    }

    pub(super) fn rollback(&mut self, engine: &Engine, mut error: anyhow::Error) -> anyhow::Error {
        let stop = self
            .session
            .take()
            .map(|session| engine.session_runner().stop(&session));
        // Keep the unregistered run directory as a durable warning if a
        // writer may have survived both this transaction and its caller.
        if error.is::<UncertainWriterLaunch>() || stop.as_ref().is_some_and(Result::is_err) {
            self.dir.take();
            self.checkout.take();
            return match stop {
                Some(Err(cleanup)) => {
                    error.context(format!("failed to close uncommitted launch: {cleanup:#}"))
                }
                _ => error,
            };
        }
        if let Some(dir) = self.dir.take() {
            if let Err(cleanup) = fs::remove_dir_all(&dir) {
                error = error.context(format!(
                    "failed to remove uncommitted run directory {}: {cleanup}",
                    dir.display()
                ));
            }
            if let Some(parent) = dir.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
        if let Some((checkout, new_branch)) = self.checkout.take() {
            if let Err(cleanup) = corral_git::worktree_remove(&checkout.repo, &checkout.path, true)
            {
                error = error.context(format!("failed to roll back task worktree: {cleanup:#}"));
            }
            if new_branch && let Err(cleanup) = checkout.delete_branch() {
                error = error.context(format!("failed to roll back task branch: {cleanup:#}"));
            }
        }
        error
    }
}
