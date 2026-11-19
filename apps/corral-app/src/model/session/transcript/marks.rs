//! Everything in a replay that is not words or a tool row: images, the compaction and branch
//! dividers, quiet marker rows (model and effort changes, bookmarks, stopped turns, provider
//! errors, run marks, host notices), presenter cards and the assistant turn's footer. Quiet
//! rows follow the terminal's rule: a glyph that says the state without its colour, muted
//! words, and on opening the full text and the recorded facts.

use corral_protocol::detail::{SessionItem, SessionMessage};
use serde_json::Value;

use crate::model::hue::Hue;

use super::replay::{Cx, Rest, first_line, lines, size};
use super::tool::{elapsed, pretty};
use super::{Card, Divider, Footer, Marker, MessageItem, More, Picture, Section};

/// The card that carries an assistant turn's complete metadata; it is the footer's detail.
pub(super) const TURN_FOOTER: &str = "Turn footer";

/// Images a `data:` URI may carry: raster formats only, nothing that runs.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Stop reasons of a turn that ended as turns normally do; any other is said in the footer.
const NORMAL_STOPS: [&str; 4] = ["stop", "toolUse", "tool_use", "end_turn"];

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
pub(super) fn picture(
    item: &SessionItem,
    _key: &str,
    rest: Rest<'_>,
    cx: Cx<'_>,
) -> Option<Picture> {
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
        glyph,
        hue,
        words,
        open: open && opens,
        body: if open { body } else { Vec::new() },
        more: rest.more(cx),
        action: opens.then(|| rest.toggle(key, cx)),
    }
}

/// A message's first line, and how many more lines it has, as one row's words.
fn headline(message: &str) -> String {
    match first_line(message) {
        (first, 0) => first.to_owned(),
        (first, more) => format!("{first} · {} more", lines(more)),
    }
}

/// A sentence with more lines than its row shows, and the recorded facts.
fn told(message: &str, facts: Option<&str>) -> Vec<Section> {
    let multi = first_line(message).1 > 0;
    [
        multi.then(|| text("message", message)).flatten(),
        facts.and_then(|facts| text("facts", &pretty(facts))),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Model and effort changes, bookmarks, stopped turns and provider errors as quiet rows. Only
/// a provider error takes the error colour; a stopped turn is never one.
pub(super) fn marker(item: &SessionItem, key: &str, rest: Rest<'_>, cx: Cx<'_>) -> Option<Marker> {
    let (glyph, hue, words, body) = match item {
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
        SessionItem::Aborted { message, facts } => (
            "○",
            Hue::Neutral,
            headline(message),
            told(message, facts.as_deref()),
        ),
        SessionItem::ProviderError { message, facts } => (
            "✗",
            Hue::Fail,
            headline(message),
            told(message, facts.as_deref()),
        ),
        _ => return None,
    };
    Some(quiet(glyph, hue, words, body, key, rest, cx))
}

/// A presenter's card. Run marks, host notices, a cut reply and billing notices are quiet rows,
/// as the terminal draws them; every other card is a titled card with its text and its data.
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
    Some(match title.as_str() {
        "corral_mark" => {
            let (glyph, hue, words) = parsed().and_then(|mark| run_mark(&mark)).unwrap_or((
                "○",
                Hue::Neutral,
                "run mark".to_owned(),
            ));
            row(glyph, hue, words)
        }
        "Host notice" => row("○", Hue::Neutral, headline(said)),
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

/// A run mark's row, as the terminal words it (`✓ done · 3m 12s · 14 tool calls · $0.42`).
fn run_mark(mark: &Value) -> Option<(&'static str, Hue, String)> {
    let field = |name: &str| mark.get(name).and_then(Value::as_str);
    match field("kind")? {
        "turn_end" => {
            let mut facts = Vec::new();
            facts.extend(
                mark.get("durationMs")
                    .and_then(Value::as_u64)
                    .and_then(elapsed),
            );
            if let Some(calls) = mark
                .get("toolCalls")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
            {
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
            let waits: Vec<&str> = mark
                .get("waits")
                .and_then(Value::as_array)
                .map(|waits| {
                    waits
                        .iter()
                        .filter_map(|wait| wait.get("label").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            let (glyph, hue, lead) = match field("reason")? {
                "done" => ("✓", Hue::Ok, "done".to_owned()),
                "waiting" => (
                    "○",
                    Hue::Neutral,
                    format!("waiting for {}", waits.join(", ")),
                ),
                "length" => (
                    "!",
                    Hue::Held,
                    "the reply was cut at the output limit".to_owned(),
                ),
                "continue_stopped" => (
                    "!",
                    Hue::Held,
                    format!(
                        "auto-continue stopped: {}",
                        field("note").unwrap_or("no reason recorded")
                    ),
                ),
                _ => ("○", Hue::Neutral, "run ended".to_owned()),
            };
            Some((
                glyph,
                hue,
                std::iter::once(lead)
                    .chain(facts)
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
