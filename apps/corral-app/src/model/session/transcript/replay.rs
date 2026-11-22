//! A session's recorded transcript as the terminal UI replays it, then its live turn. Every
//! serialized `SessionItem` maps to a view-model here (tool rows in `tool`, cells, build cards,
//! markers, cards, images and the footer in their modules); a preview followed by its
//! continuation shows the complete item once it arrives, and words are fetched as soon as they
//! are shown. Consecutive messages of one agent read as one, so a run of calls folds into one
//! group; what the host made reads as quiet rows.

use corral_protocol::detail::{
    DetailBody, DetailRef, QuestionAnswer, QuestionOption, SessionItem, SessionMessage,
    SessionToolState,
};
use corral_protocol::session_state::InputOrigin;
use std::collections::BTreeSet;

use crate::model::Id;
use crate::model::agents::clock;
use crate::model::daemon::{self, DetailCache};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

use super::super::{Ui, echo};
use super::items::Answered;
use super::journal::Journal;
use super::live::ItemFetch;
use super::marks::Turn;
use super::{
    Action, EXPAND_ALL, Fill, Marker, Message, MessageItem, MessageKind, More, Prose, QuestionCard,
    Section, TagFacts, Thinking, Transcript, bubbles, failure, group, marks, older_token, overlay,
    page, tool, tool_jobs,
};

/// What every item mapping reads: the session, the owner's open rows, the feed and what the
/// page's journal entries say.
#[derive(Clone, Copy)]
pub(super) struct Cx<'a> {
    pub id: &'a Id,
    pub ui: &'a Ui,
    pub feed: &'a daemon::Feed,
    pub journal: &'a Journal,
}

impl Cx<'_> {
    /// Whether the row with this key shows opened: the owner opened it, or opened every row
    /// and did not fold this one.
    pub fn open(&self, key: &str) -> bool {
        opened(self.ui, key)
    }
}

fn opened(ui: &Ui, key: &str) -> bool {
    ui.expanded.contains(key) != ui.expanded.contains(EXPAND_ALL)
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
            session: cx.id.clone(),
            key: key.to_owned(),
            fetch: self.token().map(str::to_owned),
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
    out.extend(resolve_at(items, feed).map(|(_, item, rest)| (item, rest)));
    out
}

/// Also retain the raw index for locating an item on its original page, including when a
/// record was split across pages or a continuation has already been fetched.
pub(super) fn resolve_at<'a>(
    items: &'a [SessionItem],
    feed: &'a daemon::Feed,
) -> impl Iterator<Item = (usize, &'a SessionItem, Rest<'a>)> {
    let mut items = items.iter().enumerate().peekable();
    std::iter::from_fn(move || {
        let (at, item) =
            items.find(|(_, item)| !matches!(item, SessionItem::Continuation { .. }))?;
        let Some((_, SessionItem::Continuation { token, bytes })) = items.peek().copied() else {
            return Some((at, item, Rest::Whole));
        };
        items.next();
        let bytes = *bytes;
        let (item, rest) = match feed.items.get(token) {
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
        };
        Some((at, item, rest))
    })
}

/// A byte count in words: `820 B`, `1.2 KB`, `34 KB`, `1.2 MB`.
pub fn size(bytes: u64) -> String {
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
pub(super) fn avatar(author: &str) -> (&'static str, Hue) {
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

/// Authors whose entries are the host's or the session's own, the owner's `!command` (a row
/// marked `you`) or what a tool returned beside its row: drawn with no avatar or header.
fn host_made(author: &str) -> bool {
    matches!(
        author,
        "system" | "custom" | "bashExecution" | "toolResult" | "tool"
    )
}

/// A question the agent asked, open or settled, as the transcript recorded it.
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
            hint: format!("{count}{recommended}"),
            open: true,
        },
        Some(answer) => QuestionCard {
            label: format!("{key} · answered by {}", answer.by),
            question: text.to_owned(),
            hint: [
                clock(Some(answer.at.as_str()), feed),
                Some(answer.option.clone().unwrap_or_else(|| answer.text.clone())),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
            open: false,
        },
    }
}

/// Thinking: shown unless the owner folded it, or hid every thinking block of the session;
/// then one row that shows them all again.
pub(super) fn thinking(
    text: &str,
    key: String,
    more: More,
    streaming: bool,
    cx: Cx<'_>,
) -> Thinking {
    let fold = format!("{key}|fold");
    let (_, rest) = first_line(text);
    if cx.ui.hide_thinking {
        return Thinking {
            text: text.to_owned(),
            open: false,
            hidden: "thinking hidden · Alt+H shows it".to_owned(),
            hidden_all: true,
            more,
            streaming,
            action: Action::Thinking(cx.id.clone()),
        };
    }
    Thinking {
        text: text.to_owned(),
        open: !cx.ui.expanded.contains(&fold),
        hidden: format!("hidden · {}", lines(rest + 1)),
        hidden_all: false,
        more,
        streaming,
        action: Action::Expand {
            session: cx.id.clone(),
            key: fold,
            fetch: None,
        },
    }
}

/// The header of an agent's message: author, then its time.
pub(super) fn head(author: &str, at: Option<&str>) -> Rich {
    let mut head: Rich = vec![span(author.to_owned()).bold()];
    if let Some(at) = at {
        head.push(span(format!(" · {at}")).dim());
    }
    head
}

/// An owner message's items and the input id an assistant forwarded with it: an `ask_user`
/// answer becomes its own bubble, a forwarded message loses its `[owner input …]` line.
fn owner_items(
    items: &mut [MessageItem],
    key: &str,
    cx: Cx<'_>,
) -> (Option<&'static str>, Option<String>) {
    let mut tag = None;
    let mut lead = None;
    for item in items.iter_mut() {
        let MessageItem::Prose(prose) = item else {
            continue;
        };
        let open = cx.open(key);
        if let Some(answers) = bubbles::answered(&prose.markdown, open) {
            lead = Some("you answered");
            *item = MessageItem::Answered(Answered {
                answers,
                open,
                action: Action::Expand {
                    session: cx.id.clone(),
                    key: key.to_owned(),
                    fetch: None,
                },
            });
            continue;
        }
        if let Some((words, id)) = bubbles::forwarded(&prose.markdown) {
            tag = Some(id.to_owned());
            prose.markdown = words.to_owned();
        }
    }
    (lead, tag)
}

fn anchor(message: &SessionMessage) -> String {
    if message.id.is_empty() {
        format!(
            "{}|{}",
            message.at.as_deref().unwrap_or_default(),
            message.author
        )
    } else {
        message.id.clone()
    }
}

fn item_key(anchor: &str, from_end: usize) -> String {
    // A prepend can add items at the front of a split record without changing these keys.
    format!("{anchor}|{from_end}")
}

fn message(
    message: &SessionMessage,
    cx: Cx<'_>,
    pairs: &std::collections::BTreeMap<&str, super::pairs::Pair<'_>>,
    first_user: bool,
) -> Message {
    let at = clock(message.at.as_deref(), cx.feed);
    let anchor = anchor(message);
    let resolved = resolve(&message.items, cx.feed);
    let turn = Turn {
        partial: resolved
            .iter()
            .any(|(item, _)| matches!(item, SessionItem::Text { text } if !text.trim().is_empty())),
        not_run: resolved
            .iter()
            .filter(|(item, _)| {
                matches!(
                    item,
                    SessionItem::ToolCall {
                        state: SessionToolState::NotRun,
                        ..
                    }
                )
            })
            .count(),
    };
    let mut items = Vec::new();
    let mut facts = None;
    let mut notice = false;
    let count = resolved.len();
    for (n, (item, rest)) in resolved.into_iter().enumerate() {
        let key = item_key(&anchor, count - n);
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
                    if std::ptr::eq(item, pair.position) {
                        items.extend(tool::call(&pair.item, &key, pair.rest, cx));
                    }
                } else {
                    items.extend(tool::call(item, &key, rest, cx));
                }
            }
            SessionItem::BashExecution { .. } => {
                items.extend(tool_jobs::owner(item, &key, rest, cx));
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
                items.extend(marks::picture(item, rest, cx).map(MessageItem::Picture));
            }
            SessionItem::Compaction { .. } | SessionItem::BranchSummary { .. } => {
                items.extend(marks::divider(item, &key, rest, cx).map(MessageItem::Divider));
            }
            SessionItem::ModelChange { .. }
            | SessionItem::ThinkingLevelChange { .. }
            | SessionItem::Label { .. }
            | SessionItem::Aborted { .. }
            | SessionItem::ProviderError { .. } => {
                items.extend(marks::marker(item, &key, rest, cx, turn).map(MessageItem::Marker));
            }
            SessionItem::Card { title, .. } if title == marks::TURN_FOOTER => {
                facts = Some((item, key, rest));
            }
            SessionItem::Card { title, text, .. } if title == "Host notice" => {
                notice = true;
                items.extend(bubbles::notice(text).into_iter().map(|mut row| {
                    let open = cx.open(&key);
                    row.action = Some(rest.toggle(&key, cx));
                    row.open = open;
                    row.more = rest.more(cx);
                    if !open {
                        row.body.clear();
                    }
                    MessageItem::Marker(row)
                }));
            }
            SessionItem::Card { .. } => items.extend(marks::card(item, &key, rest, cx)),
            // `resolve` folds each continuation into the item before it.
            SessionItem::Continuation { .. } => {}
            SessionItem::Other => items.push(MessageItem::Marker(Marker::quiet(
                "○",
                Hue::Neutral,
                "unsupported item · this version of the app cannot show it".into(),
            ))),
        }
    }
    let owner = avatar(&message.author).0 == "Y" && !notice;
    let quiet = notice || host_made(&message.author);
    let (letter, hue) = avatar(&message.author);
    if quiet {
        for item in &mut items {
            if let MessageItem::Marker(row) = item {
                row.time = at.clone();
            }
        }
        return Message {
            id: message.id.clone(),
            anchor,
            kind: MessageKind::Quiet,
            avatar: letter,
            hue,
            head: Vec::new(),
            author: message.author.clone(),
            detail: at.unwrap_or_default(),
            tag: None,
            tag_facts: None,
            items,
            footer: None,
            live: false,
        };
    }
    // What the host delivered for a child (its report, a message it sent the parent) is that
    // child's, not the owner's; owner-typed text is never read as one.
    let typed = message
        .input
        .as_ref()
        .is_some_and(|input| input.origin == InputOrigin::Owner);
    let said = items.iter().find_map(|item| match item {
        MessageItem::Prose(prose) => Some(prose.markdown.as_str()),
        _ => None,
    });
    if owner && !typed {
        if let Some(report) = said.and_then(bubbles::child_report) {
            let key = format!("{anchor}|report");
            let mut shown = bubbles::report_message(
                message.id.clone(),
                report,
                at.as_deref(),
                cx.open(&key),
                Action::Expand {
                    session: cx.id.clone(),
                    key,
                    fetch: None,
                },
            );
            shown.anchor = anchor;
            return shown;
        }
        if let Some((from, body)) = said.and_then(bubbles::peer_message) {
            let lead = vec![span(from.to_owned()).bold(), span(" → parent").dim()];
            let mut shown = bubbles::bubble(
                message.id.clone(),
                bubbles::head(lead, at.as_deref()),
                vec![MessageItem::Prose(Prose {
                    markdown: body.to_owned(),
                    more: More::Whole,
                    streaming: false,
                })],
            );
            shown.author = from.to_owned();
            shown.anchor = anchor;
            return shown;
        }
    }
    if owner {
        let (lead, forwarded) = owner_items(&mut items, &format!("{anchor}|answer"), cx);
        let head = if first_user && bubbles::child_scope(cx.id).is_some() {
            items.extend(bubbles::brief_note(cx.id, cx.feed));
            vec![span("brief from parent").dim()]
        } else {
            match lead {
                Some(lead) => vec![span(lead).dim()],
                None => bubbles::owner_lead(message.input.as_ref()),
            }
        };
        let mut shown = bubbles::bubble(
            message.id.clone(),
            bubbles::head(head, at.as_deref()),
            items,
        );
        shown.detail = at.unwrap_or_default();
        shown.anchor = anchor;
        match forwarded {
            Some(id) => shown.tag = Some(id),
            None => {
                // An answer the owner gave on its card names at most the device it came
                // from; the host delivering it is not the owner sending it automatically.
                let (tag, device) = message.input.as_ref().map_or((None, None), |input| {
                    if lead.is_some() {
                        bubbles::device_tag(input, cx.feed)
                    } else {
                        bubbles::source(input, cx.feed)
                    }
                });
                shown.tag = tag;
                shown.tag_facts = device.map(|device| {
                    let key = format!("{}|device", shown.anchor);
                    let open = cx.open(&key);
                    TagFacts {
                        open,
                        body: if open {
                            vec![Section::Text {
                                label: "sent from the device".into(),
                                text: device,
                            }]
                        } else {
                            Vec::new()
                        },
                        action: Action::Expand {
                            session: cx.id.clone(),
                            key,
                            fetch: None,
                        },
                    }
                });
            }
        }
        return shown;
    }
    Message {
        id: message.id.clone(),
        anchor,
        kind: MessageKind::Agent,
        avatar: letter,
        hue,
        head: head(&message.author, at.as_deref()),
        author: message.author.clone(),
        detail: at.unwrap_or_default(),
        tag: None,
        tag_facts: None,
        items,
        footer: marks::footer(message, facts, cx),
        live: false,
    }
}

/// Consecutive messages of one agent as one message, so its words and calls read as one reply
/// and a run of calls can fold; the merged message keeps the first's head and the latest foot.
/// Consecutive quiet rows stand together under one blank row.
fn merged(messages: Vec<(bool, Message)>) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    for (boundary, message) in messages {
        if !boundary
            && let Some(last) = out.last_mut()
            && last.kind == message.kind
            && match message.kind {
                MessageKind::Agent => last.author == message.author,
                MessageKind::Quiet => true,
                MessageKind::Bubble => false,
            }
        {
            last.items.extend(message.items);
            if message.footer.is_some() {
                last.footer = message.footer;
            }
            continue;
        }
        out.push(message);
    }
    out
}

/// The time a recorded message was written, in epoch milliseconds.
pub(super) fn millis(at: Option<&str>) -> Option<u64> {
    let stamp: jiff::Timestamp = at?.parse().ok()?;
    u64::try_from(stamp.as_millisecond()).ok()
}

/// Fetch complete content for words and special cards, and for rows already open when a page
/// adds or changes their continuation. A paired call and result can need separate fetches.
pub fn fetches(feed: &daemon::Feed, id: &Id, ui: &Ui) -> Vec<String> {
    let Some(page) = page(feed.detail(&DetailRef::Session { id: id.to_string() })) else {
        return Vec::new();
    };
    let mut fetches = Vec::new();
    for message in &page.messages {
        if !message
            .items
            .iter()
            .any(|item| matches!(item, SessionItem::Continuation { .. }))
        {
            continue;
        }
        let anchor = anchor(message);
        let items = resolve(&message.items, feed);
        let count = items.len();
        for (n, (item, rest)) in items.into_iter().enumerate() {
            let Rest::Missing { token, .. } = rest else {
                continue;
            };
            let wanted = match item {
                SessionItem::Text { .. } | SessionItem::Thinking { .. } => true,
                SessionItem::Image { .. } => false,
                SessionItem::ToolCall { name, call_id, .. } => {
                    matches!(
                        name.as_str(),
                        "task" | "build_finish" | "board_plan" | "codemode"
                    ) || if call_id.is_empty() {
                        opened(ui, &item_key(&anchor, count - n))
                    } else {
                        opened(ui, &format!("call:{call_id}"))
                    }
                }
                _ => opened(ui, &item_key(&anchor, count - n)),
            };
            if wanted {
                fetches.push(token.to_owned());
            }
        }
    }
    fetches
}

/// The transcript of session `id` (the scope its page reads, `quick` included) as `ui` shows it.
pub fn transcript(
    id: &Id,
    ui: &Ui,
    cache: Option<&DetailCache>,
    feed: &daemon::Feed,
) -> Transcript {
    let body = page(cache);
    let recorded: &[SessionMessage] = body.map_or(&[], |page| page.messages.as_slice());
    let journal = Journal::of(id, recorded, feed);
    let cx = Cx {
        id,
        ui,
        feed,
        journal: &journal,
    };
    let loading = cache.is_none_or(|cache| cache.loading);
    let fill = match body {
        Some(page)
            if page.messages.is_empty()
                && page.older.is_none()
                && !feed.live.contains_key(id.as_str()) =>
        {
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
            _ => Fill::Loading("Loading the transcript…"),
        },
    };
    let stale_note = body.and(cache).and_then(|cache| {
        if let Some(error) = &cache.error
            && error.code == "history_changed"
        {
            Some(error.message.clone())
        } else if let Some(why) = failure(Some(cache)) {
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
    let pairs = super::pairs::collect(recorded, cache, feed);
    let first_user = recorded
        .iter()
        .position(|message| avatar(&message.author).0 == "Y");
    // Keep page seams so adjacent assistant replies do not swallow the previous first
    // message's scroll anchor when the reader adds a page above it.
    let boundaries: BTreeSet<_> = cache
        .into_iter()
        .flat_map(DetailCache::bodies)
        .filter_map(|body| match body {
            DetailBody::Session(page) => page.messages.first(),
            _ => None,
        })
        .map(|row| row.id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    let mut boundary = false;
    // A split record keeps its anchor but gains items at its front. Track the first recorded
    // row before adding child steers or mail, which can stay ahead of it across pages.
    let mut first = None;
    let mut timed: Vec<_> = recorded
        .iter()
        .enumerate()
        .filter_map(|(n, m)| {
            boundary |= boundaries.contains(m.id.as_str());
            let message = message(m, cx, &pairs, Some(n) == first_user);
            if message.items.is_empty() && message.footer.is_none() {
                return None;
            }
            first.get_or_insert_with(|| item_key(&message.anchor, m.items.len()));
            Some((
                millis(m.at.as_deref()),
                std::mem::take(&mut boundary),
                message,
            ))
        })
        .collect();
    // A child conversation's steers and agent messages stand before the first message that
    // started after them; those without a time go last.
    let at_clock = |ms: u64| {
        let at = jiff::Timestamp::from_millisecond(i64::try_from(ms).ok()?).ok()?;
        clock(Some(&at.to_string()), feed)
    };
    for (at, bubble) in bubbles::child_bubbles(id, feed, at_clock) {
        let place = at.and_then(|at| {
            timed
                .iter()
                .position(|(when, _, _)| when.is_some_and(|when| when > at))
        });
        timed.insert(place.unwrap_or(timed.len()), (at, false, bubble));
    }
    let messages: Vec<Message> = merged(
        timed
            .into_iter()
            .map(|(_, seam, message)| (seam, message))
            .collect(),
    )
    .into_iter()
    .map(|mut message| {
        message.items = group::fold(std::mem::take(&mut message.items), cx);
        message
    })
    .collect();
    // Older pages prepend; the live turn always follows the newest recorded messages.
    let (live, after) = match feed.live.get(id.as_str()) {
        Some(turn) if body.is_some() => overlay::overlay(turn, recorded, cx),
        _ => (Vec::new(), Vec::new()),
    };
    // Past the history cap the loaded pages no longer join the newest one: only the latest
    // messages can be read, never the page before these.
    let capped = cache
        .and_then(|cache| cache.error.as_ref())
        .is_some_and(|error| error.code == "history_changed");
    let older_cursor = cache.and_then(older_token).filter(|_| !capped);
    Transcript {
        echoes: echo::shown(id, ui, body, feed),
        messages,
        first,
        older: older_cursor.as_ref().map(|_| Action::Older(id.clone())),
        older_cursor,
        newer: None,
        paging: body.is_some() && loading,
        fill,
        stale: stale_note,
        disclaimer: None,
        live,
        after,
        scope: id.clone(),
        reading: ui.reading.clone(),
    }
}
