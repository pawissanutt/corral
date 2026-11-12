use anyhow::Result;
use corral_core::{Status, Task};
use corral_ops::Ctx;

use super::{autopilot_line, gate_line, parse_status, review_line, route};

#[cfg(test)]
#[path = "task_output_tests.rs"]
mod hot_read_tests;

pub(super) fn show(ctx: &Ctx, id: &str, json: bool) -> Result<()> {
    corral_ops::refresh_role_sessions(ctx, id)?;
    corral_ops::refresh_usage(&ctx.store, id)?;
    let t = ctx.store.get(id)?;
    if json {
        let mut value = task_json(&t, true);
        value["total_usage"] = serde_json::to_value(t.total_usage())?;
        value["automation_blocker"] =
            serde_json::to_value(corral_ops::automation_blocker(ctx, &t)?)?;
        let gate = corral_ops::gate_readback(ctx, &t)?;
        value["gate_contract"] = serde_json::to_value(&gate.gate_contract)?;
        value["gate_ref"] = serde_json::to_value(&gate.gate_ref)?;
        let policy = ctx.store.config.task_review_policy(&t);
        value["session_policy"] = serde_json::json!({
            "worker": policy.worker_session, "reviewer": policy.reviewer_session
        });
        if let Some(error) = gate.gate_contract_error {
            value["gate_contract_error"] = error.into();
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("{}  {}", t.id, t.title);
    println!("status:   {}", t.status.label());
    if let Some(reason) = corral_ops::automation_blocker(ctx, &t)? {
        println!("automation blocked: {reason}");
    }
    println!("project:  {}", t.project.as_deref().unwrap_or("-"));
    let policy = ctx.store.config.task_review_policy(&t);
    println!(
        "sessions: worker={:?}, reviewer={:?}",
        policy.worker_session, policy.reviewer_session
    );
    println!("priority: {}", t.priority);
    if let Some(kind) = &t.kind {
        println!("kind:     {kind}");
    }
    if let Some(size) = t.size {
        println!("size:     {size}");
    }
    if let Some(difficulty) = t.difficulty {
        println!("difficulty: {difficulty}");
    }
    if !t.labels.is_empty() {
        println!("labels:   {}", t.labels.join(", "));
    }
    if let Some(p) = &t.profile {
        println!("profile:  {p}");
    }
    if !t.depends_on.is_empty() {
        println!("depends:  {}", t.depends_on.join(", "));
    }
    if let Some(plan) = &t.plan {
        println!("plan:     {plan}");
    }
    if let Some(parent) = &t.parent {
        println!("parent:   {parent}");
    }
    if t.tracker {
        println!("tracker:  yes");
        println!("scope revision: {}", t.approved_scope_revision);
        if let Some(scope) = &t.approved_scope {
            println!("goal:     {}", scope.goal);
            match &scope.delivery_target {
                corral_core::PlanDeliveryTarget::Main => println!("delivery: main (tracking only)"),
                corral_core::PlanDeliveryTarget::Integration { branch } => {
                    println!("delivery: {branch} (integration intent; tracking only)");
                }
            }
            if let Some(link) = &scope.approved_design {
                println!("design:   {link}");
            }
            for acceptance in &scope.acceptance {
                println!(
                    "acceptance{}: {} -> {}",
                    acceptance
                        .criterion
                        .as_ref()
                        .map(|id| format!(" {id}"))
                        .unwrap_or_default(),
                    acceptance.scenario,
                    acceptance.expected
                );
            }
        } else {
            println!("delivery: main (tracking only; no approved scope)");
        }
        let children = ctx.store.list()?;
        let done = t
            .children
            .iter()
            .filter(|id| {
                children
                    .iter()
                    .any(|child| child.id == **id && child.status == Status::Done)
            })
            .count();
        println!("progress: {done}/{} done", t.children.len());
    }
    if !t.children.is_empty() {
        println!("children: {}", t.children.join(", "));
    }
    for run in &t.plan_runs {
        println!(
            "plan run {} [{}]: {:?} ({})",
            run.id,
            run.profile,
            run.state,
            run.model.as_deref().unwrap_or("default")
        );
    }
    if t.queued {
        println!("queued:   yes");
    }
    for r in &t.runs {
        println!(
            "run {} [{}] {} branch {} worktree {}{}",
            r.id,
            route::run_profile(r),
            r.outcome
                .map(|o| format!("{o:?}"))
                .unwrap_or_else(|| "active".into()),
            r.branch,
            r.worktree.display(),
            r.summary
                .as_deref()
                .map(|s| format!("\n    summary: {s}"))
                .unwrap_or_default()
        );
        crate::stats::show_run_usage(r, "    ");
        show_session(r, "worker");
        for child in &r.review_runs {
            show_session(child, "reviewer");
        }
        for line in r.obligations.lines() {
            println!("    {line}");
        }
        crate::run_cmd::show_run(r, "    ");
        if let Some(g) = &r.gate {
            println!("    gate: {}", gate_line(g, r));
        }
        if let Some(rv) = &r.review {
            println!("    review: {}", review_line(rv));
        }
        if let Some(w) = t
            .cowork
            .as_ref()
            .and_then(|c| c.review_waiver.as_ref())
            .filter(|w| w.run == r.id)
        {
            println!("    review: waived by the owner at {}: {}", w.at, w.reason);
        }
        if let Some(a) = &r.autopilot {
            let max = ctx
                .store
                .config
                .review_policy(t.project.as_deref(), t.review)
                .max_rounds;
            println!("    autopilot: {}", autopilot_line(a, t.rounds, max));
        }
        if let Some(route) = &r.route {
            println!("    route: {route}");
        }
        for switch in &r.switches {
            println!(
                "    switch: {} -> {}: {}",
                switch.from, switch.to, switch.reason
            );
        }
    }
    crate::stats::show_total_usage(&t);
    show_criteria(&t);
    crate::artifact_show::print_grouped(&t.artifacts, true);
    if !t.body.trim().is_empty() {
        println!("\n{}", t.body.trim_end());
    }
    for comment in &t.comments {
        println!(
            "\ncomment by {} at {}:\n{}",
            comment.author.as_str(),
            comment.at,
            comment.text
        );
    }
    Ok(())
}

fn show_session(run: &corral_core::Run, role: &str) {
    if let Some(session) = &run.session {
        println!(
            "    {role} session ({}): {:?} {:?} {}",
            run.id,
            session.policy,
            session.action,
            session.id.as_deref().unwrap_or("not yet observed")
        );
        if let Some(reason) = &session.diagnostic {
            println!("      {reason}");
        }
    }
}

pub(super) fn list_view(start: &std::path::Path, json: bool) -> Result<()> {
    use corral_protocol::board::BoardGroup;
    let root = corral_core::Store::find_root(start)?;
    let socket = corral_ops::daemon_client::socket_path_of(&root.join(".corral"));
    let unavailable = |reason: &str| -> ! {
        eprintln!("board view unavailable ({reason}); run `corral daemon status`");
        std::process::exit(2);
    };
    let mut client = match corral_ops::daemon_client::Client::connect_at(&socket) {
        Ok(Some(client)) => client,
        Ok(None) => unavailable("daemon is not running"),
        Err(error) => unavailable(&error.to_string().replace(['\n', '\r'], " ")),
    };
    let rows = client.board()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for (group, heading) in [
        (BoardGroup::NeedsYou, "Needs you"),
        (BoardGroup::Verifying, "Corral is verifying"),
        (BoardGroup::Producing, "Workers producing"),
        (BoardGroup::Queued, "Queued"),
        (BoardGroup::Held, "Held"),
        (BoardGroup::Parked, "Parked"),
        (BoardGroup::Other, "Other"),
    ] {
        let mut grouped = rows.iter().filter(|row| row.group == group).peekable();
        if grouped.peek().is_none() {
            continue;
        }
        println!("{heading}");
        for row in grouped {
            // Spaces, not `·`: that character is itself the "not applicable" glyph.
            println!(
                "{}  {} {} {} {}  {}  {}",
                row.id,
                row.rail[0].glyph.glyph(),
                row.rail[1].glyph.glyph(),
                row.rail[2].glyph.glyph(),
                row.rail[3].glyph.glyph(),
                row.now.text,
                row.title,
            );
        }
    }
    Ok(())
}

pub(super) fn list(
    ctx: &Ctx,
    status: Option<&str>,
    project: Option<&str>,
    all: bool,
    json: bool,
    columns: &[String],
    query: Option<&str>,
) -> Result<()> {
    let want = status.map(parse_status).transpose()?;
    if query.is_none() {
        let open = (!all || want == Some(Status::NeedsMe))
            && !matches!(want, Some(Status::Done | Status::Dropped));
        let _hot = open.then(corral_core::work::HotScope::enter);
        let ctx = &Ctx {
            store: ctx.store.read_only_snapshot()?,
            ..ctx.clone()
        };
        let assessed = (want.is_some() || json)
            .then(|| corral_ops::attention::board_hot_heads(ctx, jiff::Timestamp::now()))
            .transpose()?;
        let mut heads = if open {
            match &assessed {
                Some((heads, _)) => heads
                    .iter()
                    .filter(|head| !matches!(head.status, Status::Done | Status::Dropped))
                    .cloned()
                    .collect(),
                None => ctx.store.heads(&corral_core::backend::HeadQuery::Open {
                    project: project.map(str::to_owned),
                })?,
            }
        } else {
            let statuses: Vec<_> = match want {
                Some(status) => vec![status],
                None => vec![
                    Status::Idea,
                    Status::Backlog,
                    Status::Ready,
                    Status::Running,
                    Status::NeedsMe,
                    Status::Review,
                    Status::Done,
                    Status::Dropped,
                ],
            };
            let mut heads = Vec::new();
            for status in statuses {
                // `heads` follows the backend's pages for explicit history-sized output.
                heads.extend(ctx.store.heads(
                    &corral_core::backend::HeadQuery::RecentByStatus {
                        status: status.as_str().into(),
                    },
                )?);
            }
            heads.sort_by_key(|head| head.num);
            heads
        };
        heads.retain(|head| project.is_none_or(|p| head.project.as_deref() == Some(p)));
        let assessments: std::collections::BTreeMap<_, _> = assessed
            .into_iter()
            .flat_map(|(_, board)| board.tasks)
            .map(|row| (row.task, row.assessment))
            .collect();
        let mut selected = Vec::new();
        let mut conditions = std::collections::BTreeMap::new();
        for head in heads {
            let condition = assessments
                .get(&head.id)
                .map(|assessment| corral_core::attention::blocked_ref(&head, assessment))
                .transpose()?
                .flatten();
            let keep = match want {
                Some(Status::NeedsMe) => condition.is_some(),
                Some(status) => head.status == status && condition.is_none(),
                None => true,
            };
            if keep {
                if let Some(condition) = condition {
                    conditions.insert(head.id.clone(), condition.clone());
                }
                selected.push(head);
            }
        }
        if json {
            let values = selected
                .iter()
                .map(|head| -> Result<_> {
                    let mut value = task_head_json(head)?;
                    let condition = conditions.get(&head.id);
                    value["display_status"] =
                        condition.map_or(head.status.label(), |_| "Blocked").into();
                    value["blocked"] = serde_json::json!(condition);
                    value["unblocker"] = serde_json::json!(condition.map(|c| c.unblocker()));
                    Ok(value)
                })
                .collect::<Result<Vec<_>>>()?;
            println!("{}", serde_json::to_string_pretty(&values)?);
            return Ok(());
        }
        return crate::list_cols::print_heads(ctx, &selected, columns);
    }
    let tasks = match query {
        Some(input) => corral_ops::search_tasks(&ctx.store, input)?,
        None => ctx.store.list()?,
    };
    let assessment = if want.is_some() || json {
        Some(corral_ops::attention::board(ctx, jiff::Timestamp::now())?)
    } else {
        None
    };
    let blocked = |t: &Task| {
        assessment
            .as_ref()
            .and_then(|board| board.tasks.iter().find(|row| row.task == t.id))
            .and_then(|row| corral_core::attention::blocked(t, &row.assessment))
    };
    let tasks: Vec<Task> = tasks
        .into_iter()
        .filter(|t| match want {
            Some(Status::NeedsMe) => blocked(t).is_some(),
            Some(s) => t.status == s && blocked(t).is_none(),
            None => query.is_some() || all || !matches!(t.status, Status::Done | Status::Dropped),
        })
        .filter(|t| project.is_none_or(|p| t.project.as_deref() == Some(p)))
        .collect();
    if json {
        let v: Vec<_> = tasks
            .iter()
            .map(|t| {
                let mut value = task_json(t, false);
                let condition = blocked(t);
                value["display_status"] = condition.map_or(t.status.label(), |_| "Blocked").into();
                value["blocked"] = serde_json::json!(condition);
                value["unblocker"] = serde_json::json!(condition.map(|c| c.unblocker()));
                value
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    crate::list_cols::print(ctx, &tasks, columns)
}

pub(crate) fn task_head_json(head: &corral_core::TaskHead) -> Result<serde_json::Value> {
    let mut value = task_json(&head.to_shell_task()?, false);
    if let Some(object) = value.as_object_mut() {
        object.remove("path");
    }
    Ok(value)
}

/// History vectors a summary leaves out; `show --json` keeps them.
const HISTORY: [&str; 8] = [
    "runs",
    "plan_runs",
    "plans",
    "comments",
    "artifacts",
    "aggregate_attempts",
    "approved_scope_history",
    "agent_pm_receipts",
];

/// The full task (`with_body`, as `show --json` prints it) or its summary: the task's own
/// fields and its current run, without history.
pub(crate) fn task_json(t: &Task, with_body: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
    if let Some(o) = v.as_object_mut() {
        if t.tracker {
            o.insert(
                "approved_scope_revision".into(),
                serde_json::json!(t.approved_scope_revision),
            );
        }
        o.insert("origin".into(), serde_json::json!(t.effective_origin()));
        if with_body {
            o.insert("body".into(), t.body.clone().into());
            o.insert("comments".into(), serde_json::json!(t.comments));
            if let Some(run) = t.current_run() {
                o.insert(
                    "criteria_progress".into(),
                    serde_json::json!(corral_core::criterion_progress(
                        t,
                        run,
                        run.progress.as_ref()
                    )),
                );
            }
        } else {
            for key in HISTORY {
                o.remove(key);
            }
            if let Some(serde_json::Value::Object(gates)) = o.get_mut("member_gates") {
                for gate in gates.values_mut() {
                    if let Some(gate) = gate.as_object_mut() {
                        gate.remove("history");
                    }
                }
            }
            if let Some(run) = t
                .current_run()
                .and_then(|run| corral_core::RunHead::of(run).ok())
            {
                o.insert(
                    "current_run".into(),
                    serde_json::to_value(run).unwrap_or(serde_json::Value::Null),
                );
            }
        }
    }
    v
}

pub(super) fn show_criteria(task: &Task) {
    let Some(run) = task.current_run() else {
        return;
    };
    for item in corral_core::criterion_progress(task, run, run.progress.as_ref()) {
        println!(
            "criterion {}: {}",
            item.criterion.id, item.criterion.scenario
        );
        if item.steps.is_empty() {
            println!("    progress: untracked");
        }
        for step in item.steps {
            println!("    progress {}: {}", step.id, step.status.label());
        }
        if item.evidence.is_empty() {
            println!("    evidence: none");
        }
        for record in item.evidence {
            let observation = record.observation;
            println!(
                "    evidence {} [{:?}] run {}: {}",
                observation.key,
                observation.status,
                observation.run,
                observation.artifacts.join(", ")
            );
        }
    }
}
