use anyhow::{Context, Result};
use corral_core::agent::{
    ContainerRef,
    pm_state::{self, AgentPm},
};
use corral_core::{ResolvedProfile, StoreLock};
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::hooks::pm;

/// A caller-managed launch after recovery, independent of pane ownership and session mode.
/// `Some` returns the prepared bootstrap without typing its command into the pane.
pub(crate) struct RecoveryLaunch<'a> {
    pub pane: Option<&'a str>,
    pub profile: Option<&'a ResolvedProfile>,
    pub adopted: bool,
    pub continue_session: bool,
}

impl<'a> RecoveryLaunch<'a> {
    pub(super) fn from_adopt(
        (pane, profile, continue_session): (&'a str, &'a ResolvedProfile, bool),
    ) -> Self {
        Self {
            pane: Some(pane),
            profile: Some(profile),
            adopted: true,
            continue_session,
        }
    }
}

pub(super) fn pm_profile(
    ctx: &Ctx,
    container: &ContainerRef,
    launch: Option<&RecoveryLaunch<'_>>,
) -> Result<Option<ResolvedProfile>> {
    if !matches!(container, ContainerRef::Pm) {
        return Ok(None);
    }
    let resolved = launch
        .and_then(|launch| launch.profile)
        .map_or_else(|| pm::profile(ctx), |profile| Ok(profile.clone()))?;
    pm::project(ctx)?;
    Ok(Some(resolved))
}

#[derive(Debug)]
pub(crate) struct PmRecoveryInProgress;

impl std::fmt::Display for PmRecoveryInProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PM recovery already in progress; retry after the current recovery finishes")
    }
}

impl std::error::Error for PmRecoveryInProgress {}

/// All PM launch/recovery entries use this order: pm-recovery, pm-policy, then dispatch
/// or generation delivery locks. The first two claims never wait for another operation.
/// Read state again only after both claims, so a snapshot grants no replacement authority.
pub(crate) struct PmRecoveryLease {
    _policy: StoreLock,
    _recovery: StoreLock,
    pub current: Option<AgentPm>,
    superseded: bool,
}

impl PmRecoveryLease {
    pub(crate) fn acquire(ctx: &Ctx) -> Result<Self> {
        let observed = pm_state::load_current(&ctx.store)?.map(|pm| pm.state.binding.generation);
        Self::acquire_observed(ctx, observed)
    }

    fn acquire_observed(ctx: &Ctx, observed: Option<u32>) -> Result<Self> {
        let recovery = ctx
            .store
            .try_lock("pm-recovery")?
            .ok_or(PmRecoveryInProgress)?;
        let policy = ctx
            .store
            .try_lock("pm-policy")?
            .context("PM policy delivery is in progress; retry PM recovery")?;
        let current = pm_state::load_current(&ctx.store)?;
        let superseded = current.as_ref().map(|pm| pm.state.binding.generation) != observed;
        Ok(Self {
            _policy: policy,
            _recovery: recovery,
            current,
            superseded,
        })
    }

    /// Adoption commits a starting generation before returning its bootstrap for exec.
    /// Protect that unbound-start gap without inheriting a lease into the long-lived host.
    /// An interrupted start becomes recoverable after the supervisor's same cooldown.
    pub(crate) fn pending(&self, ctx: &Ctx) -> bool {
        self.superseded
            || self.current.as_ref().is_some_and(|pm| {
                pm.state.current_generation().is_some_and(|generation| {
                    generation.bound.is_none()
                        && generation.fenced.is_none()
                        && ctx
                            .store
                            .now()
                            .as_second()
                            .saturating_sub(generation.created.as_second())
                            < crate::pm_supervisor::BACKOFF
                })
            })
    }

    pub(crate) fn keep_current(&self, ctx: &Ctx, replace: bool) -> Result<bool> {
        Ok(self.pending(ctx)
            || (!replace
                && self
                    .current
                    .as_ref()
                    .map(|pm| crate::agent::pi::host_alive(&pm.state))
                    .transpose()?
                    .unwrap_or(false)))
    }
}

pub(super) fn resume_container(
    ctx: &Ctx,
    container: &ContainerRef,
    replace: bool,
) -> Result<Value> {
    let lease = if matches!(container, ContainerRef::Pm) {
        Some(PmRecoveryLease::acquire(ctx)?)
    } else {
        None
    };
    if let Some(lease) = &lease
        && lease.keep_current(ctx, replace)?
        && let Some(pm) = &lease.current
    {
        return Ok(json!({
            "container": container.key(), "generation": pm.state.binding.generation,
            "pane": pm.pane, "replaced": false,
            "status": if lease.pending(ctx) { "starting" } else { "already_running" },
        }));
    }
    // Keep the lease until recover has issued pane_run, not merely until it fenced.
    Ok(super::recover(ctx, container, replace, None)?.0)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::agent::bridge::fixture::{self as f, HostProc};
    use crate::agent::hooks::pm::Overrides;
    use corral_core::inbox::Recipient;

    #[test]
    fn pm_recovery_entries_share_a_lease_and_preserve_the_starting_successor() {
        let fx = crate::agent::hooks::pm::tests::fixture();
        let mut old = HostProc::spawn(true);
        let bound = fx.bind(
            &crate::agent::BindTarget::Pm {
                project: String::new(),
            },
            "pm-pane",
            &old,
        );
        f::write_session(&bound, &f::session(&[], &[]));
        old.kill();
        std::fs::write(fx.root().join("owner-pane"), "").unwrap();
        let claim = fx.ctx.store.try_lock("pm-recovery").unwrap().unwrap();

        for replace in [false, true] {
            let error = super::resume_container(&fx.ctx, &ContainerRef::Pm, replace).unwrap_err();
            assert!(error.downcast_ref::<PmRecoveryInProgress>().is_some());
            let error = crate::pm::relaunch(&fx.ctx, "host_lost", replace).unwrap_err();
            assert!(error.downcast_ref::<PmRecoveryInProgress>().is_some());
        }
        let error = crate::pm::open_pm(&fx.ctx).unwrap_err();
        assert!(error.downcast_ref::<PmRecoveryInProgress>().is_some());
        let error =
            crate::agent::pi::adopt(&fx.ctx, "owner-pane", &Overrides::default()).unwrap_err();
        assert!(error.downcast_ref::<PmRecoveryInProgress>().is_some());
        let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
        assert_eq!(pm.state.binding.generation, 1);
        assert!(pm.state.generation(1).unwrap().fenced.is_none());
        assert!(fx.calls("pane run").is_empty());
        drop(claim);

        assert_eq!(
            crate::pm::relaunch(&fx.ctx, "host_lost", false).unwrap(),
            "pm-pane"
        );
        // The launch command has returned but hello has not: even --replace must not
        // fence the successor another entry just started.
        for replace in [false, true] {
            let outcome = super::resume_container(&fx.ctx, &ContainerRef::Pm, replace).unwrap();
            assert_eq!(outcome["generation"], 2);
            assert_eq!(outcome["status"], "starting");
            assert_eq!(
                crate::pm::relaunch(&fx.ctx, "host_lost", replace).unwrap(),
                "pm-pane"
            );
        }
        assert_eq!(crate::pm::open_pm(&fx.ctx).unwrap(), "pm-pane");
        let error =
            crate::agent::pi::adopt(&fx.ctx, "owner-pane", &Overrides::default()).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::agent::pi::PiRefusal>()
                .unwrap()
                .code,
            "pm_alive"
        );
        let current = pm_state::load(&fx.ctx.store).unwrap().unwrap();
        assert_eq!(current.state.binding.generation, 2);
        assert!(current.state.generation(2).unwrap().fenced.is_none());
        assert_eq!(fx.calls("pane run").len(), 1);

        let next = HostProc::spawn(true);
        fx.hello_next(&bound, &Recipient::Pm, &next);
        let outcome = super::resume_container(&fx.ctx, &ContainerRef::Pm, false).unwrap();
        assert_eq!(outcome["generation"], 2);
        assert_eq!(outcome["status"], "already_running");
        assert_eq!(fx.calls("pane run").len(), 1);
        // Deterministically model a caller that observed generation 1 immediately
        // before the winner acquired and released the lease.
        let stale = PmRecoveryLease::acquire_observed(&fx.ctx, Some(1)).unwrap();
        assert!(stale.keep_current(&fx.ctx, true).unwrap());
        drop(stale);

        // A failed startup is protected for a bounded interval, never forever.
        let mut expired = PmRecoveryLease::acquire(&fx.ctx).unwrap();
        let generation = expired
            .current
            .as_mut()
            .unwrap()
            .state
            .generations
            .iter_mut()
            .find(|generation| generation.n == 2)
            .unwrap();
        generation.bound = None;
        generation.created = "2000-01-01T00:00:00Z".parse().unwrap();
        assert!(!expired.pending(&fx.ctx));
    }
}
