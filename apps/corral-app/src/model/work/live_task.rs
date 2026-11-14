//! The selected daemon task in the inspector and on the phone's task page: its board row's
//! facts with the task detail corrald serves (owner revision, body, run, dependencies, holds,
//! artifacts, plan position, log tail), the detail's loading, stale and error states, and the
//! commands the task's state allows, each with this device's last command and its outcome.

use corral_protocol::detail::{DetailBody, DetailRef, TaskDetail, TaskHold};

use crate::model::act::{self, ActStatus};
use crate::model::board::{Actor, Mark, STAGES, daemon_actor, money, stage_word};
use crate::model::daemon::{self, Feed, Link};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::{Rich, span};
use crate::model::{Live, Selection};

use super::{
    Button, ButtonKind, Dependency, EMPTY_BOARD, GroupKey, Inspector, NO_BOARD, NowTab, Press,
    Progress, Stage, State, Tail, TailLine, TaskVerb, ago, daemon_selection, group_key, stamp,
    task_question, task_session, when,
};

/// The detail reference of a task.
pub fn task_ref(id: &str) -> DetailRef {
    DetailRef::Task { id: id.to_owned() }
}

/// The task detail corrald last served for `id`, when one has arrived.
pub fn loaded_task<'a>(feed: &'a Feed, id: &str) -> Option<&'a TaskDetail> {
    match feed.detail(&task_ref(id))?.body()? {
        DetailBody::Task(detail) => Some(detail),
        _ => None,
    }
}

/// A one-line state of a detail beside what it shows: loading, stale or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailNote {
    pub text: String,
    pub hue: Hue,
}

/// The detail's state for a task, or `None` while it is current.
pub fn detail_note(feed: &Feed, id: &str) -> Option<DetailNote> {
    let cache = feed.detail(&task_ref(id));
    let has_body = cache.and_then(|cache| cache.body()).is_some();
    let as_of = cache
        .and_then(|cache| cache.pages.last())
        .map(|page| stamp(&page.as_of, feed))
        .filter(|stamp| !stamp.is_empty());
    let note = |text: String, hue| Some(DetailNote { text, hue });
    if let Some(error) = cache.and_then(|cache| cache.error.as_ref()) {
        let why = if error.code == "not_found" {
            format!("corrald has no detail for {id}.")
        } else if error.message.is_empty() {
            format!("Task detail unavailable ({}).", error.code)
        } else {
            format!("Task detail unavailable: {}", error.message)
        };
        return note(
            match (has_body, as_of) {
                (true, Some(at)) => format!("{why} Showing the detail as of {at}."),
                _ => why,
            },
            Hue::Fail,
        );
    }
    if !has_body {
        return if feed.link == Link::Down {
            note(
                "Task detail unavailable while corrald is unreachable.".into(),
                Hue::Neutral,
            )
        } else {
            note("Loading task detail…".into(), Hue::Neutral)
        };
    }
    match cache {
        Some(cache) if cache.stale || feed.link == Link::Down => note(
            as_of.map_or_else(
                || "Task detail may be out of date.".to_owned(),
                |at| format!("Task detail as of {at} · may be out of date"),
            ),
            Hue::Held,
        ),
        _ => None,
    }
}

/// A labelled value of the live story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Text(Rich),
    /// Task ids that open the task.
    Tasks(Vec<Dependency>),
    /// The owner's control: answering the open question.
    Answer(Button),
}

/// This device's last command on the task, with its outcome as far as the device knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentLine {
    pub text: String,
    pub hue: Hue,
    /// Ask corrald for an unknown outcome under the same command id.
    pub recover: Option<Button>,
}

/// The selected daemon task's story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveStory {
    pub id: String,
    pub title: String,
    /// The heading of the group the task is in: who acts next.
    pub who: &'static str,
    pub who_hue: Hue,
    /// `· plan T-885`, opening the plan.
    pub plan: Option<(String, Press)>,
    /// The changed-since-visit chip, when the changes feed holds an unseen event for it.
    pub changed: Option<String>,
    /// The task's goal, or its canonical status where it has none.
    pub summary: Rich,
    /// The task body's canonical Markdown, exactly as corrald bounds it.
    pub body: Option<String>,
    /// Run, gate, review, land, with each stage's sha and age where the daemon knows them.
    pub stages: [Stage; 4],
    pub progress: Option<Progress>,
    /// The progress's own words, if corrald gives any.
    pub progress_note: Option<String>,
    /// Labelled facts in display order; facts corrald left out are absent.
    pub facts: Vec<(&'static str, Fact)>,
    /// Open session, Ask about this, Review candidate, and the task's legal commands.
    pub actions: Vec<Button>,
    pub note: Option<DetailNote>,
    pub sent: Option<SentLine>,
}

/// The selected daemon task's artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEvidence {
    pub title: String,
    /// Each artifact's title and its source reference with its media type.
    pub rows: Vec<(String, String)>,
    pub empty: Option<&'static str>,
    pub note: Option<DetailNote>,
}

/// The selected daemon task's run, session hand-off and log tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub description: &'static str,
    pub open: Option<Button>,
    /// The run's identity, profile and timing.
    pub run: Rich,
    pub tail: Option<Tail>,
    pub empty: Option<&'static str>,
    pub note: Option<DetailNote>,
}

/// Said by the Evidence tab when the task has no artifacts.
pub const NO_ARTIFACTS: &str = "No artifacts recorded for this task.";
/// Said by the Session tab when the task has no run.
pub const NO_RUN: &str = "No run for this task yet.";
/// Said where the log tail would be when corrald captured none.
pub const NO_TAIL: &str = "No log lines captured for this run.";

/// The inspector in the Daemon state, for the selected task on the board.
pub fn daemon_inspector(state: &State, feed: &Feed, selected: &Selection, form: Form) -> Inspector {
    if let Some(line) = feed.device.board_notice() {
        return Inspector::Quiet(line);
    }
    if !feed.board.complete {
        return match feed.link {
            Link::Down => Inspector::Quiet(NO_BOARD),
            Link::Connecting | Link::Up => Inspector::Loading,
        };
    }
    let Some(task) = daemon_selection(state, selected, &feed.board, form) else {
        return Inspector::Quiet(EMPTY_BOARD);
    };
    task_inspector(state, feed, task, state.now_tab, form)
}

/// One task's inspector tab, independent of which task the list selects (the phone page).
pub fn task_inspector(
    state: &State,
    feed: &Feed,
    task: &daemon::Task,
    tab: NowTab,
    form: Form,
) -> Inspector {
    match tab {
        NowTab::Story => Inspector::LiveStory(Box::new(live_story(state, feed, task, form))),
        NowTab::Evidence => Inspector::LiveEvidence(live_evidence(feed, task)),
        NowTab::Session => Inspector::LiveSession(live_session(feed, task)),
    }
}

/// A stage's detail: its candidate sha and how long ago it reached its state.
fn stage(task: &daemon::Task, index: usize, now: u64) -> Stage {
    let step = &task.rail[index];
    let active = step.mark == Mark::Active;
    let detail = [
        step.sha7.clone().unwrap_or_default(),
        daemon::age(step.at, now),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    Stage {
        label: STAGES[index],
        mark: step.mark,
        word: stage_word(step.mark),
        detail,
        active,
        hue: match (active, index, daemon_actor(task.group)) {
            (false, _, _) => Hue::Neutral,
            (true, 0, Actor::You) => Hue::You,
            (true, 0, _) => Hue::Worker,
            (true, _, _) => Hue::Corral,
        },
    }
}

/// `reason · since 18:40 · by owner`, leaving out what corrald does not record.
fn hold_text(hold: &TaskHold, feed: &Feed) -> Rich {
    let mut text = vec![
        span(
            hold.reason
                .clone()
                .unwrap_or_else(|| "no reason recorded".into()),
        )
        .hue(Hue::Held),
    ];
    if let Some(since) = hold.since.as_deref().map(|at| when(at, feed)) {
        text.push(span(format!(" · since {since}")).dim());
    }
    if let Some(by) = &hold.by {
        text.push(span(format!(" · by {by}")).dim());
    }
    text
}

/// The kinds of attention keys (`T-935/escalation/2026-…Z` reads `escalation`), comma-separated,
/// each once in first-seen order; a key without that shape is shown whole.
fn attention_kinds(keys: &[String]) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    for key in keys {
        let kind = key
            .split_once('/')
            .and_then(|(_, rest)| rest.split_once('/'))
            .map_or(key.as_str(), |(kind, _)| kind);
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds.join(", ")
}

/// The task's run line: `R2 · pi-sol · gpt-5 high · started 18:40`.
fn run_text(detail: Option<&TaskDetail>, task: &daemon::Task, feed: &Feed) -> Rich {
    if let Some(run) = detail.and_then(|detail| detail.run.as_ref()) {
        let mut text = vec![span(run.id.clone()).mono()];
        let model = [run.model.as_deref(), run.effort.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        for part in [run.profile.clone(), (!model.is_empty()).then_some(model)]
            .into_iter()
            .flatten()
        {
            text.push(span(format!(" · {part}")));
        }
        if let Some(started) = run.started_at.as_deref() {
            text.push(span(format!(" · started {}", when(started, feed))).dim());
        }
        if let Some(ended) = run.ended_at.as_deref() {
            text.push(span(format!(" · ended {}", when(ended, feed))).dim());
        }
        return text;
    }
    let Some(agent) = &task.agent else {
        return Vec::new();
    };
    let mut text = vec![span(agent.role.clone())];
    for part in [&agent.profile, &agent.runtime, &agent.machine]
        .into_iter()
        .flatten()
    {
        text.push(span(format!(" · {part}")));
    }
    text
}

/// The status of this device's last command on the task.
fn sent_line(state: &State, feed: &Feed, task: &str) -> Option<SentLine> {
    let sent = state.sent_for(task)?;
    let status = act::status(feed, &sent.key);
    let (words, hue) = match &status {
        None => ("Waiting to send".to_owned(), Hue::Neutral),
        Some(status) => (
            status.line(),
            match status {
                ActStatus::Done => Hue::Ok,
                ActStatus::Failed(_) | ActStatus::Refused(_) | ActStatus::Conflict(_) => Hue::Fail,
                ActStatus::Unknown => Hue::Held,
                _ => Hue::Neutral,
            },
        ),
    };
    Some(SentLine {
        text: format!("{} · {words}", sent.title),
        hue,
        recover: (status == Some(ActStatus::Unknown)).then(|| {
            Button::new(
                "Check result",
                Press::Recover(sent.key.clone()),
                ButtonKind::Quiet,
            )
            .enabled(feed.link == Link::Up)
        }),
    })
}

/// The commands the task's state allows: release a hold, hold a queued task, retry a failed
/// run. Each needs the loaded owner revision and a live link, and none is offered while the
/// task's previous command has no known result.
fn verbs(
    state: &State,
    feed: &Feed,
    task: &daemon::Task,
    detail: Option<&TaskDetail>,
) -> Vec<Button> {
    let Some(detail) = detail else {
        return Vec::new();
    };
    let mut verbs = Vec::new();
    if detail.hold.is_some() {
        verbs.push(TaskVerb::Release);
    } else if detail.park.is_none() && group_key(task.group) == GroupKey::Queued {
        verbs.push(TaskVerb::Hold);
    }
    if task.rail[0].mark == Mark::Fail && detail.hold.is_none() {
        verbs.push(TaskVerb::Retry {
            run: detail.run.as_ref().map(|run| run.id.clone()),
        });
    }
    let waiting = state
        .sent_for(&task.id)
        .and_then(|sent| act::status(feed, &sent.key))
        .is_some_and(|status| status.uncertain());
    verbs
        .into_iter()
        .map(|verb| {
            Button::new(
                verb.title(&task.id),
                Press::Run {
                    task: task.id.clone(),
                    verb,
                },
                ButtonKind::Ghost,
            )
            .enabled(feed.link == Link::Up && !waiting)
        })
        .collect()
}

/// Open session, Ask about this and Review candidate, in the mockup's order and wording; the
/// phone's shorter labels.
pub fn task_buttons(feed: &Feed, task: &daemon::Task, phone: bool) -> Vec<Button> {
    let mut buttons = Vec::new();
    if let Some(session) = task_session(feed, task) {
        buttons.push(Button::new(
            if phone { "Session" } else { "Open session" },
            Press::Open(Live::Session(session)),
            if phone {
                ButtonKind::Primary
            } else {
                ButtonKind::Ghost
            },
        ));
    }
    buttons.push(Button::new(
        "Ask about this",
        Press::Ask {
            about: Live::Task(task.id.clone()),
            rev: Some(task.rev),
        },
        ButtonKind::Ghost,
    ));
    // Only a recorded candidate can be reviewed: the run's head in the task detail, or a
    // candidate sha on the board's gate or review stage. A stage's mark alone says nothing.
    let candidate = loaded_task(feed, &task.id)
        .and_then(|detail| detail.run.as_ref()?.head.as_ref())
        .is_some()
        || task.rail[1..=2].iter().any(|step| step.sha7.is_some());
    if candidate {
        buttons.push(Button::new(
            if phone { "Review" } else { "Review candidate" },
            Press::Open(Live::Review(task.id.clone())),
            if phone {
                ButtonKind::Ghost
            } else {
                ButtonKind::Quiet
            },
        ));
    }
    buttons
}

/// Build the selected daemon task's story.
pub fn live_story(state: &State, feed: &Feed, task: &daemon::Task, form: Form) -> LiveStory {
    let detail = loaded_task(feed, &task.id);
    let key = group_key(task.group);
    let standing = super::standing(task, detail);
    let mut facts: Vec<(&'static str, Fact)> = Vec::new();

    let mut doing = vec![span(standing.now.clone())];
    let age = daemon::age(task.since, feed.now);
    if !age.is_empty() {
        doing.push(span(format!(" · for {age}")).dim());
    }
    facts.push(("Now", Fact::Text(doing)));

    let question = task_question(feed, &task.id);
    let asked = question.is_some();
    if let Some(question) = question {
        facts.push((
            "You",
            Fact::Answer(Button::new(
                format!("Answer {question}"),
                Press::Open(Live::Question(question)),
                ButtonKind::Primary,
            )),
        ));
    } else if let Some(next) = detail.and_then(|detail| detail.owner_next.clone()) {
        facts.push(("You", Fact::Text(vec![span(next).hue(Hue::You)])));
    } else if detail.is_some() && key != GroupKey::You {
        facts.push(("You", Fact::Text(vec![span("nothing needed").hue(Hue::Ok)])));
    }
    if task.questions != 0 && !asked {
        facts.push((
            "Questions",
            Fact::Text(vec![span(format!("{} open", task.questions)).hue(Hue::You)]),
        ));
    }

    if let Some(detail) = detail {
        let needs: Vec<Dependency> = detail
            .dependencies
            .iter()
            .map(|dependency| Dependency {
                id: dependency.id.clone(),
                mark: if dependency.satisfied {
                    Mark::Done
                } else {
                    Mark::Next
                },
                show_mark: true,
                action: Press::Open(Live::Task(dependency.id.as_str().into())),
            })
            .collect();
        facts.push((
            "Needs",
            if needs.is_empty() {
                Fact::Text(vec![span("nothing")])
            } else {
                Fact::Tasks(needs)
            },
        ));
        if let Some(hold) = &detail.hold {
            facts.push(("Held", Fact::Text(hold_text(hold, feed))));
        }
        if let Some(park) = &detail.park {
            facts.push(("Parked", Fact::Text(hold_text(park, feed))));
        }
        if let Some(position) = &detail.plan_context {
            facts.push((
                "Plan",
                Fact::Text(vec![
                    span(position.plan.clone()).mono(),
                    span(format!(
                        " · layer {} of {} · {}/{} landed",
                        position.layer, position.layers, position.landed, position.total
                    )),
                ]),
            ));
        }
        facts.push(("Status", Fact::Text(vec![span(detail.status.clone())])));
    } else if let Some(reason) = &task.held_reason {
        facts.push((
            "Held",
            Fact::Text(vec![span(reason.clone()).hue(Hue::Held)]),
        ));
    }

    let run = run_text(detail, task, feed);
    if !run.is_empty() {
        facts.push((
            if detail.and_then(|detail| detail.run.as_ref()).is_some() {
                "Run"
            } else {
                "Agent"
            },
            Fact::Text(run),
        ));
    }
    if let Some(project) = &task.project {
        facts.push(("Project", Fact::Text(vec![span(project.clone())])));
    }
    if task.cost.is_some() {
        let amount = span(money(task.cost.map(|cost| cost as f32)));
        let word = if task.cost_estimated {
            " estimated"
        } else {
            " reported"
        };
        facts.push(("Cost", Fact::Text(vec![amount, span(word).dim()])));
    }
    if !task.attention.is_empty() {
        facts.push((
            "Attention",
            Fact::Text(vec![span(attention_kinds(&task.attention))]),
        ));
    }

    let plan = task.plan.clone().or_else(|| {
        detail
            .and_then(|detail| detail.plan_context.as_ref())
            .map(|position| position.plan.clone())
    });
    let since = feed
        .seen
        .as_ref()
        .and_then(|seen| seen.at.as_deref())
        .map(|at| stamp(at, feed));
    let changed = super::unseen_tasks(feed)
        .contains(task.id.as_str())
        .then(|| match since {
            Some(since) if !since.is_empty() => format!("▲ changed since {since}"),
            _ => "▲ changed since your last visit".to_owned(),
        });
    let mut actions = if form == Form::Phone {
        Vec::new()
    } else {
        task_buttons(feed, task, false)
    };
    actions.extend(verbs(state, feed, task, detail));
    LiveStory {
        id: task.id.to_string(),
        title: detail.map_or_else(|| task.title.clone(), |detail| detail.title.clone()),
        who: standing.who,
        who_hue: standing.hue,
        plan: plan.map(|plan| {
            (
                format!("· plan {plan}"),
                Press::Open(Live::Plan(plan.as_str().into())),
            )
        }),
        changed,
        summary: task
            .goal
            .clone()
            .map(|goal| vec![span(goal)])
            .unwrap_or_default(),
        body: detail
            .map(|detail| detail.body_excerpt.clone())
            .filter(|body| !body.trim().is_empty()),
        stages: std::array::from_fn(|index| stage(task, index, feed.now)),
        progress: detail
            .and_then(|detail| detail.progress.as_ref())
            .map(|progress| {
                let (done, total) = (progress.completed as usize, progress.total as usize);
                Progress {
                    label: "Progress",
                    done,
                    total,
                    figure: format!("{done}/{total}"),
                }
            }),
        progress_note: detail
            .and_then(|detail| detail.progress.as_ref())
            .and_then(|progress| progress.text.clone()),
        facts,
        actions,
        note: detail_note(feed, &task.id),
        sent: sent_line(state, feed, &task.id),
    }
}

/// Build the selected daemon task's artifact list.
pub fn live_evidence(feed: &Feed, task: &daemon::Task) -> LiveEvidence {
    let detail = loaded_task(feed, &task.id);
    let rows: Vec<(String, String)> = detail
        .map(|detail| {
            detail
                .artifacts
                .iter()
                .map(|artifact| {
                    let source = match &artifact.media_type {
                        Some(kind) => format!("{} · {kind}", artifact.source_ref),
                        None => artifact.source_ref.clone(),
                    };
                    (artifact.title.clone(), source)
                })
                .collect()
        })
        .unwrap_or_default();
    LiveEvidence {
        title: format!("Evidence for {}", task.id),
        empty: (detail.is_some() && rows.is_empty()).then_some(NO_ARTIFACTS),
        rows,
        note: detail_note(feed, &task.id),
    }
}

/// The task's log tail as corrald captured it: read-only, labelled with its capture time.
pub fn task_tail(feed: &Feed, task: &daemon::Task) -> Option<Tail> {
    let detail = loaded_task(feed, &task.id)?;
    let agent = feed
        .agents
        .values()
        .find(|row| row.task.as_deref() == Some(task.id.as_str()));
    let as_of = feed
        .detail(&task_ref(&task.id))
        .and_then(|cache| cache.pages.last())
        .map(|page| ago(&page.as_of, feed))
        .filter(|age| !age.is_empty());
    let pane = agent
        .and_then(|row| row.pane.clone())
        .unwrap_or_else(|| "−".into());
    let runtime = agent
        .and_then(|row| row.runtime.clone())
        .or_else(|| task.agent.as_ref()?.runtime.clone())
        .unwrap_or_default();
    Some(Tail {
        id: task.id.to_string(),
        pane,
        runtime,
        label: as_of.map_or_else(|| "log tail".into(), |age| format!("log tail · {age} ago")),
        snapshot: true,
        illustrative: None,
        lines: detail.log_tail.iter().map(TailLine::new).collect(),
        cursor: false,
        action: task_session(feed, task).map(|session| Press::Open(Live::Session(session))),
    })
}

/// Build the selected daemon task's Session tab.
pub fn live_session(feed: &Feed, task: &daemon::Task) -> LiveSession {
    let detail = loaded_task(feed, &task.id);
    let tail = task_tail(feed, task).filter(|tail| !tail.lines.is_empty());
    let run = run_text(detail, task, feed);
    LiveSession {
        description: "Structured transcript in its session; below, the run's latest log lines as corrald captured them.",
        open: task_session(feed, task).map(|session| {
            Button::new(
                format!("Open {} session", task.id),
                Press::Open(Live::Session(session)),
                ButtonKind::Primary,
            )
        }),
        empty: if detail.is_some() && run.is_empty() {
            Some(NO_RUN)
        } else if detail.is_some() && tail.is_none() {
            Some(NO_TAIL)
        } else {
            None
        },
        run,
        tail,
        note: detail_note(feed, &task.id),
    }
}
