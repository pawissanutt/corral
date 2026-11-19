//! What the sample's pi sessions say: the seeded transcripts of two pi workers and the chats,
//! and the words each turn streams in answer to its input. Every word is labelled sample
//! content where it could pass for a model's.

use std::path::PathBuf;

use corral_protocol::detail::{CardKind, CardOption, CardQuestion, SessionCard, SessionItem};
use corral_types::{Origin, SessionRef};

use super::pi::{self, Host, Input, Role, tool_item};
use super::sample::{HOUR, MINUTE};
use super::{ago, record, rollup, save, stamp};
use crate::model::Id;
use crate::model::daemon::Feed;

/// One step of a scripted turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Beat {
    /// Words streamed into the current message, a few per tick.
    Say(String),
    /// A tool call that runs one tick per output line.
    Tool {
        tool: &'static str,
        args: String,
        lines: Vec<String>,
    },
    /// The turn ends asking the owner this.
    Ask(Question),
}

/// A question a host asks with `ask_user`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Question {
    pub(super) id: String,
    pub(super) text: String,
    pub(super) options: Vec<String>,
}

impl Question {
    /// The owner question card the host serves for it, opened at `at`.
    pub(super) fn card(&self, at: &str) -> SessionCard {
        SessionCard {
            id: self.id.clone(),
            kind: CardKind::Question,
            title: self.text.clone(),
            meta: None,
            body: None,
            body_more: false,
            questions: vec![CardQuestion {
                header: None,
                text: self.text.clone(),
                options: self
                    .options
                    .iter()
                    .enumerate()
                    .map(|(n, label)| CardOption {
                        id: format!("o{}", n + 1),
                        label: label.clone(),
                        detail: None,
                        unavailable: None,
                        needs_text: None,
                        destructive: false,
                    })
                    .collect(),
                initial: None,
                multi: false,
                free_text: true,
            }],
            child: None,
            tool: Some("ask_user".into()),
            opened_at: at.into(),
            expires_at: None,
        }
    }
}

/// The pi worker that is streaming when the preview opens, with checkpoints to rewind to.
pub(super) const STREAMING: &str = "T-14_R1";
/// The pi worker that idles after its last send-back.
pub(super) const IDLE: &str = "T-9_R2";

fn say(words: &str) -> Beat {
    Beat::Say(format!("{words} "))
}

fn tool(tool: &'static str, args: &str, lines: &[&str]) -> Beat {
    Beat::Tool {
        tool,
        args: args.into(),
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
    }
}

fn first_question() -> Question {
    Question {
        id: "q-1".into(),
        text: "Which clock should Timeline axes use when this device and corrald disagree?".into(),
        options: vec!["This device's zone".into(), "corrald's zone".into()],
    }
}

fn second_question() -> Question {
    Question {
        id: "q-2".into(),
        text: "Anything the follow-up task should say besides that?".into(),
        options: Vec::new(),
    }
}

/// The words a host's next turn says to `inputs`.
pub(super) fn reply(feed: &Feed, id: &str, inputs: &[Input]) -> Vec<Beat> {
    let last = inputs.last().map_or("", |input| input.text.as_str());
    let chat = feed
        .hosts
        .get(id)
        .is_some_and(|host| host.role == Role::Chat);
    if let Some(answer) = last.strip_prefix("[ask_user answer q-1] ") {
        return vec![
            say(&format!(
                "Preview · sample answer. Going with {answer} for the axes: every label is drawn from the run's own timestamp, so a run that crossed midnight shows both days."
            )),
            Beat::Ask(second_question()),
        ];
    }
    if let Some(answer) = last.strip_prefix("[ask_user answer q-2] ") {
        return vec![say(&format!(
            "Preview · sample answer. Noted — the follow-up will say: “{answer}”. Nothing else is waiting on you here."
        ))];
    }
    if chat {
        return vec![say(&chat_answer(feed, last))];
    }
    if inputs.iter().any(|input| input.origin == Origin::Auto) {
        let notes = inputs
            .iter()
            .find(|input| input.origin == Origin::Auto)
            .map_or("", |input| input.text.trim_start_matches("[corral] "));
        return vec![
            say(&format!(
                "Preview · sample turn. Picking up what came back: {notes}"
            )),
            tool(
                "bash",
                "cargo test pairing::",
                &[
                    "   Compiling nsh-app-corral v0.1.0",
                    "    Finished test profile in 8.4s",
                    "running 6 tests",
                    "test pairing::marker_survives_reload ... ok",
                    "test pairing::forget_clears_marker ... ok",
                    "test result: ok. 6 passed; 0 failed",
                ],
            ),
            say(
                "The marker now stays until the stored credential has been read. Committed; the gate runs next.",
            ),
        ];
    }
    vec![
        say(&format!(
            "Preview · sample turn. Read your message: “{last}”. Checking what it touches first."
        )),
        tool(
            "read",
            "src/model/timeline/axis.rs",
            &[
                "412 lines",
                "fn label(at: Timestamp, zone: &TimeZone) -> String",
            ],
        ),
        say("Done; carrying on with the task from there."),
    ]
}

/// A chat's answer about the sample: the task it names, else what the sample holds.
fn chat_answer(feed: &Feed, asked: &str) -> String {
    let named = asked
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .find_map(|word| record(feed, &word.to_ascii_uppercase()));
    match named {
        Some(task) => format!(
            "Preview · sample answer, no model ran. {} “{}” is {}{}. Its record and runs are on its task page; ask me to do something with it and I run the corral command for you.",
            task.task.id,
            task.task.title,
            task.task.status.as_str(),
            task.task
                .blocked
                .as_ref()
                .map(|blocked| format!(", blocked on: {}", blocked.reason))
                .unwrap_or_default()
        ),
        None => format!(
            "Preview · sample answer, no model ran. The sample has {} open tasks; name one (T-14, say) and I tell you where it stands, or tell me what to file and I file it.",
            feed.board.tasks().len()
        ),
    }
}

/// A worker whose turn is open is working; one between turns is quiet since its last turn.
pub(super) fn quiet(feed: &mut Feed, id: &str, quiet: bool) {
    let Some((task, run)) = id.split_once('_') else {
        return;
    };
    let Some(mut rec) = record(feed, task) else {
        return;
    };
    let now = ago(feed, 0);
    let Some(worker) = rec.runs.iter_mut().find(|worker| worker.id == run) else {
        return;
    };
    let quiet_since = if quiet {
        worker.quiet_since.or(Some(now))
    } else {
        None
    };
    if worker.quiet_since == quiet_since {
        return;
    }
    worker.quiet_since = quiet_since;
    save(feed, rec);
    rollup::rebuild(feed);
}

/// A worker run of the sample becomes a pi session, its host keeping `turns`.
fn pi_worker(feed: &mut Feed, id: &str) {
    let Some((task, run)) = id.split_once('_') else {
        return;
    };
    let Some(mut rec) = record(feed, task) else {
        return;
    };
    if let Some(worker) = rec.runs.iter_mut().find(|worker| worker.id == run) {
        worker.session = Some(pi_session(task, run, worker.session.take()));
    }
    save(feed, rec);
}

/// A new worker run after a pi worker is a pi session too, and its host continues the earlier
/// run's transcript, as a restarted host reopens the newest sibling run's; `fresh` starts an
/// empty one.
pub(super) fn follow(
    feed: &mut Feed,
    rec: &mut corral_protocol::detail::TaskDetail,
    run: &str,
    fresh: bool,
) {
    let task = rec.task.id.clone();
    let earlier = rec
        .runs
        .iter()
        .rev()
        .filter(|worker| worker.id != run && worker.role == corral_types::Role::Worker)
        .find(|worker| matches!(worker.session, Some(SessionRef::Pi { .. })))
        .map(|worker| format!("{task}_{}", worker.id));
    let Some(host) = earlier.and_then(|earlier| feed.hosts.get(earlier.as_str()).cloned()) else {
        return;
    };
    if let Some(worker) = rec.runs.iter_mut().find(|worker| worker.id == run) {
        worker.session = Some(pi_session(&task, run, None));
    }
    let next = if fresh {
        Host::new(Role::Worker)
    } else {
        host.continued()
    };
    feed.hosts.insert(Id::from(format!("{task}_{run}")), next);
}

/// The pi session of run `run`, its host in `inner`.
pub(super) fn pi_session(task: &str, run: &str, inner: Option<SessionRef>) -> SessionRef {
    let dir = PathBuf::from(format!("/preview/corral/.corral-next/runs/{task}/{run}"));
    SessionRef::Pi {
        socket: PathBuf::from(format!("/run/user/1000/corral-next/{task}-{run}.sock")),
        inner: Box::new(inner.unwrap_or_else(|| SessionRef::Herdr {
            workspace: "corral".into(),
            pane: format!("{task}-{run}"),
        })),
        dir,
    }
}

fn tool_call(tool: &str, args: &str, lines: &[&str]) -> SessionItem {
    let lines: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    tool_item(format!("seed-{args}"), tool, args, &lines, true)
}

fn text(words: &str) -> SessionItem {
    SessionItem::Text { text: words.into() }
}

/// The sample's pi hosts: the streaming worker with its rewind points, the idle worker, and
/// every chat.
pub(super) fn populate(feed: &mut Feed) {
    let now = feed.now;
    let at = |ago: u64| stamp(now.saturating_sub(ago));
    let mut streaming = Host::new(Role::Worker);
    streaming.record_turn(
        &at(2 * HOUR),
        &["[corral] Read your prompt file: runs/T-14/R1/prompt.md"],
        vec![
            text("Preview · sample transcript. The task: Timeline axes should show the real calendar dates of the runs they draw."),
            tool_call("read", "src/model/timeline/axis.rs", &["398 lines"]),
        ],
    );
    streaming.record_turn(
        &at(HOUR + 40 * MINUTE),
        &["Use the device's time zone, not UTC."],
        vec![
            text("Understood: labels use the device's zone; corrald already sends RFC 3339 times."),
            tool_call("edit", "src/model/timeline/axis.rs", &["+14 −6"]),
        ],
    );
    streaming.record_turn(
        &at(55 * MINUTE),
        &["[corral] Gate red on 4f1c2a9: timeline::axis::labels_cross_midnight failed"],
        vec![
            text("A run that crossed midnight drew the previous day. Days are now floored in the zone, not in UTC."),
            tool_call("bash", "cargo test timeline::", &["test result: ok. 31 passed; 0 failed"]),
        ],
    );
    streaming.rewinds.push(corral_protocol::session::Rewound {
        turn: 2,
        checkpoint: "cp-0-2".into(),
        rescue_ref: format!("refs/worktree/corral/rescue/{STREAMING}/0"),
        from_head: super::sample::sha("rescue-0", 40),
        head: super::sample::sha("cp-0-2/head", 40),
        at: Some(at(HOUR + 45 * MINUTE)),
    });
    feed.hosts.insert(Id::from(STREAMING), streaming);
    pi_worker(feed, STREAMING);
    pi::start_with(
        feed,
        STREAMING,
        vec![Input {
            text: "Also check the week preset while you are there.".into(),
            origin: Origin::Owner,
            entry: None,
        }],
        opening(),
    );

    let mut idle = Host::new(Role::Worker);
    idle.record_turn(
        &at(18 * HOUR),
        &["[corral] Read your prompt file: runs/T-9/R2/prompt.md"],
        vec![text("Preview · sample transcript. Round 2: keep the paired marker until the stored credential is read.")],
    );
    idle.record_turn(
        &at(HOUR),
        &["[corral] Sent back by the reviewer: the paired marker is cleared on reload before the IndexedDB read resolves."],
        vec![text("I moved the clear behind the read, but the reviewer's case still clears it first on a cold start. Waiting for the owner.")],
    );
    feed.hosts.insert(Id::from(IDLE), idle);
    pi_worker(feed, IDLE);

    chat(
        feed,
        "A-7",
        &at(4 * MINUTE),
        "why is T-9 blocked, and what does T-12 wait on?",
        "Preview · sample answer. T-9 used both automatic change rounds: the reviewer still finds the paired marker cleared before the stored credential is read. Request changes with a note, retry, or land it anyway. T-12 depends on T-14, which is still running.",
    );
    chat(
        feed,
        "A-15",
        &at(12 * MINUTE),
        "Is T-14 drawing real dates yet?",
        "Preview · sample answer. Mostly: the axes draw from each run's timestamp. One choice is yours before I file the follow-up.",
    );
    if let Some(host) = feed.hosts.get_mut("A-15") {
        let card = first_question().card(&at(11 * MINUTE));
        let asked = pi::question_card(&at(11 * MINUTE), &card, "pending", serde_json::Value::Null);
        host.transcript.push(asked);
        host.pending.push(card);
    }
    chat(
        feed,
        "A-17",
        &at(50 * MINUTE),
        "Does Forget clear the paired marker on every tab?",
        "Preview · sample answer. Only on this tab; other tabs notice on their next read.",
    );
    if let Some(host) = feed.hosts.get_mut("A-17") {
        host.gone = true;
    }
    chat(
        feed,
        "A-11",
        &at(26 * HOUR),
        "What did the old engine's status table mean by parked?",
        "Preview · sample answer. Parked meant held out of the queue by hand; the simple engine calls that Backlog.",
    );
}

/// One recorded exchange in chat `id`.
pub(super) fn chat(feed: &mut Feed, id: &str, at: &str, asked: &str, answered: &str) {
    if let Some(host) = feed.hosts.get_mut(id) {
        host.record_turn(at, &[asked], vec![text(answered)]);
    }
    super::pi_rows::thread(feed, id);
}

/// The turn the streaming worker is in when the preview opens: long enough to watch, cancel
/// or pause.
fn opening() -> Vec<Beat> {
    vec![
        say(
            "Preview · sample turn. Checking the week preset next: a week window should label each day once, with the month where it changes.",
        ),
        tool(
            "bash",
            "cargo test timeline::presets -- --nocapture",
            &[
                "   Compiling nsh-app-corral v0.1.0",
                "    Finished test profile in 12.1s",
                "running 9 tests",
                "test timeline::presets::hour_labels_minutes ... ok",
                "test timeline::presets::day_labels_hours ... ok",
                "test timeline::presets::week_labels_days ... ok",
                "test timeline::presets::week_names_month_change ... ok",
                "test timeline::presets::week_crosses_dst ... ok",
                "test timeline::presets::month_labels_weeks ... ok",
                "test timeline::presets::zone_follows_device ... ok",
                "test timeline::presets::utc_device ... ok",
                "test timeline::presets::empty_window ... ok",
                "test result: ok. 9 passed; 0 failed; 0 ignored",
            ],
        ),
        say(
            "Every preset labels real dates, including a week that crosses the clock change. Writing the summary before I call corral done.",
        ),
        tool(
            "edit",
            "src/screens/timeline/chart.rs",
            &["+22 −9", "label every day once; month where it changes"],
        ),
        say("Done here. The summary says what changed and which tests cover it."),
    ]
}
