//! Inspector, selected live tail and phone task story content of the fixture preview, and the
//! owned descriptors (dependencies, tails, tiles) the Daemon state's builders share.

use crate::model::board::{Actor, Mark, STAGES, find, money, stage_word, task};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::{Destination, Form};
use crate::model::text::{Rich, span};

use super::{Action, Button, ButtonKind, NowTab, PM_TAIL, Press, QUESTION_TAIL, RUNNING, State};

/// One run/gate/review/land stage in a task's story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// Stage name, in rail order.
    pub label: &'static str,
    /// Shape to render beside the state word.
    pub mark: Mark,
    /// Story-specific state word, including `waits on you` for T-868's run.
    pub word: &'static str,
    /// Run/profile, candidate SHA or next-stage target.
    pub detail: String,
    /// Whether this is the active stage.
    pub active: bool,
    /// Active stage hue; inactive stages are neutral.
    pub hue: Hue,
}

/// A task dependency or downstream member with its board state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// Mono task id.
    pub id: String,
    /// The task's required/result mark, or the downstream run mark.
    pub mark: Mark,
    /// Needs shows marks; the source's Unblocks row shows ids only.
    pub show_mark: bool,
    /// Open this task when a renderer makes the id interactive.
    pub action: Press,
}

/// The progress heading's completed/total counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// Heading label.
    pub label: &'static str,
    /// Steps with a passed mark.
    pub done: usize,
    /// All steps in the task snapshot.
    pub total: usize,
    /// Complete completed/total figure shown after the heading.
    pub figure: String,
}

/// The owner's story row, either an answer control or a green nothing-needed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerNext {
    /// Open the real handover sheet.
    Answer(Button),
    /// No owner action is needed.
    Nothing {
        /// Source wording.
        label: &'static str,
        /// Passed/healthy state hue.
        hue: Hue,
    },
}

/// The entire Story inspector, also reused by the phone task page.
#[derive(Debug, Clone, PartialEq)]
pub struct Story {
    /// Task id at the top of the inspector.
    pub id: &'static str,
    /// Task title, never shortened by the model.
    pub title: &'static str,
    /// Optional plan id displayed alongside the task id.
    pub plan: Option<&'static str>,
    /// Dim plan attribution beside the task id, including its leading middle dot.
    pub plan_label: Option<String>,
    /// Optional plan-opening action.
    pub plan_action: Option<Press>,
    /// Changed-since-visit chip text, absent for unchanged and simple queued tasks.
    pub changed: Option<&'static str>,
    /// The task's full goal where the snapshot provides it.
    pub goal: Option<&'static str>,
    /// Agent, machine, illustrative attribution and start time for an active task.
    pub facts: Rich,
    /// Plan and now-phrase for a task without detailed steps.
    pub summary: Rich,
    /// Four-stage stepper; absent for simple queued task stories.
    pub stages: Option<[Stage; 4]>,
    /// Progress heading and numeric counts, absent for a simple story.
    pub progress: Option<Progress>,
    /// Ordered task steps with their state marks.
    pub steps: &'static [(Mark, &'static str)],
    /// Label beside the next-actor sentence.
    pub next_label: &'static str,
    /// Who acts next, with the actor bolded.
    pub next: Rich,
    /// Label beside the owner's action.
    pub owner_label: &'static str,
    /// Owner action or nothing-needed line.
    pub owner: OwnerNext,
    /// `Needs` for detailed stories, `Waits on` for simple ones.
    pub needs_label: &'static str,
    /// Prerequisites, with their actual source marks.
    pub needs: Vec<Dependency>,
    /// Simple tasks without dependencies wait on `capacity`.
    pub needs_fallback: Option<&'static str>,
    /// Label beside the downstream task ids.
    pub unblocks_label: &'static str,
    /// Downstream tasks; their marks are available but not shown by the source.
    pub unblocks: Vec<Dependency>,
    /// Empty downstream list marker.
    pub unblocks_fallback: &'static str,
    /// Label beside the cost and profile line.
    pub cost_label: &'static str,
    /// Cost and profile line, including the held-colour warning above $20.
    pub cost: Rich,
    /// Desktop actions; the phone page supplies its separate action row.
    pub actions: Vec<Button>,
}

/// Build a selected task's full or simple queued story and all its action wording.
pub fn story(state: &State, env: Env) -> Story {
    let t = task(state.sel);
    let detailed = !t.steps.is_empty();
    let is_you = t.actor == Actor::You;
    let facts = if detailed {
        match t.machine {
            Some(machine) => vec![
                span(format!("{} · on {machine} ", t.agent.unwrap_or(""))),
                span("machine name illustrative").dim(),
                span(format!(" · started {}", t.started.unwrap_or(""))),
            ],
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let mut summary = Vec::new();
    if !detailed {
        if let Some(plan) = t.plan {
            summary.push(span(format!("Plan {plan} · ")));
        }
        summary.push(span(t.now));
    }
    let stages = detailed.then(|| {
        std::array::from_fn(|index| {
            let mark = t.rail[index];
            let active = mark == Mark::Active;
            let word = if is_you && index == 0 {
                "waits on you"
            } else {
                stage_word(mark)
            };
            Stage {
                label: STAGES[index],
                mark,
                word,
                detail: match index {
                    0 => format!("{} · {}", t.run.unwrap_or("R1"), t.agent.unwrap_or("")),
                    1 => match t.id {
                        "T-890" => "1751bd2",
                        "T-894" => "6b36916",
                        "T-867" => "5949a4b",
                        "T-852" => "af7a6dc",
                        _ => "after run",
                    }
                    .into(),
                    2 => "after gate".into(),
                    _ => "main".into(),
                },
                active,
                hue: if !active {
                    Hue::Neutral
                } else if index == 0 {
                    if is_you { Hue::You } else { Hue::Worker }
                } else {
                    Hue::Corral
                },
            }
        })
    });
    let mut cost = Vec::new();
    if detailed {
        let amount = money(t.cost);
        cost.push(span(if amount.is_empty() {
            "−".into()
        } else {
            amount
        }));
        cost.push(span(format!(" · {}", t.agent.unwrap_or(""))));
        if t.hot() {
            cost.push(span(" · over the $20 warning").hue(Hue::Held));
        }
    }
    Story {
        id: t.id,
        title: t.title,
        plan: t.plan,
        plan_label: t.plan.map(|plan| format!("· plan {plan}")),
        plan_action: t.plan.map(|plan| Press::Work(Action::OpenPlan(plan))),
        changed: (detailed && t.changed).then_some("▲ changed since 18:40"),
        goal: t.goal,
        facts,
        summary,
        stages,
        progress: detailed.then(|| {
            let done = t
                .steps
                .iter()
                .filter(|(mark, _)| *mark == Mark::Done)
                .count();
            let total = t.steps.len();
            Progress {
                label: "Progress",
                done,
                total,
                figure: format!("{done}/{total}"),
            }
        }),
        steps: t.steps,
        next_label: "Next",
        owner_label: "You",
        unblocks_label: "Unblocks",
        cost_label: "Cost",
        next: t.next.map_or_else(Vec::new, |(who, what)| {
            vec![span(who).bold(), span(format!(" · {what}"))]
        }),
        owner: if detailed && is_you {
            OwnerNext::Answer(Button::new(
                "Answer T-868/R1/Q1",
                Action::OpenQuestion,
                ButtonKind::Primary,
            ))
        } else {
            OwnerNext::Nothing {
                label: "nothing needed",
                hue: Hue::Ok,
            }
        },
        needs_label: if detailed { "Needs" } else { "Waits on" },
        needs: t
            .needs
            .iter()
            .map(|(id, mark)| Dependency {
                id: (*id).to_owned(),
                mark: *mark,
                show_mark: true,
                action: Press::Work(Action::OpenTask(id)),
            })
            .collect(),
        needs_fallback: (!detailed && t.needs.is_empty()).then_some("capacity"),
        unblocks: t
            .unblocks
            .iter()
            .map(|id| Dependency {
                id: (*id).to_owned(),
                mark: find(id).map_or(Mark::Next, |task| task.rail[0]),
                show_mark: false,
                action: Press::Work(Action::OpenTask(id)),
            })
            .collect(),
        unblocks_fallback: "−",
        cost,
        actions: if detailed && env.form != Form::Phone {
            task_actions(t.id, false)
        } else {
            Vec::new()
        },
    }
}

fn task_actions(id: &'static str, phone: bool) -> Vec<Button> {
    vec![
        Button::new(
            if phone { "Session" } else { "Open session" },
            Action::OpenSession(id),
            if phone {
                ButtonKind::Primary
            } else {
                ButtonKind::Ghost
            },
        ),
        Button::new("Ask about this", Action::AskAbout(id), ButtonKind::Ghost),
        Button::new(
            if phone { "Review" } else { "Review candidate" },
            Action::Go(Destination::Review),
            if phone {
                ButtonKind::Ghost
            } else {
                ButtonKind::Quiet
            },
        ),
    ]
}

/// A single Evidence row with the source's neutral mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Artifact {
    /// Artifact label and its file kind, separated by a middle dot.
    pub label: &'static str,
    /// The neutral not-applicable mark.
    pub mark: Mark,
}

/// The selected task's illustrative Evidence inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    /// Heading with the selected task id.
    pub title: String,
    /// Artifact rows in source order.
    pub artifacts: &'static [Artifact],
    /// Attribution below the list.
    pub illustrative: &'static str,
}

/// Build the three illustrative artifacts for the selected task.
pub fn evidence(state: &State, _env: Env) -> Evidence {
    Evidence {
        title: format!("Evidence for {}", state.sel),
        artifacts: &[
            Artifact {
                label: "gate receipts · json",
                mark: Mark::Na,
            },
            Artifact {
                label: "checkpoint 3 · resume",
                mark: Mark::Na,
            },
            Artifact {
                label: "implementation report · md",
                mark: Mark::Na,
            },
        ],
        illustrative: "artifact list illustrative",
    }
}

/// The terminal syntax classes encoded by the mockup's tail helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailTone {
    /// Unstyled output.
    Plain,
    /// A shell command or Edit/Bash/Read tool line.
    Command,
    /// A passing test or `ok ` line.
    Ok,
}

impl TailTone {
    /// The tone of a terminal line, as the mockup's tail helper decides it.
    pub fn of(line: &str) -> TailTone {
        if line.contains("PASS") || line.contains("ok ") {
            TailTone::Ok
        } else if ["$", "Edit", "Bash", "Read"]
            .iter()
            .any(|prefix| line.starts_with(*prefix))
        {
            TailTone::Command
        } else {
            TailTone::Plain
        }
    }
}

/// A terminal tail line, with text unmodified and styling decided in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailLine {
    /// Exact terminal text, including indentation.
    pub text: String,
    /// Semantic style for this line.
    pub tone: TailTone,
}

impl TailLine {
    pub fn new(text: impl Into<String>) -> TailLine {
        let text = text.into();
        TailLine {
            tone: TailTone::of(&text),
            text,
        }
    }
}

/// A task's terminal tail: a live stream in the fixture, a read-only capture from corrald.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tail {
    /// Task or PM identity.
    pub id: String,
    /// Pane at the top of the tail; `−` if absent.
    pub pane: String,
    /// The runtime name.
    pub runtime: String,
    /// `live`, or a capture's label with its time.
    pub label: String,
    /// Whether this tail is a capture rather than a stream.
    pub snapshot: bool,
    /// Attribution shown above the terminal, in the fixture preview.
    pub illustrative: Option<&'static str>,
    /// Terminal output in source order.
    pub lines: Vec<TailLine>,
    /// Draw the source's final cursor cell after the output: only a live stream has one.
    pub cursor: bool,
    /// Open the full session for this tail.
    pub action: Option<Press>,
}

/// Build a task's tail, including T-868's agent-snapshot fallback and legacy labelling.
pub fn live_tail(id: &'static str, _env: Env) -> Tail {
    tail_with_limit(id, usize::MAX)
}

fn tail_with_limit(id: &'static str, limit: usize) -> Tail {
    let t = task(id);
    let snapshot = id == "T-854";
    let lines = if id == "PM" {
        PM_TAIL
    } else if !t.tail.is_empty() {
        t.tail
    } else if id == "T-868" {
        QUESTION_TAIL
    } else {
        &["no live pane for this item"]
    };
    Tail {
        id: id.to_owned(),
        pane: if id == "PM" {
            "pm:p0"
        } else {
            t.pane.unwrap_or("−")
        }
        .to_owned(),
        runtime: if snapshot {
            "Herdr TUI (legacy)"
        } else {
            "herdr-core"
        }
        .to_owned(),
        label: if snapshot {
            "snapshot · 20s ago"
        } else {
            "live"
        }
        .to_owned(),
        snapshot,
        illustrative: Some("tail illustrative"),
        lines: lines[lines.len().saturating_sub(limit)..]
            .iter()
            .map(|line| TailLine::new(*line))
            .collect(),
        cursor: true,
        action: Some(Press::Work(Action::OpenSession(id))),
    }
}

/// The Session inspector's handoff to the full transcript and its terminal preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Source explanation above the session button.
    pub description: &'static str,
    /// Button opening the selected task's full session.
    pub open: Button,
    /// Available live/snapshot tail; desktop Now also displays it in the Live context dock.
    pub tail: Tail,
}

/// Build the selected session inspector, without importing the agents domain.
pub fn session(state: &State, env: Env) -> Session {
    Session {
        description: "Structured transcript with the raw terminal one tap away.",
        open: Button::new(
            format!("Open {} session", state.sel),
            Action::OpenSession(state.sel),
            ButtonKind::Primary,
        ),
        tail: live_tail(state.sel, env),
    }
}

/// Which selected-inspector content the renderer should show.
#[derive(Debug, Clone, PartialEq)]
pub enum Inspector {
    /// Projection detail is still loading; show three placeholders.
    Loading,
    /// Task story.
    Story(Story),
    /// Illustrative evidence list.
    Evidence(Evidence),
    /// Session handoff.
    Session(Session),
    /// The selected daemon task's story.
    LiveStory(Box<super::LiveStory>),
    /// The selected daemon task's artifacts.
    LiveEvidence(super::LiveEvidence),
    /// The selected daemon task's run and log tail.
    LiveSession(super::LiveSession),
    /// One quiet line in place of content: no board, nothing on it, an unenrolled device.
    Quiet(&'static str),
}

/// Resolve the selected inspector tab and its loading branch.
pub fn inspector(state: &State, env: Env) -> Inspector {
    if env.mode == crate::model::effect::DataMode::Loading {
        return Inspector::Loading;
    }
    match state.now_tab {
        NowTab::Story => Inspector::Story(story(state, env)),
        NowTab::Evidence => Inspector::Evidence(evidence(state, env)),
        NowTab::Session => Inspector::Session(session(state, env)),
    }
}

/// A plan graph handoff: the Plans renderer resolves the plan and highlights this member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanContext {
    /// Plan id when the selected task belongs to a fixture plan.
    pub plan: Option<&'static str>,
    /// Selected/highlighted task id.
    pub member: &'static str,
    /// Heading `Plan <id>`, or `Plan ` on a planless phone task page.
    pub heading: String,
    /// Explanation shown instead of a graph when there is no plan.
    pub empty: Option<&'static str>,
    /// Open the corresponding plan page.
    pub action: Option<Press>,
}

/// Resolve graph ids without copying plan data into the work domain.
pub fn plan_context(state: &State, _env: Env) -> PlanContext {
    let plan = task(state.sel).plan;
    PlanContext {
        plan,
        member: state.sel,
        heading: format!("Plan {}", plan.unwrap_or("")),
        empty: plan.is_none().then_some("Not part of a plan."),
        action: plan.map(|plan| Press::Work(Action::OpenPlan(plan))),
    }
}

/// A running-agent tile of the expanded Live overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTile {
    /// Agent identity, distinct from a gate's task id.
    pub id: String,
    /// Agent role label.
    pub role: String,
    /// The current action line.
    pub doing: String,
    /// The doing line's hue: needs-you when the agent waits on the owner.
    pub doing_hue: Hue,
    /// Profile or gate script; empty when unknown.
    pub profile: String,
    /// Runtime name; empty when unknown.
    pub runtime: String,
    /// Pane identity; `−` when the agent has none.
    pub pane: String,
    /// Source state-chip wording, with asking displayed as needs you.
    pub chip: &'static str,
    /// State hue of the tile's chip.
    pub chip_hue: Hue,
    /// Agent title, not its current doing-now line.
    pub title: String,
    /// Footer clock with the source's since prefix; empty when unknown.
    pub since: String,
    /// Last terminal lines with the live/snapshot label.
    pub tail: Tail,
    /// Open this agent's session.
    pub action: Option<Press>,
}

/// Selected live tail plus the running-agent overview tiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDock {
    /// The actual Now Live dock content.
    pub selected: Tail,
    /// Current PM, worker and gate tiles, retaining their distinct agent identities.
    pub tiles: Vec<LiveTile>,
}

/// Build the selected Live dock content and every current running tile.
pub fn live(state: &State, env: Env) -> LiveDock {
    LiveDock {
        selected: live_tail(state.sel, env),
        tiles: RUNNING
            .iter()
            .map(|running| {
                let mut tail = tail_with_limit(running.session, 4);
                running.pane.clone_into(&mut tail.pane);
                LiveTile {
                    id: running.id.to_owned(),
                    role: running.role.to_owned(),
                    doing: running.doing.to_owned(),
                    doing_hue: running.hue,
                    profile: running.profile.to_owned(),
                    runtime: running.runtime.to_owned(),
                    pane: running.pane.to_owned(),
                    chip: if running.state == "asking" {
                        "needs you"
                    } else {
                        running.state
                    },
                    chip_hue: match running.state {
                        "live" | "gating" => Hue::Corral,
                        "working" => Hue::Worker,
                        "asking" => Hue::You,
                        _ => Hue::Neutral,
                    },
                    title: match running.id {
                        "PM" => "supervisor · generation 14",
                        "gate-T-890" => "quick gate 1751bd2",
                        "gate-T-894" => "gate 6b36916",
                        _ => task(running.session).title,
                    }
                    .to_owned(),
                    since: format!("since {}", running.since),
                    tail,
                    action: Some(Press::Work(Action::OpenSession(running.session))),
                }
            })
            .collect(),
    }
}

/// The pushed phone task page's complete content, in story/actions/live/plan order.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskPage {
    /// Navigation title is the selected task id.
    pub title: &'static str,
    /// Back label beside the navigation chevron.
    pub back_label: &'static str,
    /// Pop the phone task page.
    pub back: Action,
    /// Task story; the full goal remains available even if not shown by the mockup.
    pub story: Story,
    /// Session, Ask about this and Review controls.
    pub actions: Vec<Button>,
    /// Heading before the terminal preview.
    pub live_label: &'static str,
    /// Heading's illustrative attribution.
    pub live_illustrative: &'static str,
    /// Live/snapshot terminal preview.
    pub tail: Tail,
    /// Plan graph ids and empty state.
    pub plan: PlanContext,
    /// Connection/stale projection shared with Now.
    pub status: super::Status,
}

/// Build the phone task page independently of the caller's presentation form.
pub fn task_page(state: &State, mut env: Env) -> TaskPage {
    env.form = Form::Phone;
    TaskPage {
        title: state.sel,
        back_label: "Now",
        back: Action::Back,
        story: story(state, env),
        actions: task_actions(state.sel, true),
        live_label: "Live",
        live_illustrative: "illustrative",
        tail: live_tail(state.sel, env),
        plan: plan_context(state, env),
        status: super::status(env),
    }
}
