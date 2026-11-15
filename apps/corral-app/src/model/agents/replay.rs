//! A session's recorded transcript as the terminal UI replays it, then its live turn. Every
//! serialized `SessionItem` maps to a view-model here (tool rows in `replay_tool`, markers,
//! cards, images and the footer in `replay_marks`); a preview followed by its continuation
//! shows the complete item once it arrives, and words are fetched as soon as they are shown.

use std::collections::BTreeSet;

use corral_protocol::detail::{
    DetailRef, QuestionAnswer, QuestionOption, SessionItem, SessionMessage,
};

use crate::model::Id;
use crate::model::daemon::{self, DetailCache};
use crate::model::hue::Hue;
use crate::model::session_feed::{ItemFetch, LiveSession};
use crate::model::text::{Rich, span};

use super::live::clock;
use super::live_session::{failure, older_token, page};
use super::{
    Action, Fill, Marker, Message, MessageItem, More, Prose, QuestionCard, State, Thinking,
    Transcript, replay_marks, replay_tool,
};

/// What every item mapping reads: the session, the owner's open rows and the feed.
#[derive(Clone, Copy)]
pub(super) struct Cx<'a> {
    pub id: &'a Id,
    pub state: &'a State,
    pub feed: &'a daemon::Feed,
}

impl Cx<'_> {
    /// Whether the owner opened the row with this key.
    pub fn open(&self, key: &str) -> bool {
        self.state.expanded.contains(key)
    }
}

/// How much of one recorded item is here, by its continuation.
#[derive(Clone, Copy)]
pub(super) enum Rest<'a> {
    Whole,
    Missing {
        token: &'a str,
        bytes: u64,
    },
    Loading {
        bytes: u64,
    },
    Failed {
        token: &'a str,
        bytes: u64,
        reason: &'a str,
    },
}

impl<'a> Rest<'a> {
    /// The token to fetch when the owner opens the row: one never asked for, or one that failed.
    fn token(self) -> Option<&'a str> {
        match self {
            Rest::Missing { token, .. } | Rest::Failed { token, .. } => Some(token),
            Rest::Whole | Rest::Loading { .. } => None,
        }
    }

    pub fn more(self, cx: Cx<'_>) -> More {
        match self {
            Rest::Whole => More::Whole,
            Rest::Missing { bytes, .. } => More::Preview { size: size(bytes) },
            Rest::Loading { bytes } => More::Loading { size: size(bytes) },
            Rest::Failed {
                token,
                bytes,
                reason,
            } => More::Failed {
                size: size(bytes),
                reason: reason.to_owned(),
                retry: Action::Fetch {
                    session: cx.id.clone(),
                    token: token.to_owned(),
                },
            },
        }
    }

    /// Opens or folds the row `key`; opening also fetches what only a preview stands for.
    pub fn toggle(self, key: &str, cx: Cx<'_>) -> Action {
        Action::Expand {
            key: key.to_owned(),
            fetch: self.token().map(|token| (cx.id.clone(), token.to_owned())),
        }
    }

    /// Fetches the complete item, when it was never asked for or its read failed.
    pub fn fetch(self, cx: Cx<'_>) -> Option<Action> {
        self.token().map(|token| Action::Fetch {
            session: cx.id.clone(),
            token: token.to_owned(),
        })
    }
}

/// Each item with what is here of it. A preview followed by its continuation is replaced by
/// the complete item once fetched; the continuation itself is not an item to draw.
pub(super) fn resolve<'a>(
    items: &'a [SessionItem],
    feed: &'a daemon::Feed,
) -> Vec<(&'a SessionItem, Rest<'a>)> {
    let mut out = Vec::with_capacity(items.len());
    let mut at = 0;
    while let Some(item) = items.get(at) {
        at += 1;
        if matches!(item, SessionItem::Continuation { .. }) {
            // A continuation belongs to the item before it; one without is nothing to draw.
            continue;
        }
        let Some(SessionItem::Continuation { token, bytes }) = items.get(at) else {
            out.push((item, Rest::Whole));
            continue;
        };
        at += 1;
        let bytes = *bytes;
        out.push(match feed.items.get(token) {
            Some(ItemFetch::Ready(full)) => (full, Rest::Whole),
            Some(ItemFetch::Loading) => (item, Rest::Loading { bytes }),
            Some(ItemFetch::Failed(reason)) => (
                item,
                Rest::Failed {
                    token,
                    bytes,
                    reason,
                },
            ),
            None => (item, Rest::Missing { token, bytes }),
        });
    }
    out
}

/// A byte count in words: `820 B`, `1.2 KB`, `34 KB`, `1.2 MB`.
pub(super) fn size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < 10.0 * KB {
        format!("{:.1} KB", value / KB)
    } else if value < KB * KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{:.1} MB", value / (KB * KB))
    }
}

/// `n line(s)`.
pub(super) fn lines(n: usize) -> String {
    if n == 1 {
        "1 line".to_owned()
    } else {
        format!("{n} lines")
    }
}

/// The first line of `text`, and how many lines follow it.
pub(super) fn first_line(text: &str) -> (&str, usize) {
    let mut rows = text.trim().lines();
    let first = rows.next().unwrap_or_default();
    (first, rows.count())
}

/// The avatar letter and hue of a transcript author.
fn avatar(author: &str) -> (&'static str, Hue) {
    let who = author.to_ascii_lowercase();
    if who.starts_with("corral") || who == "system" {
        ("C", Hue::Corral)
    } else if who.starts_with("owner") || who == "you" || who == "user" {
        ("Y", Hue::You)
    } else if who == "pm" || who.starts_with("pm ") {
        ("P", Hue::Corral)
    } else if who.starts_with("assistant") {
        ("A", Hue::Worker)
    } else if who.starts_with("gate") {
        ("G", Hue::Corral)
    } else if who.starts_with("reviewer") {
        ("R", Hue::Worker)
    } else {
        ("W", Hue::Worker)
    }
}

/// The inline handover for a question the agent asked, open or settled.
fn question(
    key: &str,
    text: &str,
    options: &[QuestionOption],
    answer: Option<&QuestionAnswer>,
    feed: &daemon::Feed,
) -> QuestionCard {
    let recommended = options
        .iter()
        .position(|option| option.recommended)
        .map(|at| format!(" · recommends {}", at + 1))
        .unwrap_or_default();
    let count = match options.len() {
        0 => "free answer".to_owned(),
        1 => "1 option".to_owned(),
        n => format!("{n} options"),
    };
    match answer {
        None => QuestionCard {
            label: format!("{key} · waiting on you"),
            question: text.to_owned(),
            button: "Open handover · decide",
            hint: format!("{count}{recommended}"),
            open: true,
            action: Action::OpenQuestion(Id::from(key)),
        },
        Some(answer) => QuestionCard {
            label: format!("{key} · answered by {}", answer.by),
            question: text.to_owned(),
            button: "Open handover",
            hint: [
                clock(Some(answer.at.as_str()), feed),
                Some(answer.option.clone().unwrap_or_else(|| answer.text.clone())),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
            open: false,
            action: Action::OpenQuestion(Id::from(key)),
        },
    }
}

/// Thinking: shown unless the owner folded it.
pub(super) fn thinking(
    text: &str,
    key: String,
    more: More,
    streaming: bool,
    cx: Cx<'_>,
) -> Thinking {
    let fold = format!("{key}|fold");
    let (_, rest) = first_line(text);
    Thinking {
        text: text.to_owned(),
        open: !cx.open(&fold),
        hidden: format!("hidden · {}", lines(rest + 1)),
        more,
        streaming,
        action: Action::Expand {
            key: fold,
            fetch: None,
        },
    }
}

/// The header of a message: author, then its time.
fn head(author: &str, at: Option<&str>) -> Rich {
    let mut head: Rich = vec![span(author.to_owned()).bold()];
    if let Some(at) = at {
        head.push(span(format!(" · {at}")).dim());
    }
    head
}

fn message(
    message: &SessionMessage,
    cx: Cx<'_>,
    pairs: &std::collections::BTreeMap<&str, super::replay_pairs::Pair<'_>>,
) -> Message {
    let (letter, hue) = avatar(&message.author);
    let author = if letter == "Y" {
        "you".to_owned()
    } else {
        message.author.clone()
    };
    let at = clock(message.at.as_deref(), cx.feed);
    // Positions inside one recorded message are stable across pages and refreshes; the message
    // is known by its time and author.
    let base = format!(
        "{}|{}",
        message.at.as_deref().unwrap_or_default(),
        message.author
    );
    let mut items = Vec::new();
    let mut facts = None;
    for (n, (item, rest)) in resolve(&message.items, cx.feed).into_iter().enumerate() {
        let key = format!("{base}|{n}");
        match item {
            SessionItem::Text { text } => items.push(MessageItem::Prose(Prose {
                markdown: text.clone(),
                more: rest.more(cx),
                streaming: false,
            })),
            SessionItem::Thinking { text } => items.push(MessageItem::Thinking(thinking(
                text,
                key,
                rest.more(cx),
                false,
                cx,
            ))),
            SessionItem::ToolCall { call_id, .. } => {
                if let Some(pair) = pairs.get(call_id.as_str()) {
                    if std::ptr::eq(item, pair.first) {
                        items.extend(
                            replay_tool::call(&pair.item, &key, pair.rest, cx)
                                .map(MessageItem::Tool),
                        );
                    }
                } else {
                    items.extend(replay_tool::call(item, &key, rest, cx).map(MessageItem::Tool));
                }
            }
            SessionItem::BashExecution { .. } => {
                items.extend(replay_tool::bash(item, &key, rest, cx).map(MessageItem::Tool));
            }
            SessionItem::QuestionCard {
                key: question_key,
                text,
                options,
                answer,
                ..
            } => items.push(MessageItem::Question(question(
                question_key,
                text,
                options,
                answer.as_ref(),
                cx.feed,
            ))),
            SessionItem::Image { .. } => {
                items.extend(replay_marks::picture(item, &key, rest, cx).map(MessageItem::Picture));
            }
            SessionItem::Compaction { .. } | SessionItem::BranchSummary { .. } => {
                items.extend(replay_marks::divider(item, &key, rest, cx).map(MessageItem::Divider));
            }
            SessionItem::ModelChange { .. }
            | SessionItem::ThinkingLevelChange { .. }
            | SessionItem::Label { .. }
            | SessionItem::Aborted { .. }
            | SessionItem::ProviderError { .. } => {
                items.extend(replay_marks::marker(item, &key, rest, cx).map(MessageItem::Marker));
            }
            SessionItem::Card { title, .. } if title == replay_marks::TURN_FOOTER => {
                facts = Some((item, key, rest));
            }
            SessionItem::Card { .. } => items.extend(replay_marks::card(item, &key, rest, cx)),
            // `resolve` folds each continuation into the item before it.
            SessionItem::Continuation { .. } => {}
            SessionItem::Other => items.push(MessageItem::Marker(Marker {
                glyph: "○",
                hue: Hue::Neutral,
                words: "unsupported item · this version of the app cannot show it".into(),
                open: false,
                body: Vec::new(),
                more: More::Whole,
                action: None,
            })),
        }
    }
    Message {
        avatar: letter,
        hue,
        head: head(&author, at.as_deref()),
        author,
        detail: at.unwrap_or_default(),
        items,
        footer: replay_marks::footer(message, facts, cx),
        live: false,
    }
}

/// The continuation tokens of the words on the shown page that are not fetched yet: text and
/// thinking show in full, as the terminal shows them, so they are fetched once on screen. Tool
/// output, arguments, diffs, card data and images wait until the owner opens them.
pub fn fetches(feed: &daemon::Feed, id: &Id) -> Vec<String> {
    let Some(page) = page(feed.detail(&DetailRef::Session { id: id.to_string() })) else {
        return Vec::new();
    };
    page.messages
        .iter()
        .flat_map(|message| message.items.windows(2))
        .filter_map(|pair| match pair {
            [
                SessionItem::Text { .. } | SessionItem::Thinking { .. },
                SessionItem::Continuation { token, .. },
            ] if !feed.items.contains_key(token) => Some(token.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn transcript(
    id: &Id,
    state: &State,
    cache: Option<&DetailCache>,
    feed: &daemon::Feed,
) -> Transcript {
    let cx = Cx { id, state, feed };
    let body = page(cache);
    let loading = cache.is_none_or(|cache| cache.loading);
    let earlier = state.earlier.contains(id.as_str());
    let fill = match body {
        Some(page) if page.messages.is_empty() && !feed.live.contains_key(id.as_str()) => {
            Fill::Empty {
                heading: "No transcript yet",
                paragraph: "corrald has recorded no messages for this session.".into(),
            }
        }
        Some(_) => Fill::Ready,
        None => match failure(cache) {
            Some(why) if !loading => Fill::Failed(format!(
                "The transcript could not be loaded: {why}. It loads again when the session changes or corrald reconnects."
            )),
            _ => Fill::Loading,
        },
    };
    let stale_note = body.and(cache).and_then(|cache| {
        if let Some(why) = failure(Some(cache)) {
            Some(format!(
                "Showing the last transcript received; refreshing failed: {why}."
            ))
        } else {
            (cache.stale && !cache.loading).then(|| {
                format!(
                    "Showing the transcript as of {}; corrald is not answering.",
                    feed.synced.as_deref().unwrap_or("the last update")
                )
            })
        }
    });
    let recorded: &[SessionMessage] = body.map_or(&[], |page| page.messages.as_slice());
    let pairs = super::replay_pairs::collect(recorded, feed);
    // The live turn follows the latest messages; an earlier page is not where it stands.
    let (live, after) = match feed.live.get(id.as_str()) {
        Some(turn) if !earlier && body.is_some() => overlay(turn, recorded, cx),
        _ => (Vec::new(), Vec::new()),
    };
    Transcript {
        messages: recorded
            .iter()
            .map(|m| message(m, cx, &pairs))
            .filter(|message| !message.items.is_empty() || message.footer.is_some())
            .collect(),
        older: cache
            .and_then(older_token)
            .map(|_| Action::Older(id.clone())),
        newer: earlier.then(|| Action::Newer(id.clone())),
        paging: body.is_some() && loading,
        fill,
        stale: stale_note,
        disclaimer: None,
        live,
        after,
    }
}

/// The open turn laid over the recorded messages: its settled messages, its tool calls, then
/// the message still being written, as one streaming assistant message; then the owner input
/// the host holds and how the turn ended. A tool call the recorded page already holds is drawn
/// there only, and an ending the recorded transcript states is not said twice.
fn overlay(
    turn: &LiveSession,
    recorded: &[SessionMessage],
    cx: Cx<'_>,
) -> (Vec<Message>, Vec<MessageItem>) {
    let calls: BTreeSet<&str> = recorded
        .iter()
        .flat_map(|message| &message.items)
        .filter_map(|item| match item {
            SessionItem::ToolCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    let base = format!(
        "live|{}|{}",
        turn.generation.unwrap_or(0),
        turn.turn.unwrap_or(0)
    );
    let mut items = words(turn, true, &base, cx);
    items.extend(
        turn.tools
            .iter()
            .filter(|tool| !calls.contains(tool.call.as_str()))
            .map(|tool| MessageItem::Tool(replay_tool::live(tool, cx))),
    );
    items.extend(words(turn, false, &base, cx));
    let live = if items.is_empty() {
        Vec::new()
    } else {
        let (avatar, hue) = avatar("assistant");
        let mut head = head("assistant", None);
        head.push(
            span(if turn.open {
                " · working now"
            } else {
                " · not recorded yet"
            })
            .dim(),
        );
        vec![Message {
            avatar,
            hue,
            head,
            author: "assistant".into(),
            detail: String::new(),
            items,
            footer: None,
            live: true,
        }]
    };
    let mut after = Vec::new();
    if let Some(held) = queued(turn.steer, turn.follow_up) {
        after.push(MessageItem::Marker(quiet("→", Hue::Neutral, held)));
    }
    if let Some(end) = turn.ended.as_ref().filter(|_| !turn.open) {
        // The latest recorded assistant message says how its turn ended once it is recorded.
        let last = recorded
            .iter()
            .rev()
            .find(|message| message.author == "assistant")
            .map_or(&[][..], |message| message.items.as_slice());
        let recorded_as = |aborted: bool| {
            last.iter().any(|item| match item {
                SessionItem::Aborted { .. } => aborted,
                SessionItem::ProviderError { .. } => !aborted,
                _ => false,
            })
        };
        if let Some(error) = &end.error {
            if !recorded_as(false) {
                after.push(MessageItem::Marker(quiet("✗", Hue::Fail, error.clone())));
            }
        } else if end.aborted && !recorded_as(true) {
            after.push(MessageItem::Marker(quiet(
                "○",
                Hue::Neutral,
                "The turn was stopped.".into(),
            )));
        }
    }
    (live, after)
}

/// The live turn's messages that are settled (or still being written): thinking, then words.
fn words(turn: &LiveSession, settled: bool, base: &str, cx: Cx<'_>) -> Vec<MessageItem> {
    let mut items = Vec::new();
    for message in turn
        .messages
        .iter()
        .filter(|message| message.settled_at.is_some() == settled)
    {
        let streaming = !settled;
        if !message.thinking.is_empty() {
            items.push(MessageItem::Thinking(thinking(
                &message.thinking,
                format!("{base}|{}", message.ordinal),
                More::Whole,
                streaming,
                cx,
            )));
        }
        if !message.text.is_empty() {
            items.push(MessageItem::Prose(Prose {
                markdown: message.text.clone(),
                more: More::Whole,
                streaming,
            }));
        }
    }
    items
}

/// A quiet row with nothing more to open.
fn quiet(glyph: &'static str, hue: Hue, words: String) -> Marker {
    Marker {
        glyph,
        hue,
        words,
        open: false,
        body: Vec::new(),
        more: More::Whole,
        action: None,
    }
}

/// Owner input the host holds: `1 message steers this turn · 2 wait for the next`.
fn queued(steer: u64, follow_up: u64) -> Option<String> {
    let steer = match steer {
        0 => None,
        1 => Some("1 message steers this turn".to_owned()),
        n => Some(format!("{n} messages steer this turn")),
    };
    let follow = match follow_up {
        0 => None,
        1 => Some("1 message waits for the next turn".to_owned()),
        n => Some(format!("{n} messages wait for the next turn")),
    };
    let words: Vec<String> = steer.into_iter().chain(follow).collect();
    (!words.is_empty()).then(|| format!("queued · {}", words.join(" · ")))
}
