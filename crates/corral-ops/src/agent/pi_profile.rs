//! Named terminal profile selection. Catalogs describe readiness, never spending authority.
use anyhow::{Context, Result, ensure};
use corral_core::ResolvedProfile;
use corral_core::agent::{AgentRole, AgentRunState, ContainerRef};
use corral_core::records::RecordArea;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::agent::hooks::pm::Overrides;
use crate::agent::launch::Prepared;

pub const PROFILE_EXIT: i32 = 79;
pub const REQUEST_FILE: &str = "profile-request.json";
pub const CATALOG_FILE: &str = "profiles.json";
const PM_PROFILE: RecordArea<String> = RecordArea::new("pi-selected-profiles");

/// Retained PM identity wins over the configured default, but never an explicit selection.
pub fn pm_current(ctx: &Ctx) -> Result<String> {
    PM_PROFILE
        .load(&ctx.store, "pm")?
        .map_or_else(|| Ok(ctx.store.config_layers()?.config()?.pm.agent), Ok)
}

pub(crate) fn remember_pm(ctx: &Ctx, name: &str) -> Result<()> {
    PM_PROFILE.save(&ctx.store, "pm", &name.to_owned())
}

/// Canonical role eligibility, exact preferred API candidate, assets and credential preflight.
/// This function is read-only and safe for catalog construction.
pub fn resolve(ctx: &Ctx, role: AgentRole, name: &str) -> Result<ResolvedProfile> {
    let overrides = Overrides {
        profile: Some(name.into()),
        ..Default::default()
    };
    match role {
        AgentRole::Assistant => crate::agent::ops::assistant::profile(ctx, None, &overrides),
        AgentRole::Pm => crate::agent::hooks::pm::profile_with(ctx, &overrides),
        _ => anyhow::bail!("profile_role: terminal selection requires assistant or PM"),
    }
}

pub(crate) fn paid(state: &AgentRunState) -> bool {
    state.paid_episode.is_some()
        || state
            .current_generation()
            .and_then(|g| g.connection.as_ref())
            .is_some_and(|b| b.connection.api_key())
}

pub(crate) fn lineage(ctx: &Ctx, container: &ContainerRef, state: &AgentRunState) -> Result<bool> {
    if paid(state) {
        return Ok(true);
    }
    let subject = crate::availability::continuation::SubjectRevision {
        task: None,
        spec_revision: None,
        target: None,
    };
    let root = crate::availability::policy::root_for(&ctx.store, container, &subject)?;
    Ok(ctx.store.paid_episode_for_root(&root)?.is_some())
}

/// Paid lineages cannot change profile provenance, even to an identically configured alias.
pub(crate) fn require_sticky(state: &AgentRunState, current: &str, requested: &str) -> Result<()> {
    ensure!(
        !paid(state) || current == requested,
        "paid route is sticky; start a new session to change profiles"
    );
    Ok(())
}

pub(crate) fn require_idle(state: &AgentRunState) -> Result<()> {
    ensure!(
        !state.turns.iter().any(|t| t.ended.is_none())
            && !state.receipts.iter().any(|r| r.effects_pending()
                || r.state == corral_core::agent::ReceiptState::Uncertain
                || r.recovery_update_due())
            && !state.jobs.iter().any(|j| j.needs_reap())
            && !state.questions.iter().any(|q| q.recorded_pending()),
        "profile_busy: settle the active turn, tools and owner questions before switching profiles"
    );
    Ok(())
}

#[derive(Serialize)]
pub struct ProfileChoice {
    pub name: String,
    pub label: String,
    pub model: String,
    pub connection: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub available: bool,
    /// Changing named profiles starts a new conversation; it is never a transcript relabel.
    pub new_session: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowance_micros: Option<u64>,
    pub binding_digest: String,
    pub policy_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Read-only role-specific choices for launchers and supervised terminal hosts.
pub fn choices(
    ctx: &Ctx,
    role: AgentRole,
    current: &str,
    state: Option<&AgentRunState>,
) -> Result<Vec<ProfileChoice>> {
    let config = ctx.store.config_layers()?.config()?;
    let policy_digest = config.profile_setup_revision()?;
    let mut profiles = Vec::new();
    for (name, profile) in &config.profiles {
        if !corral_core::is_corral_pi(&profile.harness) {
            continue;
        }
        let frozen = state.filter(|state| paid(state) && name == current);
        let resolved = if let Some(state) = frozen {
            frozen_profile(ctx, role, current, state)
        } else {
            config.resolve_profile(
                &corral_core::ProfileChoice {
                    profile: Some(name.clone()),
                    ..Default::default()
                },
                None,
                None,
            )
        };
        let binding = resolved
            .as_ref()
            .ok()
            .and_then(|r| r.connection_binding().ok().flatten());
        let Some(binding) = binding.filter(|b| !b.model.is_empty()) else {
            continue;
        };
        let readiness = (|| {
            if let Some(state) = state {
                require_sticky(state, current, name)?;
            }
            if let Some(state) = frozen {
                let resolved = frozen_profile(ctx, role, current, state)?;
                crate::agent::hooks::preflight::check(ctx, role, &resolved)?;
            } else {
                resolve(ctx, role, name)?;
            }
            Ok::<_, anyhow::Error>(())
        })();
        let provider = binding.connection.provider();
        profiles.push(ProfileChoice {
            name: name.clone(),
            label: profile
                .metadata
                .as_ref()
                .map_or_else(|| name.clone(), |m| single_line(&m.display_name)),
            model: if binding.model.contains('/') {
                binding.model.clone()
            } else {
                format!("{provider}/{}", binding.model)
            },
            connection: binding.connection.id().into(),
            provider: provider.into(),
            effort: binding.effort,
            available: readiness.is_ok(),
            new_session: name != current,
            allowance_micros: profile
                .metadata
                .as_ref()
                .and_then(|m| m.api_start.as_ref())
                .filter(|_| binding.connection.api_key())
                .map(|a| a.spending.maximum_micros),
            binding_digest: binding.digest,
            policy_digest: policy_digest.clone(),
            reason: readiness.err().map(|e| single_line(&format!("{e:#}"))),
        });
    }
    Ok(profiles)
}

fn single_line(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars() {
        let ch = if ch.is_control() { ' ' } else { ch };
        if result.len() + ch.len_utf8() > 1024 {
            break;
        }
        result.push(ch);
    }
    result
}

/// Writes the terminal catalog beside its bootstrap. A catalog grants no authority.
pub fn write_catalog(
    ctx: &Ctx,
    prepared: &Prepared,
    role: AgentRole,
    current: &str,
    state: Option<&AgentRunState>,
) -> Result<()> {
    let profiles = choices(ctx, role, current, state)?;
    let dir = prepared
        .bootstrap
        .parent()
        .context("the bootstrap has no directory")?;
    std::fs::write(
        dir.join(CATALOG_FILE),
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "current": current, "profiles": profiles,
        }))?,
    )
    .context("writing terminal profile catalog")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    version: u32,
    profile: String,
    session: Option<String>,
    generation: u32,
    #[serde(default)]
    new_session: bool,
    binding_digest: String,
    policy_digest: String,
}

/// A host request is not authority: read only a bounded regular no-follow file, then match
/// both retained identity and generation before resolving any named profile.
#[cfg(unix)]
pub(crate) fn take_request(
    prepared: &Prepared,
    session: Option<&str>,
    generation: u32,
) -> Result<Request> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let path = prepared
        .bootstrap
        .parent()
        .context("the bootstrap has no directory")?
        .join(REQUEST_FILE);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(&path)
        .context("profile_request_missing: opening profile request")?;
    ensure!(
        file.metadata()
            .context("reading profile request metadata")?
            .is_file(),
        "profile_request_invalid: request must be a regular file"
    );
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .context("reading profile request")?;
    ensure!(
        bytes.len() <= 4096,
        "profile_request_invalid: request exceeds 4096 bytes"
    );
    let request: Request =
        serde_json::from_slice(&bytes).context("profile_request_invalid: malformed request")?;
    ensure!(
        request.version == 1
            && request.session.as_deref() == session
            && request.generation == generation,
        "profile_request_invalid: request does not name the stopped session and generation"
    );
    ensure!(
        !request.profile.is_empty() && request.profile.len() <= 256,
        "profile_request_invalid: invalid profile name"
    );
    std::fs::remove_file(&path).context("consuming profile request")?;
    Ok(request)
}

#[cfg(not(unix))]
pub(crate) fn take_request(_: &Prepared, _: Option<&str>, _: u32) -> Result<Request> {
    anyhow::bail!("unsupported_platform: terminal profile selection requires Unix")
}

fn frozen_profile(
    ctx: &Ctx,
    role: AgentRole,
    current: &str,
    state: &AgentRunState,
) -> Result<ResolvedProfile> {
    let binding = state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
        .context("paid profile binding is missing")?;
    let config = ctx.store.config_layers()?.config()?;
    if role == AgentRole::Pm {
        config.check_profile_eligibility(current, corral_core::ProfileRole::Pm, None, false)?;
    }
    let candidate = config
        .profiles
        .get(current)
        .and_then(|p| p.metadata.as_ref())
        .and_then(|m| {
            m.candidates
                .iter()
                .find(|c| Some(&c.id) == binding.candidate.as_ref())
        })
        .context("paid profile frozen candidate is missing")?;
    let resolved = config.resolve_candidate(current, candidate)?;
    ensure!(
        resolved.connection_binding()?.as_ref() == Some(binding),
        "paid profile frozen binding changed"
    );
    Ok(resolved)
}

fn preflight_selection(
    ctx: &Ctx,
    role: AgentRole,
    reference: &ContainerRef,
    current: &str,
    state: &AgentRunState,
    requested: &str,
) -> Result<()> {
    ensure!(
        !lineage(ctx, reference, state)? || current == requested,
        "paid route is sticky; start a new session to change profiles"
    );
    require_idle(state)?;
    if let ContainerRef::Assistant { session } = reference {
        let retained = ctx.store.get_assistant_container(session)?;
        crate::assistant::workflow::boundary::require_resume(&ctx.store, session, &retained)?;
        ensure!(
            crate::assistant::workflow::boundary::activities(&ctx.store, session)?.is_empty()
                && !crate::agent::ops::owner_prompt::list(&ctx.store, session)?
                    .iter()
                    .any(|p| p.state == "open"),
            "profile_busy: settle pending assistant activities and owner prompts before switching"
        );
    }
    if paid(state) {
        let resolved = frozen_profile(ctx, role, current, state)?;
        crate::agent::hooks::preflight::check(ctx, role, &resolved)?;
    } else {
        ensure!(
            !lineage(ctx, reference, state)?,
            "paid profile preparation is unresolved; session remains stopped"
        );
        resolve(ctx, role, requested)?;
    }
    Ok(())
}

fn require_current_choice(
    ctx: &Ctx,
    role: AgentRole,
    current: &str,
    state: &AgentRunState,
    request: &Request,
) -> Result<()> {
    let config = ctx.store.config_layers()?.config()?;
    ensure!(
        request.policy_digest == config.profile_setup_revision()?,
        "profile_choice_stale: profile policy changed; choose and confirm again"
    );
    let resolved = if paid(state) && current == request.profile {
        frozen_profile(ctx, role, current, state)?
    } else {
        resolve(ctx, role, &request.profile)?
    };
    let binding = resolved
        .connection_binding()?
        .context("profile_choice_invalid: native binding missing")?;
    ensure!(
        request.binding_digest == binding.digest,
        "profile_choice_stale: profile binding changed; choose and confirm again"
    );
    Ok(())
}

/// A preparation with financial authority is resumable, never an excuse to fall back.
#[derive(Debug)]
pub(crate) struct FinancialStartUncertain(pub String);
impl std::fmt::Display for FinancialStartUncertain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "paid preparation for {} remains stopped; resume it explicitly",
            self.0
        )
    }
}
impl std::error::Error for FinancialStartUncertain {}

pub(crate) enum AssistantSwitch {
    Resumed(Prepared),
    New {
        claim: crate::agent::ops::assistant::Claim,
        prepared: Prepared,
    },
}

/// A differing profile opens a fresh canonical session only after explicit confirmation.
/// The original transcript and profile remain intact and resumable on their own identity.
pub(crate) fn switch_assistant(
    ctx: &Ctx,
    pane: &str,
    id: &str,
    generation: u32,
    prepared: &Prepared,
    original: &Overrides,
    over_cap: bool,
) -> Result<AssistantSwitch> {
    use crate::agent::ops::assistant as ops;
    let input = ops::claim_input(ctx, id)?.context("profile_busy: assistant input is deferred")?;
    let reference = ContainerRef::Assistant { session: id.into() };
    let source = ctx.store.get_assistant(id)?;
    let state = ctx.store.get_assistant_container(id)?.state;
    let selected = (|| {
        let request = take_request(prepared, Some(id), generation)?;
        ensure!(
            state.binding.generation == generation,
            "profile_request_invalid: generation changed"
        );
        ensure!(
            request.profile == source.profile || request.new_session,
            "profile_confirmation_required: changing profiles requires a new conversation"
        );
        preflight_selection(
            ctx,
            AgentRole::Assistant,
            &reference,
            &source.profile,
            &state,
            &request.profile,
        )?;
        require_current_choice(ctx, AgentRole::Assistant, &source.profile, &state, &request)?;
        Ok::<_, anyhow::Error>(request.profile)
    })();
    let error = match selected {
        Ok(name) if name == source.profile => {
            drop(input);
            let (_, next) = ops::resume(ctx, id, pane, original)?;
            return Ok(AssistantSwitch::Resumed(next));
        }
        Ok(name) => {
            let directory = crate::assistant::directory(&ctx.store, &source)?;
            let next = Overrides {
                profile: Some(name),
                ..Default::default()
            };
            ops::suspend(ctx, id, generation, "new profile conversation")?;
            match ops::open_claimed(ctx, &directory.path, pane, &next, over_cap, None) {
                Ok((session, prepared, warning, claim)) => {
                    if let Some(warning) = warning {
                        eprintln!("corral pi: {warning}");
                    }
                    eprintln!(
                        "corral pi: {id} suspended; profile conversation {} opened; /resume returns to {id}",
                        session.id
                    );
                    return Ok(AssistantSwitch::New { claim, prepared });
                }
                Err(error) if error.downcast_ref::<FinancialStartUncertain>().is_some() => {
                    return Err(error);
                }
                Err(error) => error,
            }
        }
        Err(error) => error,
    };
    drop(input);
    let text = format!("corral: /profiles did not switch: {error:#}. Continuing {id}.");
    let (_, next) = ops::resume(ctx, id, pane, original)
        .with_context(|| format!("{text} Resume {id} explicitly."))?;
    ops::write_switch_notice(&next, &text)?;
    eprintln!("{text}");
    Ok(AssistantSwitch::Resumed(next))
}

/// The adopted PM's child boundary uses the ordinary adoption/recovery authority path.
pub(crate) fn supervise_pm(
    ctx: &Ctx,
    pane: &str,
    mut prepared: Prepared,
    mut run: impl FnMut(&Prepared) -> Result<std::process::ExitStatus>,
) -> Result<()> {
    use corral_core::agent::pm_state;
    loop {
        let pm = pm_state::load_current(&ctx.store)?.context("no terminal PM is bound")?;
        let generation = pm.state.binding.generation;
        let result = run(&prepared);
        crate::availability::continuation::failed_start(ctx, &ContainerRef::Pm, generation)?;
        let status = result?;
        if status.code() != Some(PROFILE_EXIT) {
            ensure!(
                status.success(),
                "PM host stopped ({status}); `corral pi --pm --continue` resumes it"
            );
            return Ok(());
        }
        let current = pm_current(ctx)?;
        let selected = (|| {
            let request = take_request(&prepared, None, generation)?;
            require_current_choice(ctx, AgentRole::Pm, &current, &pm.state, &request)?;
            let name = request.profile;
            let stopped = pm_state::load_current(&ctx.store)?.context("PM disappeared")?;
            ensure!(
                stopped.state.binding.generation == generation,
                "profile_request_invalid: generation changed"
            );
            preflight_selection(
                ctx,
                AgentRole::Pm,
                &ContainerRef::Pm,
                &current,
                &stopped.state,
                &name,
            )?;
            Ok::<_, anyhow::Error>(name)
        })();
        let error = match selected {
            Ok(name) => {
                match super::adopt(
                    ctx,
                    pane,
                    &Overrides {
                        continue_session: name == current,
                        profile: Some(name),
                        ..Default::default()
                    },
                ) {
                    Ok(next) => {
                        prepared = next;
                        continue;
                    }
                    Err(error) => {
                        let stopped =
                            pm_state::load_current(&ctx.store)?.context("PM disappeared")?;
                        ensure!(
                            !lineage(ctx, &ContainerRef::Pm, &stopped.state)?,
                            "PM profile switch could not start: {error:#}; paid preparation remains stopped; resume explicitly"
                        );
                        error
                    }
                }
            }
            Err(error) => error,
        };
        let text = format!("corral: /profiles did not switch: {error:#}. Continuing PM.");
        prepared = super::adopt(
            ctx,
            pane,
            &Overrides {
                profile: Some(current),
                continue_session: true,
                ..Default::default()
            },
        )
        .with_context(|| format!("{text} `corral pi --pm --continue` resumes it."))?;
        crate::agent::ops::assistant::write_switch_notice(&prepared, &text)?;
        eprintln!("{text}");
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "pi_profile_tests.rs"]
mod tests;
