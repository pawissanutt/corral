//! Explicit API-first admission. Selection and credentials confer no spending authority.
use super::*;
use corral_core::{Config, paid_episode::PaidApproval};

pub(crate) fn approval(config: &Config, profile: &str, role: AgentRole) -> Result<PaidApproval> {
    let configured = config
        .profiles
        .get(profile)
        .context("PolicyNotApproved: profile missing")?;
    let metadata = configured
        .metadata
        .as_ref()
        .context("PolicyNotApproved: configure metadata.api_start")?;
    let start = metadata.api_start.as_ref().context(
        "PolicyNotApproved: configure metadata.api_start with roles, spending and approval",
    )?;
    metadata.validate(config, configured)?;
    ensure!(
        configured.connection.is_some_and(|c| c.api_key()) && start.roles.contains(&role),
        "PolicyNotApproved: metadata.api_start does not approve this role/connection"
    );
    let preferred = metadata
        .candidates
        .first()
        .context("NoApprovedCandidate: API start needs a preferred candidate")?;
    let binding = config
        .resolve_candidate(profile, preferred)?
        .connection_binding()?
        .context("NoApprovedCandidate")?;
    let approval = PaidApproval {
        policy_digest: config.profile_setup_revision()?,
        approval: start.approval.clone(),
        allowance_nanos: start
            .spending
            .maximum_micros
            .checked_mul(1000)
            .context("API start allowance overflow")?,
        candidates: vec![binding],
    };
    approval.validate()?;
    Ok(approval)
}

pub(super) fn for_profile(
    store: &Store,
    profile: &str,
    role: AgentRole,
    binding: &ConnectionBinding,
) -> Result<PaidApproval> {
    let config = store.config_layers()?.config()?;
    let approval = approval(&config, profile, role)?;
    ensure!(
        approval.candidates.as_slice() == std::slice::from_ref(binding),
        "PolicyNotApproved: API start must use this profile's exact preferred API candidate"
    );
    Ok(approval)
}

pub(super) fn approved(
    store: &Store,
    episode: &PaidEpisode,
    binding: &ConnectionBinding,
) -> Result<()> {
    let config = store.config_layers()?.config()?;
    for profile in config.profiles.keys() {
        if let Ok(approval) = approval(&config, profile, episode.root.role)
            && approval == episode.approval
            && approval.candidates.as_slice() == std::slice::from_ref(binding)
        {
            return Ok(());
        }
    }
    bail!("PolicyNotApproved: API start approval changed or route is not approved")
}

fn subject(store: &Store, container: &ContainerRef) -> Result<SubjectRevision> {
    let mut subject = SubjectRevision {
        task: container.task().map(str::to_owned),
        spec_revision: None,
        target: None,
    };
    if let Some(task) = container.task() {
        let task = store.get(task)?;
        subject.spec_revision = Some(match container {
            ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } => {
                crate::plan_spec_revision(&task)?
            }
            _ => corral_core::store::spec_revision(&task)?,
        });
        subject.target = match container {
            ContainerRef::Reviewer { run, .. } => Some(
                task.runs
                    .iter()
                    .find(|r| &r.id == run)
                    .and_then(|r| r.review.as_ref())
                    .context("SubjectChanged: review missing")?
                    .sha
                    .clone(),
            ),
            ContainerRef::PlanReviewer { plan, .. } => Some(plan.clone()),
            _ => None,
        };
    }
    subject_current(store, container, &subject)?;
    Ok(subject)
}

pub(crate) fn prepare_launch(
    ctx: &Ctx,
    container: &ContainerRef,
    binding: &ConnectionBinding,
    profile: Option<&str>,
) -> Result<()> {
    if !binding.connection.api_key() {
        return Ok(());
    }
    let generation = next_generation(&ctx.store, container)?;
    if intent_for(&ctx.store, container, generation)?.is_some() {
        return require_launch(&ctx.store, container, binding);
    }
    // Retained sessions may only recover their exact active paid route; never mint a
    // fresh approval/allowance or silently switch back to subscription on resume.
    if let Some(state) = state_for(&ctx.store, container)?
        && state.paid_episode.is_some()
    {
        ensure!(
            state
                .current_generation()
                .and_then(|g| g.connection.as_ref())
                == Some(binding),
            "paid route is sticky; start a new session to change connection/model"
        );
        regenerate(ctx, container)?
            .context("paid_admission_required: cannot restart this paid episode")?;
        return require_launch(&ctx.store, container, binding);
    }
    let role = super::super::policy::role(container);
    let approval = for_profile(
        &ctx.store,
        profile.context("paid_admission_required: API start requires selected profile provenance and metadata.api_start approval")?,
        role,
        binding,
    )?;
    ensure!(
        startup::session_may_start(ctx, container)?,
        "Held: API start session paused or stopped"
    );
    let subject = subject(&ctx.store, container)?;
    let root = super::super::policy::root_for(&ctx.store, container, &subject)?;
    ctx.store.records_transaction(|| {
        if let Some(task) = container.task()
            && let Some(previous) = task_intent(&ctx.store, task, role)?
            && previous.successor.container != *container
        {
            let episode = ctx.store.paid_episode(&previous.link.episode)?;
            ensure!(episode.root.subject != root.subject,
                "paid retry must inherit its original episode through an authorized continuation; a new attempt cannot refresh the allowance");
        }
        let successor = PaidBinding { container: container.clone(), generation, binding: binding.clone() };
        let link = match ctx.store.paid_episode_for_root(&root)? {
            Some(episode) => {
                ensure!(episode.approval == approval && matches!(&episode.phase,
                    PaidPhase::Prepared { successor: pending, carry: None } if pending == &successor),
                    "paid episode already exists, closed or exhausted; cannot refresh allowance");
                PaidLink { episode: episode.id, epoch: episode.epoch }
            }
            None => ctx.store.create_paid_episode(root.clone(), approval.clone(), successor.clone())?,
        };
        let now = corral_core::clock::now();
        let intent = ContinuationIntent {
            version: 1, role, source: successor.clone(), source_link: link.clone(), successor,
            link, context: ContextSource::ApiStart, subject: subject.clone(), reason: "explicit metered API start".into(),
            phase: IntentPhase::Prepared, pause: None, created: now, updated: now,
        };
        save(&ctx.store, &intent)?;
        event(&ctx.store, "api_start_prepared", &intent, &ctx.store.paid_episode(&intent.link.episode)?)
    })?;
    require_launch(&ctx.store, container, binding)
}
