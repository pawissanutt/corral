//! The served question's handover: header, S B A R, thread, history and decide bar built
//! from `QuestionDetail`, the inbox row and this device's sent commands. Fields corrald does
//! not record are left out, never filled in.

use corral_protocol::detail::{QuestionDetail, WaitKind as Waiting};
use corral_protocol::rows::{InboxKind, InboxRow, InboxState};

use super::live::{
    Draft, age, can_answer, choice, clock, is_open, ordered, question_detail, question_ref,
    question_state, sent_status,
};
use super::live_notice::{followup_status, situation_notice};
use super::sheet::{band, button, desk_bands, key_help, phone_bands};
use super::{
    Action, AnswerOption, Assessment, Background, ButtonKind, Chip, DecideBar, EmptyState,
    Handover, Header, HistoryRow, Notice, QuestionState, Recommendation, Sheet, Situation, State,
    ThreadLine, ThreadView, WaitKind, WaitLine, live_evidence, phone,
};
use crate::model::daemon::{Feed, Link};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Destination;
use crate::model::text::{Rich, span};
use crate::model::{Id, Live};

/// Whether the handover of `key` can be drawn.
pub fn handover(feed: &Feed, key: &Id) -> Handover {
    let Some(cache) = feed.detail(&question_ref(key)) else {
        return Handover::Loading;
    };
    if question_detail(feed, key).is_some() {
        return Handover::Ready;
    }
    if cache.body().is_some() {
        return Handover::Failed(Notice {
            stamp: None,
            hue: Hue::Fail,
            body: vec![
                span("This app cannot show this question.").bold(),
                span(format!(
                    " corrald served {key} in a form this version does not know."
                )),
            ],
            retry: None,
        });
    }
    match &cache.error {
        Some(error) if error.code == "not_found" => Handover::Gone(EmptyState {
            title: "No longer waiting",
            body: format!(
                "corrald no longer serves {key}: it was answered, cancelled or withdrawn. What happened stays in Changes."
            ),
            action: Some(button(
                "Back to Inbox",
                Action::Go(Destination::Inbox),
                ButtonKind::Ghost,
            )),
        }),
        Some(error) => Handover::Failed(Notice {
            stamp: None,
            hue: Hue::Fail,
            body: vec![
                span(format!("Couldn't load {key}.")).bold(),
                span(" corrald returned "),
                span(error.code.clone()).mono(),
                span(format!(": {}", error.message)),
            ],
            retry: Some(button(
                "Retry",
                Action::Refetch(key.clone()),
                ButtonKind::Quiet,
            )),
        }),
        None => Handover::Loading,
    }
}

/// Nothing to show inline: no question is selected.
pub fn gone() -> EmptyState {
    EmptyState {
        title: "Nothing selected",
        body: "Choose an item in the list to see it here.".into(),
        action: None,
    }
}

/// A sheet with no content, made only while a handover leaves the screen.
pub fn placeholder() -> Sheet {
    Sheet {
        title: "Question",
        header: Header {
            chips: Vec::new(),
            record: String::new(),
            attribution: String::new(),
            title: String::new(),
        },
        state: QuestionState::Open,
        answered: false,
        bands: Vec::new(),
        side_bands: Vec::new(),
        situation: Situation {
            text: String::new(),
            deadline: None,
            waits_title: "Waiting on this answer",
            waits: Vec::new(),
            waits_empty: None,
            notice: None,
        },
        background: Background::Loading {
            skeleton_count: 3,
            note: "Loading the question from corrald.",
        },
        assessment: Assessment::Loading {
            skeleton_count: 1,
            note: "Loading the question from corrald.",
        },
        recommendation: Recommendation {
            label: "Options",
            options: Vec::new(),
            locked: true,
            draft_mark: "✎",
            draft_placeholder: "Write a different answer…",
            draft_key: None,
            reason: None,
        },
        thread: ThreadView {
            title: String::new(),
            messages: Vec::new(),
            empty: None,
            reply_placeholder: String::new(),
            reply_label: "Follow-up to the asker",
            reply_key: None,
            reply_enabled: false,
            send: disabled(button("Ask", Action::CloseThread, ButtonKind::Ghost)),
            attribution: None,
            status: None,
            done: button("Done", Action::CloseThread, ButtonKind::Quiet),
        },
        thread_preview: Vec::new(),
        thread_action: Action::Thread,
        thread_open: false,
        history: Vec::new(),
        history_empty: None,
        decide: DecideBar {
            buttons: Vec::new(),
            stamp: None,
            note: Vec::new(),
            submit: None,
        },
        back: Some(button(
            "Inbox",
            Action::Go(Destination::Inbox),
            ButtonKind::Quiet,
        )),
        pager: None,
        next: None,
        key_help: None,
        ask: None,
    }
}

fn disabled(mut control: super::Button) -> super::Button {
    control.enabled = false;
    control
}

/// The asker as attribution names it: role, then session or task.
fn asker(detail: &QuestionDetail) -> String {
    let who = detail.from.session.as_ref().or(detail.task.as_ref());
    match who {
        Some(who) => format!("{} {who}", detail.from.role),
        None => detail.from.role.clone(),
    }
}

fn kind_label(kind: InboxKind) -> &'static str {
    match kind {
        InboxKind::WorkerQuestion => "worker question",
        InboxKind::PmAsk => "PM question",
        InboxKind::AssistantAsk => "assistant question",
        InboxKind::Escalated => "escalated",
        _ => "question",
    }
}

fn header(detail: &QuestionDetail, row: Option<&InboxRow>, feed: &Feed, env: Env) -> Header {
    let mut chips = vec![match &detail.state {
        InboxState::Answered { .. } => Chip {
            label: "answered",
            hue: Hue::Ok,
        },
        InboxState::Cancelled => Chip {
            label: "cancelled",
            hue: Hue::Neutral,
        },
        InboxState::Expired => Chip {
            label: "expired",
            hue: Hue::Neutral,
        },
        _ => Chip {
            label: "needs you",
            hue: Hue::You,
        },
    }];
    if !phone(env) {
        chips.push(Chip {
            label: kind_label(detail.kind),
            hue: Hue::Neutral,
        });
    }
    if detail.blocking {
        chips.push(Chip {
            label: "blocking",
            hue: Hue::Fail,
        });
    }
    let profile = detail
        .from
        .profile
        .as_ref()
        .map(|profile| format!(" on {profile}"))
        .unwrap_or_default();
    let ago = age(Some(detail.asked_at.as_str()), feed);
    let when = if ago.is_empty() {
        format!("asked {}", clock(&detail.asked_at, feed.now))
    } else {
        format!("{ago} ago")
    };
    let needed = detail
        .needed_by
        .as_ref()
        .map(|at| format!(" · needed by {}", clock(at, feed.now)))
        .unwrap_or_default();
    let title = row.map(|row| row.title.clone()).unwrap_or_else(|| {
        let first = detail.text.lines().next().unwrap_or_default();
        first.chars().take(120).collect()
    });
    Header {
        chips,
        record: format!("{} · rev {}", detail.key, detail.rev),
        attribution: format!("from {}{profile} · {when}{needed}", asker(detail)),
        title,
    }
}

fn situation(
    state: &State,
    feed: &Feed,
    detail: &QuestionDetail,
    row: Option<&InboxRow>,
    status: QuestionState,
) -> Situation {
    Situation {
        text: detail.text.clone(),
        deadline: detail.deadline.as_ref().map(|deadline| {
            vec![
                span(format!("Deadline {}: ", clock(&deadline.at, feed.now))).bold(),
                span(deadline.instruction.clone()),
            ]
        }),
        waits_title: "Waiting on this answer",
        waits: detail
            .place
            .waiting
            .iter()
            .map(|wait| WaitLine {
                id: wait.id.clone(),
                text: wait.title.clone(),
                kind: match wait.kind {
                    Waiting::You => WaitKind::You,
                    Waiting::Plan => WaitKind::Plan,
                    Waiting::Info | Waiting::Other => WaitKind::Info,
                },
            })
            .collect(),
        waits_empty: detail
            .place
            .waiting
            .is_empty()
            .then_some("Nothing is recorded as waiting on this answer."),
        notice: situation_notice(state, feed, detail, row, status),
    }
}

fn background(detail: &QuestionDetail, feed: &Feed) -> Background {
    let task = detail.task.as_deref();
    let board = task.and_then(|task| feed.board.task(task));
    let mut goal: Rich = Vec::new();
    if let Some(task) = task {
        let name = match board {
            Some(row) => format!("{task} {}", row.title),
            None => task.to_owned(),
        };
        goal.push(span(name).bold().live(Live::Task(Id::new(task))));
    }
    let text = detail
        .place
        .goal
        .clone()
        .or_else(|| board.and_then(|row| row.goal.clone()));
    if let Some(text) = text {
        if !goal.is_empty() {
            goal.push(span(" · "));
        }
        goal.push(span(text));
    }
    let mut plan_line: Rich = Vec::new();
    if let Some(position) = &detail.place.position {
        plan_line.extend([
            span("Plan "),
            span(position.plan.clone())
                .mono()
                .live(Live::Plan(Id::new(&position.plan))),
            span(format!(
                " · layer {} of {} · {} of {} landed",
                position.layer, position.layers, position.landed, position.total
            )),
        ]);
    } else if let Some(plan) = &detail.plan {
        plan_line.extend([
            span("Plan "),
            span(plan.clone()).mono().live(Live::Plan(Id::new(plan))),
        ]);
    }
    if goal.is_empty() && plan_line.is_empty() {
        return Background::Absent {
            note: "No task or plan is recorded for this question.",
        };
    }
    Background::Ready {
        goal,
        plan_line,
        graph: None,
    }
}

fn recommendation(
    key: &Id,
    draft: Option<&Draft>,
    detail: &QuestionDetail,
    locked: bool,
) -> Recommendation {
    let chosen = match &detail.answer {
        Some(answer) => answer.option.as_ref().and_then(|label| {
            detail
                .options
                .iter()
                .position(|option| &option.label == label)
        }),
        None => choice(draft, detail),
    };
    let role = detail.from.role.clone();
    let recommended = detail.options.iter().position(|option| option.recommended);
    Recommendation {
        label: "Options",
        options: detail
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let unblocks = option.unblocks.join(", ");
                let consequences = [
                    ("Then", option.next.clone()),
                    ("Unblocks", (!unblocks.is_empty()).then_some(unblocks)),
                    ("Costs", option.cost.clone()),
                ]
                .into_iter()
                .filter_map(|(label, value)| value.map(|value| (label, value)))
                .collect();
                AnswerOption {
                    number: index + 1,
                    label: option.label.clone(),
                    recommendation_chip: option.recommended.then(|| format!("{role} recommends")),
                    selected: chosen == Some(index),
                    enabled: !locked,
                    action: Action::Pick(key.clone(), index),
                    consequences,
                    proof: option
                        .evidence
                        .and_then(|at| detail.evidence.get(at).map(|block| (at, block)))
                        .map(|(at, block)| {
                            button(
                                format!("see {}", live_evidence::label(at, block)),
                                Action::Proof(key.clone(), at),
                                ButtonKind::Quiet,
                            )
                        }),
                }
            })
            .collect(),
        locked,
        draft_mark: "✎",
        draft_placeholder: "Write a different answer…",
        draft_key: Some(key.clone()),
        reason: detail.recommendation.as_ref().map(|text| {
            let lead = match recommended {
                Some(index) => format!("Why the {role} recommends {}:", index + 1),
                None => "Recommendation:".into(),
            };
            vec![span(lead).bold(), span(" "), span(text.clone()).dim()]
        }),
    }
}

fn thread(
    state: &State,
    feed: &Feed,
    key: &Id,
    detail: &QuestionDetail,
    status: QuestionState,
) -> ThreadView {
    let who = asker(detail);
    let draft = state.desk.draft(key);
    let busy = sent_status(feed, draft.and_then(|draft| draft.followup.as_ref()))
        .is_some_and(|status| !status.settled());
    let enabled = matches!(status, QuestionState::Open | QuestionState::Pending)
        && can_answer(feed, None)
        && !busy;
    let mut send = button("Ask", Action::Followup(key.clone()), ButtonKind::Ghost);
    send.enabled = enabled;
    ThreadView {
        title: format!("Thread · {who}"),
        messages: detail
            .thread
            .iter()
            .map(|message| ThreadLine {
                who: message.by.clone(),
                via: Some(message.via.clone()),
                at: clock(&message.at, feed.now),
                text: message.text.clone(),
            })
            .collect(),
        empty: detail
            .thread
            .is_empty()
            .then_some("No follow-ups yet. Ask before answering; replies arrive here."),
        reply_placeholder: format!("Ask {who} before answering…"),
        reply_label: "Follow-up to the asker",
        reply_key: Some(key.clone()),
        reply_enabled: enabled,
        send,
        attribution: None,
        status: followup_status(feed, detail, draft),
        done: button("Done", Action::CloseThread, ButtonKind::Quiet),
    }
}

fn history(detail: &QuestionDetail, feed: &Feed) -> Vec<HistoryRow> {
    detail
        .history
        .iter()
        .map(|entry| {
            let owner = matches!(entry.by.as_str(), "owner" | "you");
            let mut label = clock(&entry.at, feed.now);
            if let Some(source) = &entry.source_ref {
                label.push_str(&format!(" · {source}"));
            }
            label.push_str(if owner {
                " · your decision"
            } else {
                " · tried"
            });
            HistoryRow {
                label,
                owner_decision: owner,
                text: entry.text.clone(),
            }
        })
        .collect()
}

/// Open rows other than `key`, in working order, and `key`'s place among the open ones.
fn neighbours<'a>(feed: &'a Feed, key: &Id) -> (Option<usize>, Vec<&'a InboxRow>) {
    let open: Vec<&InboxRow> = ordered(feed)
        .into_iter()
        .filter(|row| is_open(row))
        .collect();
    let at = open.iter().position(|row| row.id == key.as_str());
    let rest = match at {
        Some(at) => open[at + 1..].iter().chain(&open[..at]).copied().collect(),
        None => open,
    };
    (at, rest)
}

fn pager(feed: &Feed, key: &Id) -> String {
    let (at, rest) = neighbours(feed, key);
    match at {
        Some(at) => format!("{} of {} needing you", at + 1, rest.len() + 1),
        None if rest.is_empty() => "Nothing else needs you".into(),
        None => format!("{} needing you", rest.len()),
    }
}

fn next(feed: &Feed, key: &Id, env: Env, kind: ButtonKind) -> Option<super::Button> {
    let (_, rest) = neighbours(feed, key);
    let row = rest.first()?;
    let label = if phone(env) && kind == ButtonKind::Ghost {
        "Next".to_owned()
    } else {
        match row.kind {
            InboxKind::PlanAcceptance => {
                format!("Next · {} plan", row.plan.as_deref().unwrap_or(&row.id))
            }
            _ => format!("Next · {}", row.id),
        }
    };
    Some(button(label, Action::Next(key.clone()), kind))
}

fn option_keys(count: usize) -> Option<String> {
    match count {
        0 => None,
        1..=3 => Some(
            (1..=count)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => Some(format!("1–{}", count.min(9))),
    }
}

/// The served handover of `key`. `focus` adds the page head controls; `with_side` moves B,
/// Thread and History to the desktop context column; the phone's focus page has its own order.
pub fn sheet(
    state: &State,
    feed: &Feed,
    key: &Id,
    env: Env,
    focus: bool,
    with_side: bool,
) -> Sheet {
    let Some(detail) = question_detail(feed, key) else {
        return placeholder();
    };
    let row = feed.inbox.get(key.as_str());
    let status = question_state(state, feed, detail);
    let draft = state.desk.draft(key);
    let locked = status != QuestionState::Open || row.is_some_and(|row| !row.answerable);
    let thread = thread(state, feed, key, detail, status);
    let thread_preview = match detail.thread.last() {
        Some(message) => vec![
            span(format!("{}:", message.by)).bold(),
            span(format!(" {}", message.text)),
        ],
        None => vec![span("No follow-ups yet.").dim()],
    };
    let mut sheet = Sheet {
        title: "Question",
        header: header(detail, row, feed, env),
        state: status,
        answered: matches!(
            status,
            QuestionState::Mine | QuestionState::Elsewhere | QuestionState::Closed
        ),
        bands: Vec::new(),
        side_bands: Vec::new(),
        situation: situation(state, feed, detail, row, status),
        background: background(detail, feed),
        assessment: live_evidence::assessment(key, draft, detail),
        recommendation: recommendation(key, draft, detail, locked),
        thread,
        thread_preview,
        thread_action: Action::Thread,
        thread_open: state.thread_open,
        history: history(detail, feed),
        history_empty: detail
            .history
            .is_empty()
            .then_some("No earlier attempts or decisions are recorded."),
        decide: decide(state, feed, key, env),
        back: focus.then(|| button("Inbox", Action::Go(Destination::Inbox), ButtonKind::Quiet)),
        pager: focus.then(|| pager(feed, key)),
        next: if focus {
            next(feed, key, env, ButtonKind::Ghost)
        } else {
            None
        },
        key_help: if focus && !phone(env) {
            option_keys(detail.options.len()).map(|keys| key_help(&keys))
        } else {
            None
        },
        ask: focus.then(|| {
            button(
                if phone(env) {
                    "Ask the assistant about this"
                } else {
                    "Ask about this"
                },
                Action::AskAbout(key.clone(), false),
                ButtonKind::Ghost,
            )
        }),
    };
    if focus && phone(env) {
        phone_bands(&mut sheet);
    } else {
        desk_bands(&mut sheet, with_side, detail.plan.clone());
        if with_side && detail.history.is_empty() {
            if let Some(side) = sheet.side_bands.last_mut() {
                *side = band(super::BandKind::History, "H", "History", None);
            }
        }
    }
    sheet
}

/// The decide bar: answer controls while open, the canonical stamp once settled.
pub fn decide(state: &State, feed: &Feed, key: &Id, env: Env) -> DecideBar {
    let Some(detail) = question_detail(feed, key) else {
        return placeholder().decide;
    };
    let row = feed.inbox.get(key.as_str());
    let status = question_state(state, feed, detail);
    let session = feed.session.as_ref();
    let via = session.map(|session| format!("app:{}", session.device.name));
    if matches!(
        status,
        QuestionState::Mine | QuestionState::Elsewhere | QuestionState::Closed
    ) {
        let next = next(feed, key, env, ButtonKind::Primary);
        if phone(env) {
            let mut buttons = vec![button(
                "Back to Inbox",
                Action::Go(Destination::Inbox),
                ButtonKind::Ghost,
            )];
            buttons.extend(next);
            return DecideBar {
                buttons,
                stamp: None,
                note: Vec::new(),
                submit: None,
            };
        }
        let watch = match (&detail.from.session, &detail.task) {
            (Some(session), _) => Some(button(
                format!("Watch {} resume", detail.task.as_deref().unwrap_or(session)),
                Action::Live(Live::Session(Id::new(session))),
                ButtonKind::Ghost,
            )),
            (None, Some(task)) => Some(button(
                format!("Open {task}"),
                Action::Live(Live::Task(Id::new(task))),
                ButtonKind::Ghost,
            )),
            (None, None) => None,
        };
        let (at, by_via) = match (&detail.answer, row.map(|row| &row.state)) {
            (Some(answer), _) => (Some(answer.at.as_str()), Some(answer.via.as_str())),
            (None, Some(InboxState::Answered { via, at, .. })) => {
                (Some(at.as_str()), Some(via.as_str()))
            }
            _ => (None, None),
        };
        let at = at
            .map(|at| format!(" {}", clock(at, feed.now)))
            .unwrap_or_default();
        let stamp = match status {
            QuestionState::Mine => (format!("ANSWERED{at}"), Hue::Ok),
            QuestionState::Elsewhere => (
                match by_via {
                    Some(by_via) => format!("ANSWERED VIA {}{at}", by_via.to_uppercase()),
                    None => format!("ANSWERED{at}"),
                },
                Hue::Ok,
            ),
            _ => (
                if matches!(detail.state, InboxState::Expired) {
                    "EXPIRED".into()
                } else {
                    "CANCELLED".into()
                },
                Hue::Neutral,
            ),
        };
        return DecideBar {
            buttons: watch.into_iter().collect(),
            stamp: Some(stamp),
            note: vec![span(
                "One answer wins: the PM pane, this app and every paired device read the same question record.",
            )],
            submit: None,
        };
    }
    let draft = state.desk.draft(key);
    let written = draft.is_some_and(|draft| !draft.text.trim().is_empty());
    let chosen = choice(draft, detail);
    let target = match (written, chosen) {
        (true, _) => "your text".to_owned(),
        (false, Some(index)) => format!("option {}", index + 1),
        (false, None) => "choose an option".to_owned(),
    };
    let ready =
        status == QuestionState::Open && can_answer(feed, row) && (written || chosen.is_some());
    let mut answer = button(
        format!(
            "{} · {target}",
            if status == QuestionState::Pending {
                "Sending"
            } else {
                "Answer"
            }
        ),
        Action::Submit(key.clone()),
        ButtonKind::Primary,
    );
    answer.enabled = ready;
    let submit = ready.then(|| Action::Submit(key.clone()));
    let role = detail.from.role.clone();
    if phone(env) {
        return DecideBar {
            buttons: vec![
                button(format!("Ask {role}"), Action::Thread, ButtonKind::Ghost),
                answer,
            ],
            stamp: None,
            note: Vec::new(),
            submit,
        };
    }
    answer.shortcut = Some("Ctrl ↵");
    let mut note = if feed.link != Link::Up {
        vec![span(
            "Offline: answers need a live connection to corrald. Nothing is queued.",
        )]
    } else if session.is_none() {
        vec![span(
            "Waiting for this device's authenticated session before answering.",
        )]
    } else if !session.is_some_and(|session| session.authority.reply) {
        vec![span(
            "This device may not answer questions: its reply authority is off.",
        )]
    } else if row.is_some_and(|row| !row.answerable) {
        vec![span("corrald does not take this answer from the app.")]
    } else {
        vec![
            span("Runs "),
            span(format!(
                "corral question answer --owner {} --rev {}",
                detail.key, detail.rev
            ))
            .mono(),
        ]
    };
    if let Some(via) = via {
        note.push(span("\n"));
        note.push(span(format!("actor=owner · via={via}")).mono());
    }
    DecideBar {
        buttons: vec![
            answer,
            button(
                format!("Ask the {role}"),
                Action::FocusReply,
                ButtonKind::Ghost,
            ),
            button(
                "Discuss with PM",
                Action::AskAbout(key.clone(), true),
                ButtonKind::Quiet,
            ),
        ],
        stamp: None,
        note,
        submit,
    }
}
