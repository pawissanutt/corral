//! The sidebar's Context tab: the agents at work (from the fleet corrald sends with the live
//! pages), the commands the host lists (`DetailRef::SessionJobs`), then what the session
//! works with as its host reports it (`DetailRef::SessionContext`): its directory, worktree and
//! branch, the model and effort and how full the context window is, the loaded context files,
//! skills, prompts and tools, and the MCP servers with their state. A row opens its agent or job
//! in the Agents tab; an agent that asks the owner docks its question.

use corral_protocol::detail::{DetailBody, DetailRef, SessionContext, SessionJob, SessionJobs};
use corral_protocol::subscription::SessionChild;

use crate::model::board::Mark;
use crate::model::hue::Hue;

use super::super::status::tokens;
use super::super::{Action, Cx, DockAction, FleetAction};
use super::SidebarAction;

#[derive(Debug, Clone, PartialEq)]
pub struct ContextTab {
    /// Agents at work, asking or failed, in the fleet's order.
    pub agents: Vec<SideAgent>,
    /// `none at work`, or why no agent list is here.
    pub agents_note: Option<String>,
    /// `Completed · 2` and the ids, joined.
    pub completed: Option<(String, String)>,
    /// Shown only while the host lists a job.
    pub jobs: Vec<SideJob>,
    /// Why the commands could not be read.
    pub jobs_note: Option<(Hue, String)>,
    /// `no agents or commands yet`.
    pub empty: Option<String>,
    /// Directory, worktree, branch, model, effort, context window.
    pub facts: Vec<ContextFact>,
    /// Context files, skills, prompts, tools; empty until the context is read.
    pub lists: Vec<ContextList>,
    /// The MCP servers with their state; none until the context is read.
    pub mcp: Option<Vec<McpRow>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SideAgent {
    /// `side-agent-<id>`.
    pub test_id: String,
    pub child: String,
    pub mark: Mark,
    pub hue: Hue,
    /// `running`, `asks you`; heard beside the mark.
    pub state: &'static str,
    /// `slice · gpt-6`.
    pub profile: String,
    /// What it does now, or how it ended.
    pub activity: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SideJob {
    /// `side-job-<id>`.
    pub test_id: String,
    pub job: String,
    pub mark: Mark,
    pub hue: Hue,
    pub command: String,
    /// `1m 02s`, `exit 0`, `aborted`.
    pub state: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextFact {
    pub label: &'static str,
    pub value: String,
    /// A path, branch or model: drawn in the mono face.
    pub mono: bool,
    pub hue: Hue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextList {
    /// `Context files`.
    pub label: &'static str,
    pub items: Vec<String>,
    /// What an empty list says.
    pub empty: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpRow {
    pub name: String,
    pub state: String,
    pub hue: Hue,
}

/// A child's mark, hue and word in the rail; none for one that finished cleanly (it is among
/// the completed).
fn child_mark(child: &SessionChild) -> Option<(Mark, Hue, &'static str)> {
    if child.question == Some(true) || child.state == "needs" {
        return Some((Mark::Active, Hue::You, "asks you"));
    }
    match child.state.as_str() {
        "running" => Some((Mark::Active, Hue::Worker, "running")),
        "queued" => Some((Mark::Next, Hue::Neutral, "queued")),
        "failed" => Some((Mark::Fail, Hue::Fail, "failed")),
        "done" | "cancelled" => None,
        _ => Some((Mark::Na, Hue::Neutral, "state not known to this app")),
    }
}

fn activity(child: &SessionChild) -> String {
    if let Some(error) = &child.error {
        return error.clone();
    }
    if child.question == Some(true) || child.state == "needs" {
        return "waiting for your answer".into();
    }
    match child.state.as_str() {
        "queued" if !child.after.is_empty() => format!("waits for {}", child.after.join(", ")),
        "queued" => "waits for a free slot".into(),
        "running" => child.activity.clone().unwrap_or_else(|| "working".into()),
        "failed" => "failed".into(),
        _ => "–".into(),
    }
}

/// `slice · gpt-6`: the profile (or the helper's kind) and the model's own name.
fn profile(child: &SessionChild) -> String {
    let lead = child.profile.clone().unwrap_or_else(|| {
        child
            .kind
            .strip_prefix("helper:")
            .map_or_else(|| child.kind.clone(), |kind| format!("{kind} helper"))
    });
    let model = child.model.rsplit('/').next().unwrap_or(&child.model);
    if model.is_empty() {
        lead
    } else {
        format!("{lead} · {model}")
    }
}

fn agent(
    cx: &Cx<'_>,
    child: &SessionChild,
    (mark, hue, state): (Mark, Hue, &'static str),
) -> SideAgent {
    let card = (hue == Hue::You)
        .then(|| {
            cx.state?
                .cards
                .iter()
                .find(|card| card.child.as_deref() == Some(child.id.as_str()))
        })
        .flatten();
    let action = match card {
        Some(card) => Action::Dock(DockAction::Bring {
            session: cx.id.clone(),
            card: card.id.clone(),
        }),
        None => Action::Sidebar(SidebarAction::Agent {
            session: cx.id.clone(),
            child: child.id.clone(),
        }),
    };
    SideAgent {
        test_id: format!("side-agent-{}", child.id),
        child: child.id.clone(),
        mark,
        hue,
        state,
        profile: profile(child),
        activity: activity(child),
        action,
    }
}

fn jobs_read<'a>(cx: &Cx<'a>) -> Option<&'a SessionJobs> {
    match cx.feed.detail(&jobs_target(cx))?.body()? {
        DetailBody::SessionJobs(jobs) => Some(jobs),
        _ => None,
    }
}

pub(super) fn jobs_target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionJobs {
        id: cx.id.to_string(),
    }
}

fn job(cx: &Cx<'_>, job: &SessionJob) -> SideJob {
    let running = job.state == "running";
    let failed = job.exit_code.is_some_and(|code| code != 0)
        || matches!(job.state.as_str(), "failed" | "aborted" | "killed");
    let (mark, hue) = if running {
        (Mark::Active, Hue::Worker)
    } else if failed {
        (Mark::Fail, Hue::Fail)
    } else if job.ended_at.is_some() || job.exit_code.is_some() {
        (Mark::Done, Hue::Neutral)
    } else {
        (Mark::Na, Hue::Neutral)
    };
    let state = if running {
        job.started_at
            .as_deref()
            .and_then(crate::model::agents::unix)
            .filter(|start| cx.feed.now > 0 && *start <= cx.feed.now)
            .map_or_else(
                || "running".into(),
                |start| super::super::duration(cx.feed.now - start),
            )
    } else {
        match job.exit_code {
            Some(code) => format!("exit {code}"),
            None => job.state.replace('_', " "),
        }
    };
    SideJob {
        test_id: format!("side-job-{}", job.id),
        job: job.id.clone(),
        mark,
        hue,
        command: job.command.lines().next().unwrap_or_default().to_owned(),
        state,
        action: Action::Sidebar(SidebarAction::Job {
            session: cx.id.clone(),
            job: job.id.clone(),
        }),
    }
}

fn read<'a>(cx: &Cx<'a>, target: &DetailRef) -> Option<&'a SessionContext> {
    match cx.feed.detail(target)?.body()? {
        DetailBody::SessionContext(context) => Some(context),
        _ => None,
    }
}

fn facts(cx: &Cx<'_>, context: Option<&SessionContext>) -> Vec<ContextFact> {
    let fact = |label, value: String, mono| ContextFact {
        label,
        value,
        mono,
        hue: Hue::Neutral,
    };
    let mut facts = Vec::new();
    if let Some(context) = context {
        facts.push(fact("Directory", context.cwd.clone(), true));
        match &context.worktree {
            Some(worktree) => {
                facts.push(fact("Worktree", worktree.clone(), true));
                facts.push(match &context.branch {
                    Some(branch) => fact("Branch", branch.clone(), true),
                    None => fact("Branch", "none checked out (detached)".into(), false),
                });
            }
            None => facts.push(fact("Worktree", "not in a Git repository".into(), false)),
        }
    }
    let pending = cx.state.and_then(|state| state.pending_model.clone());
    facts.push(match (cx.model(), pending) {
        (Some(model), Some(next)) => fact("Model", format!("{model} → {next} next turn"), true),
        (Some(model), None) => fact("Model", model, true),
        (None, _) => fact("Model", "not reported".into(), false),
    });
    facts.push(fact(
        "Effort",
        cx.effort().unwrap_or_else(|| "not reported".into()),
        false,
    ));
    let usage = cx
        .state
        .map(|state| &state.usage)
        .filter(|usage| usage.context_window > 0);
    facts.push(match usage {
        Some(usage) => {
            let percent = usage
                .context_percent
                .filter(|value| value.is_finite())
                .unwrap_or(usage.context_tokens as f32 * 100.0 / usage.context_window as f32)
                .clamp(0.0, 100.0);
            ContextFact {
                label: "Context",
                value: format!(
                    "{}% · {} / {}",
                    percent.round() as u32,
                    tokens(usage.context_tokens),
                    tokens(usage.context_window)
                ),
                mono: false,
                hue: if percent >= 95.0 {
                    Hue::Fail
                } else if percent >= 80.0 {
                    Hue::Held
                } else {
                    Hue::Neutral
                },
            }
        }
        None => fact("Context", "window not reported".into(), false),
    });
    facts
}

fn lists(context: &SessionContext) -> Vec<ContextList> {
    vec![
        ContextList {
            label: "Context files",
            items: context.context_files.clone(),
            empty: "none loaded",
        },
        ContextList {
            label: "Skills",
            items: context.skills.clone(),
            empty: "none loaded",
        },
        ContextList {
            label: "Prompts",
            items: context.prompts.clone(),
            empty: "none loaded",
        },
        ContextList {
            label: "Tools",
            items: context.tools.clone(),
            empty: "none reported",
        },
    ]
}

fn mcp(context: &SessionContext) -> Vec<McpRow> {
    context
        .mcp
        .iter()
        .map(|server| McpRow {
            name: server.name.clone(),
            state: server.state.replace('_', " "),
            hue: match server.state.as_str() {
                "connected" => Hue::Ok,
                "unavailable" | "failed" | "error" => Hue::Fail,
                _ => Hue::Held,
            },
        })
        .collect()
}

pub(super) fn context_tab(cx: &Cx<'_>, target: &DetailRef) -> ContextTab {
    let fleet = cx.live.and_then(|live| live.fleet.as_ref());
    let children = fleet
        .map(|fleet| fleet.children.as_slice())
        .unwrap_or_default();
    let agents: Vec<SideAgent> = children
        .iter()
        .filter_map(|child| child_mark(child).map(|mark| agent(cx, child, mark)))
        .collect();
    let completed: Vec<&str> = children
        .iter()
        .filter(|child| child_mark(child).is_none())
        .map(|child| child.id.as_str())
        .collect();
    let reported = cx.state.map_or(0, |state| state.children.total);
    let agents_note = if fleet.is_none() && reported > 0 {
        Some(format!(
            "{reported} {} · the host has not sent the agent list yet",
            if reported == 1 { "agent" } else { "agents" }
        ))
    } else if agents.is_empty() && !completed.is_empty() {
        Some("none at work".into())
    } else {
        None
    };
    let listed = cx.state.is_some_and(|state| state.jobs.total > 0);
    let served_jobs = jobs_read(cx);
    let jobs_note = match served_jobs {
        Some(served) if served.unavailable.is_some() => served
            .unavailable
            .as_ref()
            .map(|why| (Hue::Held, format!("commands unavailable: {why}"))),
        Some(_) => None,
        None if listed => {
            let cache = cx.feed.detail(&jobs_target(cx));
            Some(match cache.and_then(|cache| cache.error.as_ref()) {
                Some(error) => (Hue::Fail, format!("commands not read: {}", error.message)),
                None => (Hue::Neutral, "reading the commands…".into()),
            })
        }
        None => None,
    };
    let jobs: Vec<SideJob> = super::super::fleet::visible_commands(
        served_jobs.map_or(&[][..], |served| served.jobs.as_slice()),
    )
    .into_iter()
    .map(|row| job(cx, row))
    .collect();
    let empty =
        (children.is_empty() && jobs.is_empty() && agents_note.is_none() && jobs_note.is_none())
            .then(|| "no agents or commands yet".into());
    let context = read(cx, target);
    ContextTab {
        agents,
        agents_note,
        completed: (!completed.is_empty()).then(|| {
            (
                format!("Completed · {}", completed.len()),
                completed.join(" · "),
            )
        }),
        jobs,
        jobs_note,
        empty,
        facts: facts(cx, context),
        lists: context.map(lists).unwrap_or_default(),
        mcp: context.map(mcp),
    }
}

/// Opens a child in the Agents tab with it selected.
pub(super) fn open_agent(session: &crate::model::Id, child: String) -> [Action; 2] {
    [
        Action::Tab {
            session: session.clone(),
            tab: super::super::Tab::Agents,
        },
        Action::SelectAgent {
            session: session.clone(),
            child: Some(child),
        },
    ]
}

/// Opens a background job in the Agents tab with it selected.
pub(super) fn open_job(session: &crate::model::Id, job: String) -> [Action; 2] {
    [
        Action::Tab {
            session: session.clone(),
            tab: super::super::Tab::Agents,
        },
        Action::Fleet(FleetAction::SelectJob {
            session: session.clone(),
            job,
        }),
    ]
}
