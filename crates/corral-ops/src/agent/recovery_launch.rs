use anyhow::Result;
use corral_core::ResolvedProfile;
use corral_core::agent::ContainerRef;

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
