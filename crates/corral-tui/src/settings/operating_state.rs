//! Fresh read-only policy facts remain inspectable even while ordinary settings are dirty.
use anyhow::Result;
use corral_ops::Ctx;

pub(super) fn facts(ctx: &Ctx) -> Result<Vec<String>> {
    let workset = corral_ops::pm_workset(ctx)?;
    let config = ctx.store.config_layers()?.config()?;
    let effective = ctx.store.effective_operating_policy()?;
    let mut facts = vec![
        crate::app::operating::summary(&workset).replace('\n', " / "),
        format!("Effective revision: {}", effective.revision),
        format!(
            "Grant: {}",
            effective.authorization.as_deref().unwrap_or("none")
        ),
        format!(
            "Delegation: dispatch={}, landing={}, plan acceptance={}",
            config.automation.authority.dispatch,
            config.automation.authority.accept,
            config.automation.authority.plan_accept
        ),
        format!(
            "Finite tasks: {}; exclusions: {}",
            config
                .automation
                .scope
                .tasks
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            config
                .automation
                .scope
                .exclusions
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ];
    if let Some(approval) = ctx.store.operating_state()?.approval {
        facts.push(format!(
            "Confirmed by {} at {} (revision {})",
            approval.confirmed_by, approval.confirmed_at, approval.revision
        ));
    }
    facts.extend(effective.reasons);
    if !config.autopilot_enabled() {
        facts
            .push("Legacy review restriction: disabled; a finite grant cannot override it.".into());
    }
    for (name, review) in std::iter::once(("workspace", &config.review)).chain(
        config
            .projects
            .iter()
            .map(|(name, project)| (name.as_str(), &project.review)),
    ) {
        if review.accept == Some(corral_core::Accept::Owner) {
            facts.push(format!(
                "Owner exception: {name} implementation acceptance is explicitly owner-only"
            ));
        }
        if review.plan_accept == Some(corral_core::Accept::Owner) {
            facts.push(format!(
                "Owner exception: {name} plan acceptance is explicitly owner-only"
            ));
        }
        if let Some(paths) = &review.owner_paths
            && !paths.is_empty()
        {
            facts.push(format!("Owner paths: {name}: {}", paths.join(", ")));
        }
    }
    for (id, policy) in effective.tasks {
        let project = config
            .automation
            .scope
            .tasks
            .get(&id)
            .map(|member| member.project.as_str());
        facts.push(format!(
            "{id}: acceptance source {}; plan acceptance source {}",
            policy.acceptance_source.label(project),
            policy.plan_acceptance_source.label(project)
        ));
        for (action, reasons) in policy.refusals {
            facts.push(format!("{id} {action:?}: {}", reasons.join("; ")));
        }
    }
    for decision in workset.decisions {
        facts.push(format!(
            "Decision {} / {}: {}",
            decision.task,
            decision.id,
            crate::app::operating::reason(&decision.action)
        ));
    }
    for wait in workset.waits {
        facts.push(format!(
            "Wait {}: {}",
            wait.task.as_deref().unwrap_or("workspace"),
            crate::app::operating::reason(&wait)
        ));
    }
    facts.extend(workset.blockers);
    Ok(facts)
}
