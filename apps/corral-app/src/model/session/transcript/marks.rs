//! Everything in a replay that is not words or a tool row: images, the compaction and branch
//! dividers, quiet rows (model and effort changes, bookmarks, stopped and failed turns, run
//! marks, owner questions), presenter cards and the assistant turn's footer. Quiet rows follow
//! the terminal's rule: a glyph that says the state without its colour, muted words, and on
//! opening the full text and the recorded facts. A failed turn says what happened, what is
//! lost and the next step, as `error-copy.ts` words it from the typed provider failure.

use corral_protocol::detail::{SessionItem, SessionMessage};
use serde_json::Value;

use crate::model::hue::Hue;

use super::journal::{FACT_ENTRIES, Journal};
use super::replay::{Cx, Rest, first_line, lines, size};
use super::tool::{elapsed, pretty};
use super::{Card, Divider, Footer, Marker, MessageItem, More, Picture, Section};

/// The card that carries an assistant turn's complete metadata; it is the footer's detail.
pub(super) const TURN_FOOTER: &str = "Turn footer";

/// Images a `data:` URI may carry: raster formats only, nothing that runs.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Stop reasons of a turn that ended as turns normally do; any other is said in the footer.
const NORMAL_STOPS: [&str; 4] = ["stop", "toolUse", "tool_use", "end_turn"];

/// The texts Pi and its providers give a turn the owner stopped.
const STOP_TEXTS: [&str; 4] = [
    "Operation aborted",
    "Request was aborted",
    "The operation was aborted.",
    "Retry cancelled",
];

/// What a failed or stopped turn's row needs from the rest of its message.
#[derive(Clone, Copy, Default)]
pub(super) struct Turn {
    /// The reply had words before it stopped.
    pub partial: bool,
    /// Tool calls of the turn that did not run.
    pub not_run: usize,
}

/// Recorded text under a label, unless there is none.
fn text(label: &str, body: &str) -> Option<Section> {
    (!body.trim().is_empty()).then(|| Section::Text {
        label: label.to_owned(),
        text: body.to_owned(),
    })
}

/// Token counts in words: `820`, `12.3k`, `1.2M`.
fn tokens(count: f64) -> String {
    if count < 1000.0 {
        format!("{count:.0}")
    } else if count < 1_000_000.0 {
        format!("{:.1}k", count / 1000.0)
    } else {
        format!("{:.1}M", count / 1_000_000.0)
    }
}

/// US dollars: `$0.42`, small amounts to four places (`$0.0042`).
fn dollars(usd: f64) -> String {
    if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

fn number(value: &Value, field: &str) -> Option<f64> {
    value.get(field).and_then(Value::as_f64)
}

/// Usage as Pi records it (`input`, `output`, `cacheRead`, `cacheWrite`, `cost.total`): tokens
/// in and out, cache reads and writes, cost. A shape without these fields says nothing.
fn usage(usage: &Value) -> Vec<String> {
    let mut words = Vec::new();
    if let (Some(input), Some(output)) = (number(usage, "input"), number(usage, "output")) {
        words.push(format!("{} in · {} out", tokens(input), tokens(output)));
        let read = number(usage, "cacheRead").unwrap_or(0.0);
        let write = number(usage, "cacheWrite").unwrap_or(0.0);
        if read > 0.0 || write > 0.0 {
            words.push(format!(
                "cache {} read · {} written",
                tokens(read),
                tokens(write)
            ));
        }
    }
    if let Some(cost) = usage.get("cost").and_then(|cost| number(cost, "total"))
        && cost > 0.0
    {
        words.push(dollars(cost));
    }
    words
}

/// The assistant message's foot: model, an unusual stop and usage; opened, every fact the turn
/// recorded (its `Turn footer` card).
pub(super) fn footer(
    message: &SessionMessage,
    facts: Option<(&SessionItem, String, Rest<'_>)>,
    cx: Cx<'_>,
) -> Option<Footer> {
    let mut words: Vec<String> = message.model.iter().cloned().collect();
    if let Some(stop) = message
        .stop_reason
        .as_deref()
        .filter(|stop| !NORMAL_STOPS.contains(stop))
    {
        words.push(format!("stop: {stop}"));
    }
    words.extend(message.usage.as_ref().map(usage).unwrap_or_default());
    let Some((item, key, rest)) = facts else {
        return (!words.is_empty()).then(|| Footer {
            words: words.join(" · "),
            open: false,
            body: Vec::new(),
            more: More::Whole,
            action: None,
        });
    };
    let open = cx.open(&key);
    let body = match (item, open) {
        (
            SessionItem::Card {
                text: said, data, ..
            },
            true,
        ) => [
            text("turn", said),
            data.as_deref()
                .and_then(|data| text("turn facts", &pretty(data))),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => Vec::new(),
    };
    Some(Footer {
        words: if words.is_empty() {
            "turn facts".to_owned()
        } else {
            words.join(" · ")
        },
        open,
        body,
        more: rest.more(cx),
        action: Some(rest.toggle(&key, cx)),
    })
}

/// The decoded size of base64 `data`, without decoding it.
fn decoded(data: &str) -> Option<u64> {
    let length = data.len();
    if length % 4 != 0 {
        return None;
    }
    let padding = data
        .bytes()
        .rev()
        .take(2)
        .filter(|byte| *byte == b'=')
        .count();
    u64::try_from(length / 4 * 3 - padding).ok()
}

/// An image the session recorded: drawn once its complete bytes are here and match the size
/// the record states; until then named, with its size, and fetched when asked for.
pub(super) fn picture(item: &SessionItem, rest: Rest<'_>, cx: Cx<'_>) -> Option<Picture> {
    let SessionItem::Image {
        media_type,
        size: bytes,
        encoding,
        data,
    } = item
    else {
        return None;
    };
    let label = format!("{media_type} · {}", size(*bytes));
    if !matches!(rest, Rest::Whole) {
        return Some(Picture {
            label,
            uri: None,
            note: None,
            more: rest.more(cx),
            action: rest.fetch(cx),
        });
    }
    let note = if encoding != "base64" {
        Some(format!(
            "This image is in an encoding the app cannot draw ({encoding})."
        ))
    } else if !IMAGE_TYPES.contains(&media_type.as_str()) {
        Some("The app draws PNG, JPEG, GIF and WebP images only.".to_owned())
    } else if decoded(data) != Some(*bytes) {
        Some("The image's data does not match its recorded size.".to_owned())
    } else {
        None
    };
    Some(Picture {
        label,
        uri: note
            .is_none()
            .then(|| format!("data:{media_type};base64,{data}")),
        note,
        more: More::Whole,
        action: None,
    })
}

/// A compaction or branch summary: a rule with words in it; opened, the summary.
pub(super) fn divider(
    item: &SessionItem,
    key: &str,
    rest: Rest<'_>,
    cx: Cx<'_>,
) -> Option<Divider> {
    let open = cx.open(key);
    let (words, note, summary) = match item {
        SessionItem::Compaction {
            summary,
            tokens_before,
        } => (
            format!(
                "context compacted · {} tokens before",
                tokens(*tokens_before as f64)
            ),
            if open {
                "Everything above stays readable; the agent keeps only the summary below."
            } else {
                "Everything above stays readable; the agent keeps only a summary."
            },
            summary,
        ),
        SessionItem::BranchSummary { summary, from_id } => (
            match from_id {
                Some(from) => format!("branch summary · from {from}"),
                None => "branch summary".to_owned(),
            },
            "The session continued from another branch; it carries this summary.",
            summary,
        ),
        _ => return None,
    };
    Some(Divider {
        words,
        note: note.to_owned(),
        open,
        body: if open && !summary.trim().is_empty() {
            vec![Section::Prose(summary.clone())]
        } else {
            Vec::new()
        },
        more: rest.more(cx),
        action: rest.toggle(key, cx),
    })
}

/// A quiet row whose opened body is `body`; it opens only when there is more to show than
/// its words.
fn quiet(
    glyph: &'static str,
    hue: Hue,
    words: String,
    body: Vec<Section>,
    key: &str,
    rest: Rest<'_>,
    cx: Cx<'_>,
) -> Marker {
    let open = cx.open(key);
    let opens = !body.is_empty() || !matches!(rest, Rest::Whole);
    Marker {
        open: open && opens,
        body: if open { body } else { Vec::new() },
        more: rest.more(cx),
        action: opens.then(|| rest.toggle(key, cx)),
        ..Marker::quiet(glyph, hue, words)
    }
}

/// A message's first line, and how many more lines it has, as one row's words.
fn headline(message: &str) -> String {
    match first_line(message) {
        (first, 0) => first.to_owned(),
        (first, more) => format!("{first} · {} more", lines(more)),
    }
}

/// The raw text and the recorded facts, shown opened, never cut.
fn told(message: &str, facts: Option<&str>) -> Vec<Section> {
    [
        text("message", message),
        facts.and_then(|facts| text("failure facts", &pretty(facts))),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The connection's name as the owner knows it.
fn connection(name: &str) -> String {
    match name {
        "openai-subscription" => "OpenAI subscription",
        "openai-api" => "OpenAI API",
        "anthropic-subscription" => "Claude subscription",
        "anthropic-api" => "Anthropic API",
        "amazon-bedrock" => "Amazon Bedrock",
        other => other,
    }
    .to_owned()
}

/// A failed turn's sentence and next step, from the typed provider failure when the turn
/// recorded one; an unknown text keeps its own words.
fn failure(message: &str, facts: Option<&str>, turn: Turn) -> (String, Option<String>, bool) {
    let lost = if turn.partial {
        "The reply above stopped early."
    } else {
        "Nothing was lost."
    };
    let typed = facts.and_then(|facts| serde_json::from_str::<Value>(facts).ok());
    let category = typed
        .as_ref()
        .and_then(|failure| failure.get("category").and_then(Value::as_str));
    let Some(category) = category else {
        let words = message.trim();
        return if words.is_empty() {
            (
                format!("The turn failed and gave no reason. {lost}"),
                Some("Send a message to retry.".to_owned()),
                false,
            )
        } else {
            (words.to_owned(), None, true)
        };
    };
    let failure = typed.as_ref().unwrap_or(&Value::Null);
    let name = connection(
        failure
            .get("connection")
            .and_then(Value::as_str)
            .unwrap_or("provider"),
    );
    let reference = failure
        .get("request_ref")
        .and_then(Value::as_str)
        .map(|reference| format!(" (ref {})", reference.chars().take(8).collect::<String>()))
        .unwrap_or_default();
    let until = failure
        .get("reset_at")
        .and_then(Value::as_str)
        .and_then(|at| at.parse::<jiff::Timestamp>().ok())
        .map(|at| {
            let local = at.to_zoned(jiff::tz::TimeZone::system());
            format!("{:02}:{:02}", local.hour(), local.minute())
        });
    let model_scope = failure.get("scope").and_then(Value::as_str) == Some("model");
    let not_sent = if turn.partial {
        "The reply above stopped early."
    } else {
        "Nothing was sent to the model."
    };
    let (sentence, step) = match category {
        "transient" => (
            format!("{name} is overloaded or limiting requests{reference}. {lost}"),
            match &until {
                Some(until) => format!(
                    "Wait until {until}, then send a message to retry, or choose another model."
                ),
                None => "Send a message to retry in a moment, or choose another model.".into(),
            },
        ),
        "subscription_quota" => (
            format!(
                "The {name} usage limit{} is reached. {lost}",
                if model_scope { " for this model" } else { "" }
            ),
            match &until {
                Some(until) => {
                    format!("It resets at {until}: wait until then, or choose another model.")
                }
                None => "Wait for the limit to reset, or choose another model.".into(),
            },
        ),
        "auth" => (
            format!("{name} refused the sign-in for this account. {lost}"),
            "Sign in again, or choose another model.".into(),
        ),
        "billing" => (
            format!(
                "{name} refused the request: the account has no credit or plan for it. {lost}"
            ),
            "Check the account's billing, or choose another model.".into(),
        ),
        "invalid_request" => (
            format!("{name} rejected the request as invalid{reference}. {lost}"),
            "Sending it again fails the same way: compact the conversation, rephrase, or choose another model.".into(),
        ),
        "unavailable" => (
            format!("This model is not available on the {name}. {lost}"),
            "Choose another model.".into(),
        ),
        "local_budget" => (
            format!("The request stopped because a budget limit was reached. {not_sent}"),
            "Choose another model.".into(),
        ),
        "network_policy" => (
            format!("This session's network policy blocked the request. {not_sent}"),
            "The destination is not allowed here; retrying is blocked the same way.".into(),
        ),
        _ => (
            format!("The {name} request failed and gave no reason{reference}. {lost}"),
            "Send a message to retry, or choose another model.".into(),
        ),
    };
    (sentence, Some(step), false)
}

/// A stopped turn: `stopped` (who stopped it is not recorded), `stopped: a budget limit was
/// reached`, with the calls that did not run.
fn stopped(message: &str, turn: Turn) -> String {
    let mut words = if message.trim_start().starts_with("budget_exhausted") {
        "stopped: a budget limit was reached".to_owned()
    } else {
        "stopped".to_owned()
    };
    match turn.not_run {
        0 => {}
        1 => words.push_str(" · 1 tool call did not run"),
        n => words.push_str(&format!(" · {n} tool calls did not run")),
    }
    words
}

/// Model and effort changes, bookmarks, stopped turns and failed turns as quiet rows. Only a
/// failure takes the error colour; a stopped turn is never one. Pi's own model and effort
/// entries are not drawn where the host recorded its own model marks, which say both.
pub(super) fn marker(
    item: &SessionItem,
    key: &str,
    rest: Rest<'_>,
    cx: Cx<'_>,
    turn: Turn,
) -> Option<Marker> {
    let (glyph, hue, words, body) = match item {
        SessionItem::ModelChange { .. } | SessionItem::ThinkingLevelChange { .. }
            if cx.journal.model_marks =>
        {
            return None;
        }
        SessionItem::ModelChange { provider, model } => (
            "○",
            Hue::Neutral,
            format!("model → {model} · {provider}"),
            Vec::new(),
        ),
        SessionItem::ThinkingLevelChange { level } => {
            ("○", Hue::Neutral, format!("effort → {level}"), Vec::new())
        }
        SessionItem::Label { target_id, label } => (
            "○",
            Hue::Neutral,
            match label {
                Some(label) => format!("bookmark “{label}” · entry {target_id}"),
                None => format!("bookmark removed · entry {target_id}"),
            },
            Vec::new(),
        ),
        SessionItem::Aborted { message, facts } => {
            let known = message.trim().is_empty()
                || STOP_TEXTS.contains(&message.trim())
                || message.starts_with("Aborted after ");
            (
                "○",
                Hue::Neutral,
                stopped(message, turn),
                if known {
                    told("", facts.as_deref())
                } else {
                    told(message, facts.as_deref())
                },
            )
        }
        SessionItem::ProviderError { message, facts } => {
            let (sentence, step, verbatim) = failure(message, facts.as_deref(), turn);
            let mut row = quiet(
                "✗",
                Hue::Fail,
                sentence,
                told(message, facts.as_deref()),
                key,
                rest,
                cx,
            );
            row.step = step;
            row.verbatim = verbatim;
            return Some(row);
        }
        _ => return None,
    };
    Some(quiet(glyph, hue, words, body, key, rest, cx))
}

/// An owner question the host recorded: `▲ asked you: …` while it waits, then how it ended.
fn owner_question(data: &Value) -> Option<(&'static str, Hue, String)> {
    let questions = data.get("questions").and_then(Value::as_array)?;
    let first = questions
        .first()
        .and_then(|question| question.get("question"))
        .and_then(Value::as_str)
        .map(|question| first_line(question).0)?;
    let more = match questions.len() {
        0 | 1 => String::new(),
        n => format!(" + {} more", n - 1),
    };
    Some(match data.get("status").and_then(Value::as_str) {
        Some("pending") => ("▲", Hue::You, format!("asked you: {first}{more}")),
        Some("answered") => ("○", Hue::Neutral, format!("you answered: {first}{more}")),
        Some(status) => (
            "○",
            Hue::Neutral,
            format!("question {status}: {first}{more}"),
        ),
        None => ("○", Hue::Neutral, format!("question: {first}{more}")),
    })
}

/// A presenter's card. Run marks, a cut reply, owner questions, billing notices and a cell
/// settled in the background are quiet rows, as the terminal draws them; the host's fact
/// entries settle other rows and are not rows themselves; any other card is a titled card with
/// its text and its data.
pub(super) fn card(
    item: &SessionItem,
    key: &str,
    rest: Rest<'_>,
    cx: Cx<'_>,
) -> Option<MessageItem> {
    let SessionItem::Card {
        title,
        text: said,
        data,
    } = item
    else {
        return None;
    };
    if FACT_ENTRIES.contains(&title.as_str()) {
        return None;
    }
    let data = data.as_deref();
    let detail = || -> Vec<Section> {
        [
            text("text", said),
            data.and_then(|data| text("data", &pretty(data))),
        ]
        .into_iter()
        .flatten()
        .collect()
    };
    let row =
        |glyph, hue, words| MessageItem::Marker(quiet(glyph, hue, words, detail(), key, rest, cx));
    let parsed = || {
        data.filter(|_| matches!(rest, Rest::Whole))
            .and_then(|data| serde_json::from_str::<Value>(data).ok())
    };
    // The build session's decision entries read as their terminal cards.
    if let Some(card) =
        parsed().and_then(|data| super::build_entries::entry(title, said, &data, key, rest, cx))
    {
        return Some(MessageItem::Build(card));
    }
    Some(match title.as_str() {
        "corral_mark" => {
            let (glyph, hue, words) = parsed()
                .and_then(|mark| run_mark(&mark, cx.journal))
                .unwrap_or(("○", Hue::Neutral, "run mark".to_owned()));
            row(glyph, hue, words)
        }
        "corral_code_cell" => {
            let settled = parsed()?;
            let call = settled.get("tool_call_id").and_then(Value::as_str);
            // The cell's own row shows its settlement when it is on this page.
            if call.is_some_and(|call| cx.journal.codemode.contains(call)) {
                return None;
            }
            let cell = settled
                .get("cell")
                .and_then(Value::as_str)
                .unwrap_or("cell");
            let cell = cell.strip_prefix("cell-").unwrap_or(cell);
            let status = settled
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("settled");
            let (glyph, hue) = if status == "ok" {
                ("✓", Hue::Ok)
            } else {
                ("✗", Hue::Fail)
            };
            row(
                glyph,
                hue,
                format!("code cell {cell} settled in background: {status}"),
            )
        }
        "corral_owner_question" => {
            let (glyph, hue, words) = parsed().and_then(|data| owner_question(&data)).unwrap_or((
                "○",
                Hue::Neutral,
                "owner question".to_owned(),
            ));
            row(glyph, hue, words)
        }
        "Reply cut" => row(
            "!",
            Hue::Held,
            "the reply was cut at the output limit".to_owned(),
        ),
        "Summary cost" | "Cache warming" => {
            let cost = parsed()
                .and_then(|value| value.get("usage").map(usage))
                .unwrap_or_default();
            let words = std::iter::once(title.to_lowercase())
                .chain(cost)
                .collect::<Vec<_>>()
                .join(" · ");
            row("○", Hue::Neutral, words)
        }
        _ => {
            let open = cx.open(key);
            let opens = data.is_some() || !matches!(rest, Rest::Whole);
            MessageItem::Card(Card {
                title: title.replace('_', " "),
                text: said.clone(),
                open: open && opens,
                body: match data {
                    Some(data) if open => text("data", &pretty(data)).into_iter().collect(),
                    _ => Vec::new(),
                },
                more: rest.more(cx),
                action: opens.then(|| rest.toggle(key, cx)),
            })
        }
    })
}

/// The first eight characters of an id: rows name a job or watch by it, the opened row by the
/// whole id.
pub(super) fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// What a finished run waits on, in words: the question's title rather than its id, a job's
/// command with its id shortened.
fn wait_words(wait: &Value, journal: &Journal) -> Option<String> {
    let field = |name: &str| wait.get(name).and_then(Value::as_str);
    let label = field("label").unwrap_or_default();
    let id = field("id").unwrap_or_default();
    Some(match field("kind")? {
        "owner" => match journal.questions.get(id) {
            Some(title) => format!("your answer to “{title}”"),
            None => "your answer".to_owned(),
        },
        "job" => {
            let command = journal
                .commands
                .get(id)
                .map(String::as_str)
                .or_else(|| label.split_once(": ").map(|(_, command)| command));
            match command {
                Some(command) => format!("job · {} ({})", first_line(command).0, short_id(id)),
                None => format!("job ({})", short_id(id)),
            }
        }
        "event" => match label.rsplit_once(" on ") {
            Some((_, job)) => format!("a watch on job ({})", short_id(job)),
            None => "a watch on a job".to_owned(),
        },
        _ => label.to_owned(),
    })
}

/// A run mark's row, as `mark-rows.ts` words it (`✓ done · 3m 12s · 14 tool calls · $0.42`).
fn run_mark(mark: &Value, journal: &Journal) -> Option<(&'static str, Hue, String)> {
    let field = |name: &str| mark.get(name).and_then(Value::as_str);
    match field("kind")? {
        "turn_end" => {
            let duration = mark.get("durationMs").and_then(Value::as_u64).unwrap_or(0);
            let calls = mark.get("toolCalls").and_then(Value::as_u64).unwrap_or(0);
            let reason = field("reason")?;
            let mut facts = Vec::new();
            // A run under 2 s without tool calls reads `✓ done` alone.
            if !(reason == "done" && duration < 2000 && calls == 0) {
                facts.extend(elapsed(duration));
            }
            if calls > 0 {
                facts.push(if calls == 1 {
                    "1 tool call".to_owned()
                } else {
                    format!("{calls} tool calls")
                });
            }
            if let Some(skipped) = mark
                .get("notRun")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
            {
                facts.push(format!("{skipped} not run"));
            }
            facts.extend(number(mark, "cost").filter(|usd| *usd > 0.0).map(dollars));
            facts.extend(
                number(mark, "rebilledTokens")
                    .filter(|n| *n > 0.0)
                    .map(|n| format!("{} tokens re-billed", tokens(n))),
            );
            let waits: Vec<String> = mark
                .get("waits")
                .and_then(Value::as_array)
                .map(|waits| {
                    waits
                        .iter()
                        .filter_map(|wait| wait_words(wait, journal))
                        .collect()
                })
                .unwrap_or_default();
            let (glyph, hue, lead, tail) = match reason {
                "done" => ("✓", Hue::Ok, "done".to_owned(), None),
                "waiting" => (
                    "○",
                    Hue::Neutral,
                    format!("waiting for {}", waits.join(", ")),
                    Some("the agent resumes when it finishes"),
                ),
                "length" => (
                    "!",
                    Hue::Held,
                    "the reply was cut at the output limit".to_owned(),
                    None,
                ),
                "continue_stopped" => (
                    "!",
                    Hue::Held,
                    format!(
                        "auto-continue stopped: {}",
                        field("note").unwrap_or("no reason recorded")
                    ),
                    Some("send a message to go on"),
                ),
                // A stopped or failed run names its cause on the row above it.
                _ => ("○", Hue::Neutral, "run ended".to_owned(), None),
            };
            Some((
                glyph,
                hue,
                std::iter::once(lead)
                    .chain(facts)
                    .chain(tail.map(str::to_owned))
                    .collect::<Vec<_>>()
                    .join(" · "),
            ))
        }
        "model" => {
            let mut words = format!("model → {}", field("model")?);
            if let Some(effort) = field("effort") {
                words.push_str(&format!(" · effort {effort}"));
            }
            if let Some(was) = mark.get("was") {
                let model = was
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                match was.get("effort").and_then(Value::as_str) {
                    Some(effort) => words.push_str(&format!(" (was {model} · {effort})")),
                    None => words.push_str(&format!(" (was {model})")),
                }
            }
            Some(("○", Hue::Neutral, words))
        }
        "note" => {
            let (glyph, hue) = match field("tone") {
                Some("error") => ("✗", Hue::Fail),
                Some("warning") => ("!", Hue::Held),
                _ => ("○", Hue::Neutral),
            };
            Some((glyph, hue, headline(field("text")?)))
        }
        "child" => {
            let child = field("child")?;
            let said = first_line(field("text").unwrap_or_default()).0;
            Some(match field("act")? {
                "asked" => ("▲", Hue::You, format!("{child} asked: {said}")),
                "steered" => ("→", Hue::Neutral, format!("{child} steered: “{said}”")),
                _ => ("→", Hue::Neutral, format!("{child}: {said}")),
            })
        }
        other => Some(("○", Hue::Neutral, format!("run mark · {other}"))),
    }
}
