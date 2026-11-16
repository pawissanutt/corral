use super::{content, put, stamp};
use crate::model::daemon::*;
use corral_protocol::{assistant::*, board::CostBasis, detail::*, rows::*};

pub(super) fn controls(status: AgentStatus) -> SessionControls {
    let live = status != AgentStatus::Ended;
    let yes = |available, refusal: &str| ControlState {
        available,
        refusal: (!available).then(|| refusal.into()),
    };
    SessionControls {
        send: yes(live, "ended"),
        cancel: yes(status == AgentStatus::Working, "no_turn"),
        pause: yes(live && status != AgentStatus::Paused, "not_running"),
        resume: yes(status == AgentStatus::Paused, "not_paused"),
        stop: yes(live, "ended"),
        relaunch: yes(!live, "host_alive"),
        keys: yes(false, "preview_no_pane"),
        take_over: yes(false, "already_daemon_hosted"),
    }
}

pub(super) fn populate(feed: &mut Feed) {
    for (id, role, task, now, status) in [
        (
            "pm",
            "pm",
            None,
            "waiting · next wake on T-894 gate",
            AgentStatus::Live,
        ),
        (
            "T-871",
            "worker",
            Some("T-871"),
            "Edit crates/corral-core/src/annot/record.rs",
            AgentStatus::Working,
        ),
        (
            "T-854",
            "worker",
            Some("T-854"),
            "cargo test -p corral-ops cowork::publish",
            AgentStatus::Working,
        ),
        (
            "T-868",
            "worker",
            Some("T-868"),
            "waiting on you · T-868/R1/Q1",
            AgentStatus::Asking,
        ),
        (
            "gate-T-890",
            "gate",
            Some("T-890"),
            "nextest corral-ops 412/980",
            AgentStatus::Gating,
        ),
        (
            "gate-T-894",
            "gate",
            Some("T-894"),
            "Compiling corral-tui",
            AgentStatus::Gating,
        ),
        (
            "T-867",
            "worker",
            Some("T-867"),
            "candidate 5949a4b awaits re-gate",
            AgentStatus::Idle,
        ),
    ] {
        let title = task
            .and_then(|id| feed.board.task(id))
            .map(|row| row.title.clone())
            .unwrap_or_else(|| "Preview PM · supervisor generation 14".into());
        let at = stamp(feed, feed.now.saturating_sub(10800));
        let row = AgentRow {
            id: id.into(),
            role: role.into(),
            task: task.map(str::to_owned),
            run: task.map(|_| "R1".into()),
            title: format!("Preview · {title}"),
            profile: Some("pi-sol xhigh".into()),
            runtime: Some("preview".into()),
            machine: Some("preview-workstation".into()),
            pane: Some(format!("preview:{id}")),
            status,
            now: now.into(),
            since: Some(at.clone()),
            tail: vec![format!("Preview · {now}")],
            last_output_at: Some(stamp(feed, feed.now.saturating_sub(output_age(status)))),
            activity: Some(sample_activity(feed, status)),
            steps: (id == "T-871").then(|| {
                [
                    ("Read the annotation contract", AgentStepState::Done),
                    ("Trace existing record callers", AgentStepState::Done),
                    ("Update record handling", AgentStepState::InProgress),
                    ("Exercise focused regressions", AgentStepState::Pending),
                    ("Summarize the change", AgentStepState::Pending),
                ]
                .into_iter()
                .map(|(title, state)| AgentStep {
                    title: title.into(),
                    state,
                })
                .collect()
            }),
            cost_usd: Some(3.9),
            cost_basis: Some(CostBasis::Estimated),
            tokens: Some(12000),
            rev: 1,
            generation: Some(if role == "pm" { 14 } else { 1 }),
            turn: (status == AgentStatus::Working).then_some(1),
            controls: controls(status),
        };
        feed.agents.insert(id.into(), row);
        if let Some(task_id) = task
            && role == "worker"
            && let Some(mut row) = feed.board.task(task_id).cloned()
        {
            row.agent = Some(Agent {
                session: Some(id.into()),
                role: role.into(),
                profile: Some("pi-sol xhigh".into()),
                runtime: Some("preview".into()),
                machine: Some("preview-workstation".into()),
            });
            content::save_task(feed, row);
        }
        let mut messages = vec![message(
            role,
            &at,
            vec![SessionItem::Text {
                text: format!("Preview · {title}. {now}."),
            }],
        )];
        if id == "T-868" {
            messages.push(message(
                "worker",
                &at,
                vec![
                    SessionItem::Thinking {
                        text: "Keep restoration policy separate from queue identity and lifecycle."
                            .into(),
                    },
                    SessionItem::ToolCall {
                        call_id: "preview-check".into(),
                        name: "Bash".into(),
                        arguments: "{\"command\":\"bun test test/queue-host.test.ts\"}".into(),
                        summary: "bun test test/queue-host.test.ts".into(),
                        state: SessionToolState::Done,
                        paired: true,
                        call: None,
                        output: Some("16 pass · 0 fail · preview".into()),
                        result: None,
                        elapsed_ms: Some(1640),
                        diff: None,
                        content: None,
                    },
                ],
            ));
            if let Some(DetailBody::Question(question)) = feed
                .detail(&DetailRef::Question {
                    key: "T-868/R1/Q1".into(),
                })
                .and_then(|cache| cache.body())
            {
                messages.push(message(
                    "worker",
                    &at,
                    vec![SessionItem::QuestionCard {
                        key: question.key.clone(),
                        rev: question.rev,
                        text: question.text.clone(),
                        options: question.options.clone(),
                        answer: question.answer.clone(),
                    }],
                ));
            }
            messages.push(message(
                "owner",
                &at,
                vec![SessionItem::Text {
                    text: "Does option 1 change anything in rich mode?".into(),
                }],
            ));
            messages.push(message(
                "worker",
                &at,
                vec![SessionItem::Text {
                    text:
                        "No. Rich mode stays read-only/count-only; neither option restores there."
                            .into(),
                }],
            ));
        }
        put(
            feed,
            DetailRef::Session { id: id.into() },
            DetailBody::Session(SessionPage {
                id: id.into(),
                role: role.into(),
                task: task.map(str::to_owned),
                run: task.map(|_| "R1".into()),
                machine: Some("preview-workstation".into()),
                messages,
                older: None,
                replyable: true,
                reply_refusal: None,
            }),
        );
        let lines = vec![format!("Preview · {id} · {title}"), now.into()];
        put(
            feed,
            DetailRef::Terminal { id: id.into() },
            DetailBody::Terminal(TerminalSnapshot {
                rows: lines
                    .iter()
                    .map(|text| {
                        vec![TerminalRun {
                            text: text.clone(),
                            dim: true,
                            ..TerminalRun::default()
                        }]
                    })
                    .collect(),
                lines,
                captured_at: at,
                live: false,
            }),
        );
        feed.topics
            .insert(format!("terminal:{id}"), TopicState::default());
        feed.topics
            .insert(format!("session:{id}"), TopicState::default());
    }
    for (id, title, focus, ended) in [
        ("A-7", "Quick thread", None, false),
        ("A-15", "T-854 co-work check", Some("T-854"), false),
        (
            "A-17",
            "Board TUI mockup: async loading and settings",
            None,
            false,
        ),
        ("A-12", "Co-work C2 follow-ups", Some("T-853"), false),
        ("A-11", "PM read contract questions", Some("T-890"), true),
        ("A-18", "Changes review notes", None, false),
    ] {
        assistant(feed, id, title, focus, Some("pi-sol xhigh".into()));
        if ended && let Some(row) = feed.assistants.get_mut(id) {
            row.state = AssistantState::Ended;
            row.live_in = AssistantLocation::None;
        }
        if ended && let Some(row) = feed.agents.get_mut(id) {
            row.status = AgentStatus::Ended;
            row.now = "Preview · session ended".into();
            row.controls = controls(AgentStatus::Ended);
        }
    }
    let at = stamp(feed, feed.now.saturating_sub(180));
    let text = "T-867 is stale because the base moved after gate candidate 5949a4b. T-870 also waits on your answer to T-868/R1/Q1 (restore policy). T-869 depends on T-867, not on that answer. Preview · this is an illustrative recorded conversation.";
    let until = stamp(feed, feed.now);
    if let Some(row) = feed.agents.get_mut("A-7") {
        row.now = text.into();
        row.last_output_at = Some(at.clone());
        row.activity = Some(AgentActivity {
            until,
            per_minute: vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 1, 0, 0, 0, 0, 1, 0, 0, 0],
        });
    }
    put(
        feed,
        DetailRef::Session { id: "A-7".into() },
        DetailBody::Session(SessionPage {
            id: "A-7".into(),
            role: "assistant".into(),
            task: None,
            run: None,
            machine: Some("preview-workstation".into()),
            messages: vec![message(
                "assistant",
                &at,
                vec![SessionItem::Text { text: text.into() }],
            )],
            older: None,
            replyable: true,
            reply_refusal: None,
        }),
    );
    let mut answer = entry("preview-answer", &at, EntryKind::Answer, text);
    answer.links = [
        ("task", "T-867"),
        ("plan", "T-870"),
        ("question", "T-868/R1/Q1"),
    ]
    .into_iter()
    .map(|(kind, id)| ContextRef {
        kind: kind.into(),
        id: id.into(),
        rev: None,
    })
    .collect();
    if let Some(thread) = feed.threads.get_mut("A-7") {
        thread
            .entries
            .insert(answer.id.clone().into(), answer.clone());
    }
    if let Some(thread) = feed.threads.get_mut("quick") {
        thread.entries.insert(answer.id.clone().into(), answer);
    }
    for (task, words) in [
        (
            "T-901",
            "review diffs need an inline comment box tied to the candidate SHA",
        ),
        ("T-902", "board: show the PM inbox count in the title bar"),
        (
            "T-903",
            "we should cache gate receipts so a base move doesn't force a full re-gate",
        ),
    ] {
        let title = feed
            .board
            .task(task)
            .map(|row| row.title.clone())
            .unwrap_or_default();
        if let Some(mut row) = feed.board.task(task).cloned() {
            row.goal = Some(words.into());
            row.now_code = "idea".into();
            content::save_task(feed, row);
        }
        let mut filed = entry(
            &format!("preview-filed:{task}"),
            &at,
            EntryKind::Idea,
            words,
        );
        filed.card = Some(EntryCard::Idea {
            task: task.into(),
            title,
            labels: vec!["preview".into()],
            criteria: vec!["Preserve consumer contracts".into()],
            duplicates: vec![],
            words: words.into(),
        });
        for scope in ["quick", "A-7"] {
            if let Some(thread) = feed.threads.get_mut(scope) {
                thread
                    .entries
                    .insert(filed.id.clone().into(), filed.clone());
            }
        }
    }
    let mut owner = entry(
        "preview-owner-question",
        &stamp(feed, feed.now.saturating_sub(240)),
        EntryKind::Message,
        "why is T-867 stale, and what's blocking T-870?",
    );
    owner.typed_at = Some(owner.at.clone());
    for scope in ["quick", "A-7"] {
        if let Some(thread) = feed.threads.get_mut(scope) {
            thread
                .entries
                .insert(owner.id.clone().into(), owner.clone());
        }
    }
}

pub(super) fn message(author: &str, at: &str, items: Vec<SessionItem>) -> SessionMessage {
    SessionMessage {
        author: author.into(),
        at: Some(at.into()),
        items,
        model: Some("preview · gpt-6.1-sol".into()),
        stop_reason: None,
        usage: None,
    }
}

pub(super) fn entry(id: &str, at: &str, kind: EntryKind, text: &str) -> ThreadEntry {
    ThreadEntry {
        id: id.into(),
        rev: 1,
        at: at.into(),
        typed_at: None,
        kind,
        device: Some("preview-device".into()),
        via: Some("preview:local-only".into()),
        text: Some(text.into()),
        state: Some(MessageState::Delivered),
        model: None,
        cost_usd: None,
        tools: vec![],
        part: None,
        completion: None,
        cost_basis: None,
        links: vec![],
        card: None,
        attachments: vec![],
    }
}

pub(super) fn assistant(
    feed: &mut Feed,
    id: &str,
    title: &str,
    focus: Option<&str>,
    profile: Option<String>,
) {
    let at = stamp(feed, feed.now.saturating_sub(9 * 60));
    let now = "Preview · ready for your next message";
    let agent = AgentRow {
        id: id.into(),
        role: "assistant".into(),
        task: None,
        run: None,
        title: format!("Preview · {title}"),
        profile: profile.clone(),
        runtime: Some("preview daemon-hosted".into()),
        machine: Some("preview-workstation".into()),
        pane: None,
        status: AgentStatus::Idle,
        now: now.into(),
        since: Some(stamp(feed, feed.now.saturating_sub(4 * 3600))),
        tail: vec![],
        cost_usd: Some(1.12),
        cost_basis: Some(CostBasis::Estimated),
        tokens: None,
        rev: 1,
        generation: Some(1),
        turn: None,
        controls: controls(AgentStatus::Idle),
        last_output_at: Some(at.clone()),
        activity: Some(sample_activity(feed, AgentStatus::Idle)),
        steps: None,
    };
    feed.agents.insert(id.into(), agent);
    put(
        feed,
        DetailRef::Session { id: id.into() },
        DetailBody::Session(SessionPage {
            id: id.into(),
            role: "assistant".into(),
            task: None,
            run: None,
            machine: Some("preview-workstation".into()),
            messages: vec![message(
                "assistant",
                &at,
                vec![SessionItem::Text { text: now.into() }],
            )],
            older: None,
            replyable: true,
            reply_refusal: None,
        }),
    );
    feed.assistants.insert(
        id.into(),
        AssistantRow {
            id: id.into(),
            kind: if id == "A-7" {
                AssistantKind::Quick
            } else {
                AssistantKind::Session
            },
            title: format!("Preview · {title}"),
            state: AssistantState::Ready,
            live_in: AssistantLocation::Daemon,
            profile,
            focus: focus.map(str::to_owned),
            pending: 0,
            turn: None,
            cost_usd: Some(1.12),
            cost_basis: Some(CostBasis::Estimated),
            wait: None,
            rev: 1,
        },
    );
    feed.threads.insert(id.into(), ThreadSnapshot::default());
    if id == "A-7" {
        feed.threads
            .insert("quick".into(), ThreadSnapshot::default());
    }
}

pub(super) fn append(feed: &mut Feed, id: &str, text: &str) -> Result<(), String> {
    let target = DetailRef::Session { id: id.into() };
    let Some(DetailBody::Session(mut page)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    else {
        return Err(format!("Preview session {id} does not exist"));
    };
    if !page.replyable {
        return Err("Preview session has ended".into());
    }
    page.messages.push(message(
        "owner",
        &stamp(feed, feed.now),
        vec![SessionItem::Text { text: text.into() }],
    ));
    put(feed, target, DetailBody::Session(page));
    Ok(())
}

pub(super) fn sync_worker(feed: &mut Feed, task_id: &str) {
    let Some(task) = feed.board.task(task_id).cloned() else {
        return;
    };
    let Some(binding) = &task.agent else { return };
    let Some(id) = &binding.session else { return };
    let status = match task.group {
        Group::Held | Group::Parked => AgentStatus::Paused,
        Group::Other => AgentStatus::Ended,
        Group::NeedsYou => AgentStatus::Asking,
        Group::Verifying => AgentStatus::Gating,
        _ => AgentStatus::Working,
    };
    let at = stamp(feed, feed.now);
    let activity = sample_activity(feed, status);
    let row = feed
        .agents
        .entry(id.clone().into())
        .or_insert_with(|| AgentRow {
            id: id.clone(),
            role: "worker".into(),
            task: Some(task_id.into()),
            run: Some("R1".into()),
            title: format!("Preview · {}", task.title),
            profile: binding.profile.clone(),
            runtime: Some("preview".into()),
            machine: Some("preview-workstation".into()),
            pane: Some(format!("preview:{id}")),
            status,
            now: task.now.clone(),
            since: Some(at.clone()),
            tail: vec![],
            last_output_at: Some(at.clone()),
            activity: Some(activity.clone()),
            steps: None,
            cost_usd: None,
            cost_basis: None,
            tokens: None,
            rev: 0,
            generation: Some(1),
            turn: None,
            controls: controls(status),
        });
    row.status = status;
    row.now = task.now.clone();
    row.last_output_at = Some(at.clone());
    row.activity = Some(activity);
    row.profile = binding.profile.clone();
    row.title = format!("Preview · {}", task.title);
    row.controls = controls(status);
    row.rev += 1;
    row.turn = (status == AgentStatus::Working).then_some(row.turn.unwrap_or(1));
    let target = DetailRef::Session { id: id.clone() };
    let mut page = match feed.detail(&target).and_then(|cache| cache.body()).cloned() {
        Some(DetailBody::Session(page)) => page,
        _ => SessionPage {
            id: id.clone(),
            role: "worker".into(),
            task: Some(task_id.into()),
            run: Some("R1".into()),
            machine: Some("preview-workstation".into()),
            messages: vec![message(
                "notice",
                &at,
                vec![SessionItem::Text {
                    text: "Preview · worker dispatched locally; no process was launched".into(),
                }],
            )],
            older: None,
            replyable: true,
            reply_refusal: None,
        },
    };
    page.replyable = status != AgentStatus::Ended;
    page.reply_refusal = (!page.replyable).then(|| "Preview session ended".into());
    put(feed, target, DetailBody::Session(page));
    put(
        feed,
        DetailRef::Terminal { id: id.clone() },
        DetailBody::Terminal(TerminalSnapshot {
            lines: vec![format!("Preview · {task_id} · {}", task.now)],
            captured_at: at,
            live: false,
            rows: vec![],
        }),
    );
    feed.topics
        .insert(format!("session:{id}"), TopicState::default());
    feed.topics
        .insert(format!("terminal:{id}"), TopicState::default());
}

fn output_age(status: AgentStatus) -> u64 {
    match status {
        AgentStatus::Working | AgentStatus::Asking => 38,
        AgentStatus::Gating | AgentStatus::Reviewing => 12,
        AgentStatus::Idle | AgentStatus::Waiting => 9 * 60,
        AgentStatus::Ended => 36 * 60,
        _ => 3 * 60,
    }
}

fn sample_activity(feed: &Feed, status: AgentStatus) -> AgentActivity {
    let per_minute = match status {
        AgentStatus::Working => [1, 2, 1, 0, 3, 1, 2, 4, 1, 2, 1, 0, 2, 3, 1, 2, 1, 3, 2, 1],
        AgentStatus::Gating | AgentStatus::Reviewing => {
            [0, 0, 1, 0, 1, 1, 0, 2, 1, 1, 2, 2, 1, 3, 2, 1, 2, 3, 3, 2]
        }
        AgentStatus::Asking => [2, 1, 2, 1, 0, 1, 2, 1, 1, 0, 1, 0, 1, 1, 0, 0, 0, 0, 0, 1],
        AgentStatus::Idle | AgentStatus::Waiting => {
            [2, 1, 2, 3, 2, 1, 2, 1, 1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        }
        AgentStatus::Ended => [0; 20],
        _ => [0, 1, 0, 0, 0, 1, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0],
    };
    AgentActivity {
        until: stamp(feed, feed.now),
        per_minute: per_minute.to_vec(),
    }
}
