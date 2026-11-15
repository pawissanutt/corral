use super::*;

pub(super) fn plans(feed: &mut Feed) {
    let definitions: &[(&str, &str, PlanState, &[&str], u32)] = &[
        (
            "T-885",
            "PM supervisor evolution: bounded reads, warm-context episodes",
            PlanState::Awaiting,
            &[],
            6,
        ),
        (
            "T-870",
            "Pi input: stable queue item lifecycle, editing, cancellation",
            PlanState::Executing,
            &["T-867", "T-868", "T-869"],
            15,
        ),
        (
            "T-893",
            "PM supervisor evolution: bounded reads, warm-context episodes",
            PlanState::Executing,
            &["T-890", "T-891", "T-892"],
            6,
        ),
        (
            "T-898",
            "Anthropic + OpenAI API connections, bounded subscription fallback",
            PlanState::Executing,
            &["T-894", "T-895", "T-896", "T-897"],
            7,
        ),
        (
            "T-874",
            "Pi review annotations: durable candidate-bound threads",
            PlanState::Executing,
            &["T-871", "T-872", "T-873"],
            12,
        ),
        (
            "T-851",
            "Co-work lifecycle",
            PlanState::Executing,
            &["T-852", "T-854", "T-855"],
            8,
        ),
        (
            "T-385",
            "Design: reliable task progress and PM attention across harnesses",
            PlanState::Approved,
            &[],
            0,
        ),
    ];
    for &(id, title, state, ids, scenarios) in definitions {
        let design = matches!(state, PlanState::Awaiting | PlanState::Approved);
        let revision = design.then(|| if id == "T-385" { "P5" } else { "P2" }.into());
        let role = Some(if design {
            PlanRole::Design
        } else {
            PlanRole::Tracker
        });
        if feed.board.task(id).is_none() {
            let mut record = task(feed, id, title, None, Group::Other, "plan");
            record.now_code = "plan".into();
            save_task(feed, record);
        }
        let members: Vec<_> = ids
            .iter()
            .filter_map(|id| feed.board.task(id))
            .enumerate()
            .map(|(index, row)| PlanMember {
                id: row.id.to_string(),
                title: row.title.clone(),
                layer: Some(if id == "T-893" || id == "T-851" {
                    index as u32
                } else if id == "T-898" && index == 3 {
                    2
                } else {
                    index.min(1) as u32
                }),
                state: row.now_code.clone(),
                rail: stages(row),
            })
            .collect();
        let edge_indices: Vec<(usize, usize)> = if id == "T-893" || id == "T-851" {
            (1..members.len()).map(|index| (index - 1, index)).collect()
        } else if id == "T-898" {
            vec![(0, 1), (0, 2), (2, 3)]
        } else {
            (1..members.len()).map(|index| (0, index)).collect()
        };
        let edges: Vec<_> = edge_indices
            .into_iter()
            .filter_map(|(from, to)| {
                Some(PlanEdge {
                    from: members.get(from)?.id.clone(),
                    to: members.get(to)?.id.clone(),
                })
            })
            .collect();
        let layers = members
            .iter()
            .filter_map(|member| member.layer)
            .max()
            .map_or(0, |layer| layer + 1);
        for member in &members {
            let target = DetailRef::Task {
                id: member.id.clone(),
            };
            if let Some(DetailBody::Task(mut task)) =
                feed.detail(&target).and_then(|cache| cache.body()).cloned()
            {
                task.plan_context = Some(PlanPosition {
                    plan: id.into(),
                    layer: member.layer.unwrap_or(0),
                    layers,
                    landed: 0,
                    total: members.len() as u32,
                });
                task.dependencies = edges
                    .iter()
                    .filter(|edge| edge.to == member.id)
                    .filter_map(|edge| {
                        let dependency = members.iter().find(|member| member.id == edge.from)?;
                        Some(TaskDependency {
                            id: dependency.id.clone(),
                            title: dependency.title.clone(),
                            status: dependency.state.clone(),
                            satisfied: dependency.state == "done",
                        })
                    })
                    .collect();
                put(feed, target, DetailBody::Task(task));
            }
        }
        feed.plans.insert(
            id.into(),
            PlanRow {
                id: id.into(),
                title: title.into(),
                role,
                linked: None,
                state,
                revision: revision.clone(),
                delivery: Some("main".into()),
                members_total: members.len() as u32,
                members_landed: 0,
                blockers: vec![],
                accepted_at: (!design).then(|| stamp(feed, feed.now.saturating_sub(10800))),
                rev: 1,
            },
        );
        put(feed, DetailRef::Plan { id: id.into() }, DetailBody::Plan(PlanDetail {
            id: id.into(), role, linked: None, revision: revision.clone(), revisions: if id == "T-885" { vec!["P1".into(), "P2".into()] } else { revision.into_iter().collect() },
            owner_decisions: vec![], state, document: Some(PlanDocument {
                problem: Some("The PM owner and agent need a bounded and consistent view of current responsibilities; operational continuity should not depend on an ever-growing conversation.".into()),
                goal: Some(format!("Preview · {title}")),
                changes: vec!["Common bounded semantic read contract for PM CLI and native tools".into(), "Existing attention and inbox records form one read-only obligation view".into()],
                retained: vec!["Owner authority, candidate identities, gates, independent review and landing checks".into()],
                non_goals: vec!["No second scheduler or task database".into()],
                proposed: if id == "T-885" { ["pm-bounded-read-contract", "pm-obligation-read-view", "pm-read-costs-and-guidance"].into_iter().enumerate().map(|(index, name)| PlanProposal {
                    name: name.into(), needs: if index == 0 { vec![] } else { vec![if index == 1 { "pm-bounded-read-contract" } else { "pm-obligation-read-view" }.into()] },
                }).collect() } else { vec![] },
            }), reviews: if design { vec![PlanReview { revision: if id == "T-385" { "P5" } else { "P2" }.into(), verdict: "approve".into(),
                profile: Some("pi-sol".into()), at: Some(stamp(feed, feed.now.saturating_sub(14400))), note: Some("Preview · no blocking defect".into()), source_ref: None }] } else { vec![] },
            members, edges, scenarios: Some(scenarios), mapped: Some(0), acceptable: state == PlanState::Awaiting, accept_blockers: vec![],
        }));
        if id == "T-885" {
            put(feed, DetailRef::PlanDiff { id: id.into(), from: "P1".into(), to: "P2".into() }, DetailBody::PlanDiff(PatchPage {
                path: "preview/plan-P2.md".into(), patch: "@@ -1 +1 @@\n-Unbounded paging\n+Bounded paging with fairness and complete detail access\n".into(), truncated: false, candidate: None,
            }));
        }
    }
    refresh_plans(feed);
}

pub(in crate::model::preview) fn refresh_plans(feed: &mut Feed) {
    let plans: Vec<_> = feed.plans.keys().cloned().collect();
    for id in plans {
        let target = DetailRef::Plan { id: id.to_string() };
        let Some(DetailBody::Plan(mut detail)) =
            feed.detail(&target).and_then(|cache| cache.body()).cloned()
        else {
            continue;
        };
        for member in &mut detail.members {
            if let Some(task) = feed.board.task(&member.id) {
                member.title = task.title.clone();
                member.state = task.now_code.clone();
                member.rail = stages(task);
            }
        }
        let landed = detail
            .members
            .iter()
            .filter(|member| member.state == "done")
            .count() as u32;
        for member in &detail.members {
            let task_ref = DetailRef::Task {
                id: member.id.clone(),
            };
            if let Some(DetailBody::Task(mut task)) = feed
                .detail(&task_ref)
                .and_then(|cache| cache.body())
                .cloned()
            {
                for dependency in &mut task.dependencies {
                    if let Some(row) = feed.board.task(&dependency.id) {
                        dependency.title = row.title.clone();
                        dependency.status = row.now_code.clone();
                        dependency.satisfied = row.now_code == "done";
                    }
                }
                task.plan_context = Some(PlanPosition {
                    plan: id.to_string(),
                    layer: member.layer.unwrap_or(0),
                    layers: detail
                        .members
                        .iter()
                        .filter_map(|member| member.layer)
                        .max()
                        .map_or(0, |layer| layer + 1),
                    landed,
                    total: detail.members.len() as u32,
                });
                put(feed, task_ref, DetailBody::Task(task));
            }
        }
        let blockers: Vec<_> = detail
            .members
            .iter()
            .filter(|member| member.state != "done")
            .map(|member| {
                if feed
                    .board
                    .task(&member.id)
                    .is_some_and(|task| task.questions > 0)
                {
                    format!("{} waits on your answer", member.id)
                } else {
                    format!("{} required work is {}", member.id, member.state)
                }
            })
            .collect();
        if let Some(row) = feed.plans.get_mut(&id) {
            row.members_total = detail.members.len() as u32;
            row.members_landed = detail
                .members
                .iter()
                .filter(|member| member.state == "done")
                .count() as u32;
            row.blockers = blockers;
        }
        if let Some(plan) = feed.plans.get(&id).cloned() {
            let key = format!(
                "plan:{}:{}",
                plan.id,
                plan.revision.as_deref().unwrap_or("current")
            );
            let at = stamp(feed, feed.now);
            if plan.state == PlanState::Awaiting {
                owner_move(
                    feed,
                    &key,
                    &plan.id,
                    Some(&plan.id),
                    &format!(
                        "Accept plan {} {}",
                        plan.id,
                        plan.revision.as_deref().unwrap_or("")
                    ),
                    InboxKind::PlanAcceptance,
                    AttentionDestination::Plans,
                );
            } else if let Some(item) = feed.inbox.get_mut(key.as_str())
                && item.state == InboxState::Open
            {
                item.state = if plan.state == PlanState::Cancelled {
                    InboxState::Cancelled
                } else {
                    InboxState::Answered {
                        by: "owner".into(),
                        via: "preview:local-only".into(),
                        at,
                    }
                };
                item.rev += 1;
            }
            let mut owner = feed
                .board
                .task(&id)
                .cloned()
                .unwrap_or_else(|| task(feed, &id, &plan.title, None, Group::Other, "plan"));
            owner.attention.retain(|attention| attention != &key);
            if plan.state == PlanState::Awaiting {
                owner.attention.push(key);
                owner.group = Group::NeedsYou;
            } else {
                owner.group = Group::Other;
            }
            owner.rev = owner.rev.max(plan.rev);
            owner.now_code = match plan.state {
                PlanState::Cancelled => "cancelled",
                PlanState::Complete | PlanState::Approved => "done",
                _ => "plan",
            }
            .into();
            owner.now = format!("Preview · plan {:?}", plan.state);
            save_task(feed, owner);
        }
        put(feed, target, DetailBody::Plan(detail));
    }
}
