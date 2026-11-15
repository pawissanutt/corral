use super::*;
use crate::model::effect::DataMode;

#[test]
fn offline_send_queues_instead_of_refusing() {
    for conn in [Conn::Offline, Conn::Down] {
        for thread in ["A-7", "A-17"] {
            for target in [AskTarget::Assistant, AskTarget::Pm] {
                let mut state = State {
                    thread,
                    ..State::default()
                };
                let env = Env {
                    conn,
                    mode: DataMode::Live,
                    form: Form::Desktop,
                };
                let effects = if thread == "A-7" {
                    state.send("queue T-873", target, env)
                } else {
                    state.send_to_thread(thread, "queue T-873", env)
                };
                assert_eq!(state.sent.len(), 1);
                assert_eq!(state.sent[0].state, MessageState::QueuedOnDevice);
                assert!(effects.is_empty());
            }
        }
    }
}

#[test]
fn late_input_is_answerable_but_never_authoritative() {
    let items = thread(
        &State::default(),
        "A-7",
        Env {
            conn: Conn::Healthy,
            mode: DataMode::Live,
            form: Form::Desktop,
        },
    );
    assert!(!MessageState::Late.can_instruct());
    assert!(items.iter().any(|item| matches!(item, ThreadItem::Assistant(message) if message.at == "21:47" && message.body.iter().any(|body| matches!(body, Body::Paragraph(_))))));
    assert!(items.iter().any(|item| matches!(item, ThreadItem::Assistant(message) if message.at == "21:47" && message.body.iter().any(|body| matches!(body, Body::Action(ActionCard { class: ActionClass::Late, outcome: ActionOutcome::Late { .. }, .. }))))));
}

#[test]
fn model_cycle_wraps_without_losing_the_conversation() {
    let mut state = State {
        model: Some(MODELS[MODELS.len() - 1].name),
        last_q: Some("kept".into()),
        ..State::default()
    };
    state.cycle_model();
    assert_eq!(state.model_name(), MODELS[0].name);
    assert_eq!(state.last_q.as_deref(), Some("kept"));
}

#[test]
fn hotkey_send_keeps_the_actual_delivery_state() {
    let mut state = State::default();
    let env = Env {
        conn: Conn::Offline,
        mode: DataMode::Live,
        form: Form::Desktop,
    };
    state.send_from_hotkey("queue T-873", env);
    assert_eq!(state.hotkey, HotkeyState::Typing);
    assert_eq!(state.hotkey_sent, Some(MessageState::QueuedOnDevice));
    assert_eq!(
        hotkey_window(&state, env).sent.map(|chip| chip.hue),
        Some(crate::model::hue::Hue::Held)
    );
    state.hotkey = HotkeyState::Filed;
    let sent = state.sent.len();
    state.send_from_hotkey("  ", env);
    assert_eq!(state.hotkey, HotkeyState::Filed);
    assert_eq!(state.sent.len(), sent);
}

#[test]
fn sent_draft_stays_retired_until_the_client_drops_it_then_restores_again() {
    // A composer mounted after Send but before the client retired the saved draft (phone
    // CloseAsk → thread page) must not bring the sent words back; another scope is untouched.
    let mut state = State::default();
    state.handed_off("A-4", "FINAL MODEL PROOF".into());
    assert!(state.already_sent("A-4", Some("FINAL MODEL PROOF")));
    assert!(!state.already_sent("quick", Some("FINAL MODEL PROOF")));
    // The client retired it: the handoff ends and a later saved draft restores normally.
    assert!(!state.already_sent("A-4", None));
    assert!(!state.already_sent("A-4", Some("FINAL MODEL PROOF")));
}

#[test]
fn quick_scope_is_the_current_generation_and_earlier_ones_stay_their_own_threads() {
    // The quick thread rolled from A-3 (ended) to A-6; id order must not make "quick" mean A-3,
    // and A-3 stays openable as its own historical thread.
    use corral_protocol::rows::{AssistantKind, AssistantLocation, AssistantRow, AssistantState};
    let quick = |id: &str, state| AssistantRow {
        id: id.into(),
        kind: AssistantKind::Quick,
        title: "Quick thread".into(),
        state,
        live_in: AssistantLocation::Daemon,
        profile: None,
        focus: None,
        pending: 0,
        turn: None,
        cost_usd: None,
        cost_basis: None,
        wait: None,
        rev: 1,
    };
    let mut feed = crate::model::daemon::Feed::default();
    for row in [
        quick("A-3", AssistantState::Ended),
        quick("A-6", AssistantState::Ready),
    ] {
        feed.assistants.insert(row.id.as_str().into(), row);
    }
    assert_eq!(row(&feed, QUICK).map(|row| row.id.as_str()), Some("A-6"));
    assert_eq!(row(&feed, "A-3").map(|row| row.id.as_str()), Some("A-3"));
    let blocks = live_agents(&State::default(), &feed, true);
    assert_eq!(blocks.quick_thread.id, "A-6");
    assert!(blocks.sessions.iter().any(|session| session.id == "A-3"));
    assert!(blocks.sessions.iter().all(|session| session.id != "A-6"));
    // No filings source negotiated yet: Filed from chat claims no count, never a complete "0".
    assert_eq!(blocks.filed_total, None);
    assert!(blocks.filed_note.is_some());
}

#[test]
fn rolled_quick_thread_never_shows_the_previous_generations_reply_as_its_own() {
    // Quick rolled from A-6 (which answered) to A-7, whose first turn failed with no reply. The
    // scope's last streamed turn is still A-6's: A-7 must show no assistant answer at all.
    use corral_protocol::assistant::{EntryKind, ThreadEntry};
    use corral_protocol::rows::{AssistantKind, AssistantLocation, AssistantRow, AssistantState};
    use corral_protocol::subscription::LiveText;
    let quick = |id: &str, state| AssistantRow {
        id: id.into(),
        kind: AssistantKind::Quick,
        title: "Quick thread".into(),
        state,
        live_in: AssistantLocation::Daemon,
        profile: Some("app-smoke".into()),
        focus: None,
        pending: 0,
        turn: None,
        cost_usd: None,
        cost_basis: None,
        wait: None,
        rev: 1,
    };
    let mut feed = crate::model::daemon::Feed::default();
    for row in [
        quick("A-6", AssistantState::Ended),
        quick("A-7", AssistantState::Failed),
    ] {
        feed.assistants.insert(row.id.as_str().into(), row);
    }
    let asked = ThreadEntry {
        id: "A-7/M1".into(),
        rev: 1,
        at: "2026-11-14T19:40:00Z".into(),
        kind: EntryKind::Message,
        device: None,
        via: None,
        text: Some("report the private fixture task".into()),
        state: None,
        model: None,
        cost_usd: None,
        tools: Vec::new(),
        links: Vec::new(),
        card: None,
        attachments: Vec::new(),
        typed_at: None,
        part: None,
        completion: None,
        cost_basis: None,
    };
    for done in [true, false] {
        let mut thread = crate::model::daemon::ThreadSnapshot::default();
        thread
            .entries
            .insert(asked.id.as_str().into(), asked.clone());
        thread.live = Some(LiveText {
            entry: "A-6/M3".into(),
            turn: 3,
            offset: 0,
            text: "Private wiring answer".into(),
            done,
        });
        feed.threads.insert(QUICK.into(), thread);
        let items = live_items(&State::default(), &feed, QUICK, true);
        assert!(
            !items
                .iter()
                .any(|item| matches!(item, ThreadItem::Assistant(_))),
            "A-6's turn rendered as A-7's (done: {done})"
        );
    }
}

#[test]
fn filed_from_chat_lists_every_filing_from_the_global_source_without_threads_or_assistant() {
    // No thread is loaded and the assistant runtime is not offered; the served filings page
    // alone lists the old quick generation's AI-filed T-008 and A-7's as-written T-012.
    use corral_protocol::detail::{DetailBody, DetailReply, FiledDecision, FiledIdea, FilingsPage};
    let mut feed = crate::model::daemon::Feed::default();
    feed.link = crate::model::daemon::Link::Up;
    feed.capabilities
        .insert(corral_protocol::app::FILINGS_V1.into());
    let idea = |seq: u64, task: &str, session: &str, decision| FiledIdea {
        id: format!("{session}/M{seq}"),
        seq,
        task: task.into(),
        session: session.into(),
        quick: true,
        device: None,
        at: "2026-11-14T19:00:00Z".into(),
        title: task.into(),
        status: "idea".into(),
        decision,
    };
    let page = FilingsPage {
        total: 3,
        rows: vec![
            idea(2, "T-012", "A-7", None),
            idea(
                1,
                "T-008",
                "A-3",
                Some(FiledDecision {
                    action: "merge".into(),
                    into: Some("T-002".into()),
                }),
            ),
        ],
        older: Some(1),
        newer: None,
    };
    let target = filings_target(&State::default());
    feed.details.insert(
        crate::model::daemon::detail_key(&target),
        crate::model::daemon::DetailCache {
            target: target.clone(),
            pages: vec![DetailReply {
                etag: "e1".into(),
                as_of: "2026-11-14T19:01:00Z".into(),
                seq: 1,
                next: None,
                body: Some(DetailBody::Filings(page.clone())),
            }],
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Filings(page)),
        },
    );
    let blocks = live_agents(&State::default(), &feed, false);
    let tasks: Vec<&str> = blocks.filed.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(tasks, ["T-012", "T-008"]);
    // The total is the index's, not the rows on this page, and the merged one opens its target.
    assert_eq!(blocks.filed_total, Some(3));
    assert_eq!(
        blocks.filed[1].action,
        Action::OpenLive(crate::model::live::Live::Task("T-002".into()))
    );
    assert!(
        blocks
            .filed_pager
            .iter()
            .any(|button| button.action == Action::FilingsPage(Some(1)))
    );
}

#[test]
fn an_ended_session_reads_ended_even_with_a_recorded_failure() {
    // corrald reports A-5 Ended while its row keeps the failure that stopped it; the session is
    // over, not waiting for the assistant.
    use corral_protocol::rows::{
        AssistantKind, AssistantLocation, AssistantRow, AssistantState, WaitReason,
    };
    let mut feed = crate::model::daemon::Feed::default();
    feed.link = crate::model::daemon::Link::Up;
    let row = AssistantRow {
        id: "A-5".into(),
        kind: AssistantKind::Session,
        title: "Session".into(),
        state: AssistantState::Ended,
        live_in: AssistantLocation::None,
        profile: Some("app-smoke".into()),
        focus: None,
        pending: 0,
        turn: None,
        cost_usd: Some(0.02),
        cost_basis: None,
        wait: Some(WaitReason {
            code: "unavailable".into(),
            text: "Assistant host exited: exit status: 2".into(),
            retry_at: None,
        }),
        rev: 1,
    };
    feed.assistants.insert("A-5".into(), row);
    let chip = live_chip(&feed, "A-5");
    assert_ne!(chip.hue, crate::model::hue::Hue::Held);
    assert!(ended(&feed, "A-5"));
}

#[test]
fn a_new_quick_generation_streaming_before_its_row_is_never_labelled_as_the_old_one() {
    // corrald's rows still name A-9 (ended) while A-10's first answer already streams in the
    // quick thread: everything current (the thread's author, the wall's quick tile) is A-10,
    // compared as a generation number, not as text ("A-10" sorts before "A-9").
    use corral_protocol::assistant::{EntryKind, ThreadEntry};
    use corral_protocol::rows::{AssistantKind, AssistantLocation, AssistantRow, AssistantState};
    use corral_protocol::subscription::LiveText;
    let mut feed = crate::model::daemon::Feed::default();
    feed.link = crate::model::daemon::Link::Up;
    feed.assistants.insert(
        "A-9".into(),
        AssistantRow {
            id: "A-9".into(),
            kind: AssistantKind::Quick,
            title: "Quick thread".into(),
            state: AssistantState::Ended,
            live_in: AssistantLocation::None,
            profile: None,
            focus: None,
            pending: 0,
            turn: None,
            cost_usd: None,
            cost_basis: None,
            wait: None,
            rev: 1,
        },
    );
    let asked = ThreadEntry {
        id: "A-10/M1".into(),
        rev: 1,
        at: "2026-11-14T20:32:00Z".into(),
        kind: EntryKind::Message,
        device: None,
        via: None,
        text: Some("next question".into()),
        state: None,
        model: None,
        cost_usd: None,
        tools: Vec::new(),
        links: Vec::new(),
        card: None,
        attachments: Vec::new(),
        typed_at: None,
        part: None,
        completion: None,
        cost_basis: None,
    };
    let mut thread = crate::model::daemon::ThreadSnapshot::default();
    thread.entries.insert(asked.id.as_str().into(), asked);
    thread.live = Some(LiveText {
        entry: "A-10/M1/r".into(),
        turn: 1,
        offset: 0,
        text: "partial answer".into(),
        done: false,
    });
    feed.threads.insert(QUICK.into(), thread);
    assert_eq!(quick_id(&feed).as_deref(), Some("A-10"));
    let authors: Vec<String> = live_items(&State::default(), &feed, QUICK, true)
        .into_iter()
        .filter_map(|item| match item {
            ThreadItem::Assistant(message) => Some(message.id),
            _ => None,
        })
        .collect();
    assert_eq!(authors, ["A-10"]);
    assert_eq!(
        live_agents(&State::default(), &feed, true).quick_thread.id,
        "A-10"
    );
}

#[test]
fn a_thread_lists_each_filed_task_once_however_many_entries_record_it() {
    // T-012 was saved as written (the owner's message state) and then shows as an Idea entry:
    // one filing. T-013 is a different filing and stays its own row.
    use corral_protocol::assistant::{EntryCard, EntryKind, MessageState, ThreadEntry};
    let entry = |id: &str, kind, state, card| ThreadEntry {
        id: id.into(),
        rev: 1,
        at: format!("2026-11-14T20:0{}:00Z", id.len() % 10),
        kind,
        device: None,
        via: None,
        text: Some("keep the receipts".into()),
        state,
        model: None,
        cost_usd: None,
        tools: Vec::new(),
        links: Vec::new(),
        card,
        attachments: Vec::new(),
        typed_at: None,
        part: None,
        completion: None,
        cost_basis: None,
    };
    let idea = |task: &str| EntryCard::Idea {
        task: task.into(),
        title: task.into(),
        labels: Vec::new(),
        criteria: Vec::new(),
        duplicates: Vec::new(),
        words: "keep the receipts".into(),
    };
    let mut thread = crate::model::daemon::ThreadSnapshot::default();
    for item in [
        entry(
            "A-7/M1",
            EntryKind::Message,
            Some(MessageState::Saved {
                task: "T-012".into(),
            }),
            None,
        ),
        entry("A-7/M1/i", EntryKind::Idea, None, Some(idea("T-012"))),
        entry("A-7/M2/i", EntryKind::Idea, None, Some(idea("T-013"))),
    ] {
        thread.entries.insert(item.id.as_str().into(), item);
    }
    let mut feed = crate::model::daemon::Feed::default();
    feed.threads.insert("A-7".into(), thread);
    let mut tasks: Vec<String> = live_filed(&feed, "A-7")
        .into_iter()
        .map(|row| row.id)
        .collect();
    tasks.sort();
    assert_eq!(tasks, ["T-012", "T-013"]);
}
