//! Read-only presentation of the same projection returned by `plan progress --json`.
use corral_core::PlanDeliveryTarget;
use corral_ops::{PlanProgress, PlanWorkState};
use ratatui::{style::Style, text::Line};

use crate::theme;

pub(crate) type Progress = Result<PlanProgress, String>;

pub(crate) fn load(ctx: &corral_ops::Ctx, task: &corral_core::Task) -> Option<Progress> {
    (task.tracker && task.approved_scope.is_some())
        .then(|| corral_ops::plan_progress(ctx, &task.id).map_err(|error| format!("{error:#}")))
}

pub(crate) fn counts(progress: &PlanProgress) -> String {
    let c = &progress.counts;
    format!(
        "{} accepted · {} running · {} review · {} waiting · {} missing",
        c.accepted, c.running, c.review, c.waiting, c.missing
    )
}

pub(crate) fn badge(progress: &Progress) -> String {
    match progress {
        Ok(p) => format!(
            "Plan A{} R{} V{} W{} M{}",
            p.counts.accepted,
            p.counts.running,
            p.counts.review,
            p.counts.waiting,
            p.counts.missing
        ),
        Err(_) => "Plan unavailable".into(),
    }
}

pub(crate) fn summary(progress: &Progress) -> Vec<String> {
    let p = match progress {
        Ok(p) => p,
        Err(error) => return vec![format!("Plan progress unavailable: {error}")],
    };
    let mut lines = vec![counts(p)];
    if let Some(scope) = &p.approved_scope {
        lines.push(format!(
            "Approved scope r{}: {}",
            p.scope_revision, scope.goal
        ));
        lines.push(match &scope.approved_design {
            Some(link) => format!("Design approval: {link} (approved scope reference)"),
            None => "Design approval: no design reference recorded".into(),
        });
        lines.push(match &scope.delivery_target {
            PlanDeliveryTarget::Main => "Delivery target: main (independent increments)".into(),
            PlanDeliveryTarget::Integration { branch } => {
                format!("Delivery target: {branch} (integration; execution not available)")
            }
        });
    }
    lines.push("Final delivery: not recorded; accepted tasks do not prove target inclusion".into());
    lines.push("Final acceptance: not recorded (read-only; owner decision required)".into());
    lines
}

pub(crate) fn details(progress: &Progress) -> Vec<String> {
    let Ok(p) = progress else {
        return Vec::new();
    };
    let mut lines = vec!["Task readiness and blockers".into()];
    for work in &p.work {
        let bucket = match work.state {
            PlanWorkState::Accepted => "accepted",
            PlanWorkState::Running => "running",
            PlanWorkState::Review => "review",
            PlanWorkState::Waiting => "waiting",
            PlanWorkState::Missing => "missing",
        };
        lines.push(format!(
            "{}: {bucket}{} · {}",
            work.task,
            work.run
                .as_ref()
                .map(|r| format!(" {r}"))
                .unwrap_or_default(),
            work.reason.as_deref().unwrap_or(&work.title)
        ));
        for dep in &work.prerequisites {
            lines.push(format!(
                "  {} -> {}{}: {} · target {} · receipt {} · head {}",
                work.task,
                dep.task,
                if dep.external { " (external)" } else { "" },
                dep.reason.as_deref().unwrap_or("satisfied"),
                dep.target,
                dep.design_publication
                    .as_deref()
                    .or(dep.receipt_sha.as_deref())
                    .unwrap_or("none"),
                dep.target_head.as_deref().unwrap_or("unknown"),
            ));
        }
    }
    lines.push("Criterion coverage (mapping is not verification)".into());
    for mapping in &p.coverage {
        lines.push(format!(
            "{}: {} -> {} · tasks: {}",
            mapping
                .acceptance
                .criterion
                .as_deref()
                .unwrap_or("(unnamed)"),
            mapping.acceptance.scenario,
            mapping.acceptance.expected,
            if mapping.tasks.is_empty() {
                "MISSING COVERAGE".into()
            } else {
                mapping.tasks.join(", ")
            }
        ));
    }
    if p.coverage.is_empty() {
        lines.push("No aggregate criteria recorded.".into());
    }
    lines
}

pub(crate) fn lines(text: &[String], width: usize, style: Style) -> Vec<Line<'static>> {
    text.iter()
        .flat_map(|text| {
            crate::render::wrap::wrap(
                &crate::render::wrap::chunks(&[(text.clone(), style)]),
                width,
            )
            .into_iter()
            .map(Line::from)
        })
        .collect()
}

pub(crate) fn card_lines(progress: &Progress, width: usize) -> Vec<Line<'static>> {
    let text = match progress {
        Ok(p) => vec![
            counts(p),
            format!(
                "{} unmapped criteria · final acceptance pending",
                p.unmapped_criteria.len()
            ),
        ],
        Err(error) => vec![format!("Plan unavailable: {error}")],
    };
    lines(&text, width, Style::new().fg(theme::CYAN))
}
