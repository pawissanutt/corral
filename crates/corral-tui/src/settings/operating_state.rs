//! Fresh read-only policy facts remain inspectable even while ordinary settings are dirty.
use std::collections::BTreeSet;

use anyhow::Result;
use corral_core::{Accept, OperatingMode, Status};
use corral_ops::Ctx;

#[derive(Default)]
pub(super) struct Facts {
    pub summary: String,
    pub overview: Vec<(&'static str, String)>,
    pub details: Vec<String>,
}

fn yes(on: bool) -> &'static str {
    if on { "yes" } else { "no" }
}

pub(super) fn facts(ctx: &Ctx) -> Result<Facts> {
    let workset = corral_ops::pm_workset(ctx)?;
    let config = ctx.store.config_layers()?.config()?;
    let effective = ctx.store.effective_operating_policy()?;
    let scope = &config.automation.scope;
    let authority = &config.automation.authority;
    let finished = |id: &str| {
        ctx.store
            .get(id)
            .is_ok_and(|task| matches!(task.status, Status::Done | Status::Dropped))
    };
    let mut owner_only = Vec::new();
    for (name, review) in std::iter::once(("workspace", &config.review)).chain(
        config
            .projects
            .iter()
            .map(|(name, project)| (name.as_str(), &project.review)),
    ) {
        if review.accept == Some(Accept::Owner) {
            owner_only.push(name.to_owned());
        }
    }

    let mut overview = Vec::new();
    let mut covers = Vec::new();
    for project in scope.projects.keys() {
        let mut text = format!("{project}: all unfinished tasks, including new ones");
        if let Some(max) = scope.max_new_tasks {
            text.push_str(&format!(" (up to {max} new)"));
        }
        covers.push(text);
    }
    let active = effective
        .tasks
        .keys()
        .filter(|id| !finished(id))
        .cloned()
        .collect::<Vec<_>>();
    let done = scope.tasks.keys().filter(|id| finished(id)).count();
    if !active.is_empty() {
        covers.push(format!("{} active: {}", active.len(), active.join(", ")));
    }
    if done > 0 {
        covers.push(format!("{done} finished"));
    }
    if let Some(days) = scope.expires_after_days
        && let Some(approval) = ctx.store.operating_state()?.approval
    {
        covers.push(format!(
            "ends {}",
            approval
                .confirmed_at
                .checked_add(jiff::SignedDuration::from_hours(i64::from(days) * 24))
                .map_or_else(
                    |_| "never".into(),
                    |end| end.strftime("%Y-%m-%d %H:%M").to_string()
                )
        ));
    }
    overview.push((
        "Covers",
        if covers.is_empty() {
            "nothing yet".into()
        } else {
            covers.join(" · ")
        },
    ));
    let allowed = if effective.mode == OperatingMode::Assist {
        "nothing runs on its own (Assist)".to_owned()
    } else if config.automation.paused {
        "nothing new starts (paused)".to_owned()
    } else {
        let land = if !authority.accept {
            "no".to_owned()
        } else if owner_only.iter().any(|name| name == "workspace") {
            "no (owner-only)".to_owned()
        } else if owner_only.is_empty() {
            "yes".to_owned()
        } else {
            format!("yes, except owner-only {}", owner_only.join(", "))
        };
        format!(
            "start work {} · check & review {} · land {land} · accept plans {}",
            yes(authority.dispatch),
            yes(authority.dispatch),
            yes(authority.plan_accept),
        )
    };
    overview.push(("Allowed", allowed));
    if let Some(reason) = effective.reasons.first()
        && effective.mode == OperatingMode::Assist
        && config.automation.mode == OperatingMode::Autopilot
    {
        overview.push(("Stopped", reason.clone()));
    }
    let mut asks: Vec<(String, Vec<&str>)> = Vec::new();
    for decision in &workset.decisions {
        let reason = crate::app::operating::reason(&decision.action);
        match asks.iter_mut().find(|(known, _)| *known == reason) {
            Some((_, tasks)) => tasks.push(&decision.task),
            None => asks.push((reason, vec![&decision.task])),
        }
    }
    for (reason, tasks) in asks.iter().take(2) {
        let who = match tasks.len() {
            0..=3 => tasks.join(", "),
            n => format!("{} +{}", tasks[..3].join(", "), n - 3),
        };
        overview.push(("Needs you", format!("{who}: {reason}")));
    }
    if asks.len() > 2 {
        overview.push((
            "Needs you",
            format!("{} more in Policy details", asks.len() - 2),
        ));
    }

    let mut details = vec![
        format!("Effective revision: {}", effective.revision),
        format!(
            "Grant: {}",
            effective.authorization.as_deref().unwrap_or("none")
        ),
        format!(
            "Delegation: dispatch={}, landing={}, plan acceptance={}",
            authority.dispatch, authority.accept, authority.plan_accept
        ),
        format!(
            "Finite tasks: {}; exclusions: {}",
            scope.tasks.keys().cloned().collect::<Vec<_>>().join(", "),
            scope
                .exclusions
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ];
    if let Some(approval) = ctx.store.operating_state()?.approval {
        details.push(format!(
            "Confirmed by {} at {} (revision {})",
            approval.confirmed_by, approval.confirmed_at, approval.revision
        ));
    }
    details.extend(effective.reasons.iter().cloned());
    if !config.autopilot_enabled() {
        details
            .push("Legacy review restriction: disabled; a finite grant cannot override it.".into());
    }
    for (name, review) in std::iter::once(("workspace", &config.review)).chain(
        config
            .projects
            .iter()
            .map(|(name, project)| (name.as_str(), &project.review)),
    ) {
        if review.accept == Some(Accept::Owner) {
            details.push(format!(
                "Owner exception: {name} implementation acceptance is explicitly owner-only"
            ));
        }
        if review.plan_accept == Some(Accept::Owner) {
            details.push(format!(
                "Owner exception: {name} plan acceptance is explicitly owner-only"
            ));
        }
        if let Some(paths) = &review.owner_paths
            && !paths.is_empty()
        {
            details.push(format!("Owner paths: {name}: {}", paths.join(", ")));
        }
    }
    let hidden = effective
        .tasks
        .keys()
        .filter(|id| finished(id))
        .collect::<BTreeSet<_>>();
    for (id, policy) in &effective.tasks {
        if hidden.contains(id) {
            continue;
        }
        let project = scope.tasks.get(id).map(|member| member.project.as_str());
        details.push(format!(
            "{id}: acceptance source {}; plan acceptance source {}",
            policy.acceptance_source.label(project),
            policy.plan_acceptance_source.label(project)
        ));
        for (action, reasons) in &policy.refusals {
            details.push(format!("{id} {action:?}: {}", reasons.join("; ")));
        }
    }
    if !hidden.is_empty() {
        details.push(format!("{} finished tasks hidden", hidden.len()));
    }
    for decision in &workset.decisions {
        details.push(format!(
            "Decision {} / {}: {}",
            decision.task,
            decision.id,
            crate::app::operating::reason(&decision.action)
        ));
    }
    for wait in &workset.waits {
        details.push(format!(
            "Wait {}: {}",
            wait.task.as_deref().unwrap_or("workspace"),
            crate::app::operating::reason(wait)
        ));
    }
    details.extend(workset.blockers.iter().cloned());
    Ok(Facts {
        summary: crate::app::operating::summary(&workset)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned(),
        overview,
        details,
    })
}
