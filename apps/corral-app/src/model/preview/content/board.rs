use super::*;

pub(super) const TASKS: &[(&str, &str, Option<&str>, Group, &str)] = &[
    (
        "T-868",
        "host-queue-ledger",
        Some("T-870"),
        Group::NeedsYou,
        "needs you · restore policy",
    ),
    (
        "T-890",
        "pm-bounded-read-contract",
        Some("T-893"),
        Group::Verifying,
        "quick gate 1751bd2",
    ),
    (
        "T-894",
        "api-connections",
        Some("T-898"),
        Group::Verifying,
        "gate 6b36916",
    ),
    (
        "T-867",
        "queue-contract-core",
        Some("T-870"),
        Group::Verifying,
        "stale 5949a4b · re-gate",
    ),
    (
        "T-852",
        "Co-work C1: shared completion engine + failure triage",
        Some("T-851"),
        Group::Held,
        "held · external op",
    ),
    (
        "T-871",
        "annotation-service",
        Some("T-874"),
        Group::Producing,
        "working · step 1/3",
    ),
    (
        "T-854",
        "Co-work C3: save, take over, take back",
        Some("T-851"),
        Group::Producing,
        "working · publication",
    ),
    (
        "T-891",
        "pm-obligation-read-view",
        Some("T-893"),
        Group::Queued,
        "after T-890",
    ),
    (
        "T-892",
        "pm-read-costs-and-guidance",
        Some("T-893"),
        Group::Queued,
        "after T-891",
    ),
    (
        "T-895",
        "failure-signals",
        Some("T-898"),
        Group::Queued,
        "after T-894",
    ),
    (
        "T-896",
        "metered-fallback",
        Some("T-898"),
        Group::Queued,
        "after T-894",
    ),
    (
        "T-897",
        "role-continuations",
        Some("T-898"),
        Group::Queued,
        "after T-896",
    ),
    (
        "T-869",
        "inbox-queue-ops",
        Some("T-870"),
        Group::Queued,
        "after T-867",
    ),
    (
        "T-872",
        "annotation-owner-cli",
        Some("T-874"),
        Group::Queued,
        "after T-871",
    ),
    (
        "T-873",
        "annotation-pi-host-api",
        Some("T-874"),
        Group::Queued,
        "after T-871",
    ),
    (
        "T-855",
        "Co-work C4: assistant + board handoff TUI",
        Some("T-851"),
        Group::Queued,
        "after T-854",
    ),
    (
        "T-883",
        "Record reviewer child outcome for co-work resubmit",
        None,
        Group::Queued,
        "queued · capacity",
    ),
    (
        "T-832",
        "Co-work finish missing usage after diag failure",
        None,
        Group::Queued,
        "queued · capacity",
    ),
    (
        "T-853",
        "Co-work C2: shared checkpoints and follow-ups",
        Some("T-851"),
        Group::Other,
        "done",
    ),
    (
        "T-701",
        "Pi input queue lifecycle design",
        None,
        Group::Other,
        "done",
    ),
    (
        "T-901",
        "Review: inline comments bound to the candidate SHA",
        None,
        Group::Queued,
        "idea",
    ),
    (
        "T-902",
        "Board: PM inbox count in the title bar",
        None,
        Group::Queued,
        "idea",
    ),
    (
        "T-903",
        "Gate: reuse receipts for unchanged crates after a base move",
        None,
        Group::Queued,
        "idea",
    ),
];

pub(in crate::model::preview) fn rail(group: Group) -> [Step; 4] {
    let marks = match group {
        Group::Verifying => [Mark::Done, Mark::Active, Mark::Next, Mark::Next],
        Group::Producing | Group::NeedsYou => [Mark::Active, Mark::Next, Mark::Next, Mark::Next],
        Group::Held => [Mark::Done, Mark::Stale, Mark::Next, Mark::Next],
        Group::Other => [Mark::Done; 4],
        _ => [Mark::Next; 4],
    };
    marks.map(|mark| Step {
        mark,
        sha7: None,
        at: None,
    })
}

pub(in crate::model::preview) fn stages(task: &Task) -> [RailStep; 4] {
    task.rail.clone().map(|step| RailStep {
        glyph: match step.mark {
            Mark::Done => RailGlyph::Pass,
            Mark::Active => RailGlyph::Active,
            Mark::Fail => RailGlyph::Fail,
            Mark::Stale => RailGlyph::Stale,
            Mark::Next => RailGlyph::Pending,
            _ => RailGlyph::Unavailable,
        },
        sha7: step.sha7,
        at: None,
    })
}

pub(in crate::model::preview) fn task(
    feed: &Feed,
    id: &str,
    title: &str,
    plan: Option<&str>,
    group: Group,
    now: &str,
) -> Task {
    Task {
        id: id.into(),
        title: title.into(),
        project: Some("corral".into()),
        plan: plan.map(str::to_owned),
        goal: Some(format!("Preview · {title}")),
        group,
        rail: rail(group),
        now: now.into(),
        now_code: match group {
            Group::Queued => "ready",
            Group::Other => "done",
            _ => "running",
        }
        .into(),
        age_since: Some(feed.now.saturating_sub(10800)),
        since: Some(feed.now.saturating_sub(540)),
        held_reason: (group == Group::Held).then(|| "external operation".into()),
        questions: 0,
        attention: vec![],
        agent: None,
        cost: Some(3.9),
        cost_estimated: true,
        rev: 1,
    }
}

pub(in crate::model::preview) fn save_task(feed: &mut Feed, row: Task) {
    let target = DetailRef::Task {
        id: row.id.to_string(),
    };
    let old = feed.detail(&target).and_then(|cache| cache.body()).cloned();
    let mut detail = match old {
        Some(DetailBody::Task(detail)) => detail,
        _ => TaskDetail {
            id: row.id.to_string(),
            owner_revision: "preview-1".into(),
            title: row.title.clone(),
            status: row.now_code.clone(),
            body_excerpt: row.goal.clone().unwrap_or_default(),
            stages: stages(&row),
            dependencies: vec![],
            progress: Some(TaskProgress {
                completed: 2,
                total: 5,
                text: Some("Preview · implementation and verification".into()),
            }),
            owner_next: None,
            hold: None,
            park: None,
            park_revision: None,
            artifacts: vec![],
            run: None,
            plan_context: None,
            log_tail: vec![],
        },
    };
    detail.owner_revision = format!("preview-{}", row.rev);
    detail.title = row.title.clone();
    detail.status = row.now_code.clone();
    detail.stages = stages(&row);
    detail.body_excerpt = row.goal.clone().unwrap_or_default();
    detail.owner_next =
        (row.questions > 0).then(|| "Answer the open restore-policy question".into());
    detail.hold = (row.group == Group::Held || row.held_reason.is_some()).then(|| TaskHold {
        reason: row.held_reason.clone(),
        since: Some(stamp(feed, feed.now)),
        by: Some("owner".into()),
    });
    if let Some(agent) = &row.agent {
        let run = detail.run.get_or_insert_with(|| TaskRun {
            id: "R1".into(),
            session: agent.session.clone(),
            profile: agent.profile.clone(),
            model: Some("gpt-6.1-sol".into()),
            effort: Some("xhigh".into()),
            base: Some("0123456789abcdef".into()),
            head: None,
            started_at: Some(stamp(feed, feed.now.saturating_sub(10800))),
            ended_at: None,
        });
        run.session = agent.session.clone();
        run.profile = agent.profile.clone();
    }
    if detail.plan_context.is_none()
        && let Some(plan) = &row.plan
    {
        detail.plan_context = Some(PlanPosition {
            plan: plan.clone(),
            layer: 1,
            layers: 3,
            landed: 0,
            total: 3,
        });
    }
    feed.board.apply_batch(feed.board.epoch, vec![row], &[]);
    put(feed, target, DetailBody::Task(detail));
}
