//! The Usage view (`Chat › Usage`, opened from the status bar's cost and meter, or from the
//! session's actions): what the session and each of its members spent, read from its host
//! (`SessionUsage`). One row for the session's own pane and one per child and helper; a row the
//! host did not price says `unpriced` and is never `$0.00`; the total carries the status bar's
//! partial mark and, when a row is unpriced, one sentence saying which rows are missing and why.
//! The status bar takes its cost from [`total`], so both name the same figure.

use corral_protocol::detail::{ContextUse, DetailBody, DetailRef, SessionUsage, UsageRow};

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::{ActionGroup, row};
use super::keys::KeyBinding;
use super::status::{cost_text, tokens};
use super::tabs::{PartBody, load, part};
use super::{Action, Can, Cx, State, SubView, Tab};

const CAPABILITY: &str = "usage";

/// What a row or the total shows where the host reported no figure.
const NONE: &str = "–";

/// How many unpriced rows the partial sentence names before it counts the rest.
const NAMED: usize = 3;

/// The Usage view's state on this device, per session. The view has no state of its own: it
/// scrolls and rereads through the shared part frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageUi {}

impl UsageUi {
    pub const EMPTY: Self = Self {};
}

/// What the Usage view does: nothing beyond the part frame's Reread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageAction {}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageView {
    pub part: PartBody,
    /// `Model`, `Effort`, `Context window` of the session, each said when the host did not
    /// report it.
    pub facts: Vec<(String, String)>,
    /// The pane first, then every child and helper in the host's order.
    pub rows: Vec<UsageLine>,
    pub total: Option<UsageTotalLine>,
    /// Why there is no row to show, once the host answered.
    pub empty: Option<String>,
    /// What the figures are and are not.
    pub notes: Vec<&'static str>,
}

/// One member's figures, as words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLine {
    /// `usage-row-pane`, `usage-row-<child>`.
    pub id: String,
    /// `This session`, a child's title, a helper's kind.
    pub name: String,
    /// `worker`, `helper · explore`; none for the session's own pane.
    pub kind: Option<String>,
    pub turns: String,
    pub tokens: String,
    pub input: String,
    pub output: String,
    /// `24%`, `–` when the host did not report it.
    pub context: String,
    /// `$0.42`, `unpriced`.
    pub cost: String,
    pub unpriced: bool,
    /// Why a figure is missing, plainly.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageTotalLine {
    pub tokens: String,
    /// The status bar's own text: `$1.24`, `$1.24+`, `$–`.
    pub cost: String,
    pub partial: bool,
    /// What a partial total adds up and which rows are missing from it, and why.
    pub why: Option<String>,
}

/// The session's total tokens and cost, and whether some usage was not priced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Total {
    pub tokens: u64,
    pub cost_usd: Option<f64>,
    pub partial: bool,
}

pub fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionUsage {
        id: cx.id.to_string(),
    }
}

fn read<'f>(cx: &Cx<'f>) -> Option<&'f SessionUsage> {
    match cx.feed.detail(&target(cx))?.body()? {
        DetailBody::SessionUsage(usage) => Some(usage),
        _ => None,
    }
}

fn priced(row: &UsageRow) -> bool {
    row.cost_usd.is_some_and(f64::is_finite)
}

/// The total of a read: a cost that is only a lower bound of nothing is no cost, and a total
/// that leaves an unpriced row out says so even when the host did not mark it.
fn read_total(usage: &SessionUsage) -> Option<Total> {
    let total = usage.total.as_ref()?;
    let partial = total.partial || usage.rows.iter().any(|row| !priced(row));
    let cost_usd = total
        .cost_usd
        .filter(|cost| cost.is_finite() && !(partial && *cost == 0.0));
    Some(Total {
        tokens: total.tokens,
        cost_usd,
        partial,
    })
}

/// The total of the host's state brief, which the status bar shows while the view is closed.
fn brief_total(cx: &Cx<'_>) -> Option<Total> {
    let usage = &cx.state?.usage;
    Some(Total {
        tokens: usage.tokens,
        cost_usd: usage
            .cost_usd
            .filter(|cost| cost.is_finite() && !(usage.cost_partial && *cost == 0.0)),
        partial: usage.cost_partial,
    })
}

fn open(cx: &Cx<'_>) -> bool {
    cx.ui.tab == Tab::Chat && cx.ui.sub == Some(SubView::Usage)
}

/// The total the status bar shows: the read the Usage view shows while it is open (so the two
/// name one figure), else the host's state brief, which follows the session live. `None` when
/// the host reported neither.
pub fn total(cx: &Cx<'_>) -> Option<Total> {
    if open(cx)
        && let Some(total) = read(cx).and_then(read_total)
    {
        return Some(total);
    }
    brief_total(cx)
}

/// `$1.24+ · 48.0k tokens · ctx 24%`; `No usage recorded yet.` while the host counted nothing.
pub fn summary(cx: &Cx<'_>) -> String {
    let Some(total) = total(cx) else {
        return "usage not reported yet".into();
    };
    if total.tokens == 0 && total.cost_usd.is_none() {
        return "No usage recorded yet.".into();
    }
    let context = cx
        .state
        .and_then(|state| state.usage.context_percent)
        .map_or_else(
            || "ctx –".to_owned(),
            |percent| format!("ctx {percent:.0}%"),
        );
    format!(
        "{} · {} tokens · {context}",
        cost_text(total.cost_usd, total.partial),
        tokens(total.tokens)
    )
}

fn is_helper(row: &UsageRow) -> bool {
    row.kind.starts_with("helper:")
}

fn name(row: &UsageRow) -> String {
    if row.key == "pane" {
        return "This session".into();
    }
    let named = row.name.trim();
    if !named.is_empty() {
        return named.to_owned();
    }
    row.key
        .strip_prefix("child:")
        .unwrap_or(&row.key)
        .to_owned()
}

fn kind(row: &UsageRow) -> Option<String> {
    if row.key == "pane" {
        return None;
    }
    let kind = row.kind.replace('_', " ");
    Some(match kind.strip_prefix("helper:") {
        Some(helper) => format!("helper · {helper}"),
        None => kind,
    })
}

fn slug(key: &str) -> String {
    let id = key.strip_prefix("child:").unwrap_or(key);
    id.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

fn line(row: &UsageRow) -> UsageLine {
    let helper = is_helper(row);
    let unpriced = !priced(row);
    // The host counts no turns or tokens for a helper, so none is shown rather than a zero.
    let figure = |text: String| if helper { NONE.to_owned() } else { text };
    let note = if helper {
        Some("Helper usage is not counted.".to_owned())
    } else if unpriced {
        Some("The host reported no cost for it.".to_owned())
    } else {
        None
    };
    UsageLine {
        id: format!("usage-row-{}", slug(&row.key)),
        name: name(row),
        kind: kind(row),
        turns: figure(row.turns.to_string()),
        tokens: figure(tokens(row.tokens)),
        input: figure(tokens(row.input)),
        output: figure(tokens(row.output)),
        context: row
            .context_percent
            .filter(|percent| percent.is_finite())
            .map_or_else(|| NONE.to_owned(), |percent| format!("{percent:.0}%")),
        cost: match row.cost_usd {
            Some(cost) if cost.is_finite() => cost_text(Some(cost), false),
            _ => "unpriced".into(),
        },
        unpriced,
        note,
    }
}

/// `a2`, `a2 and b3`, `a2, b3, c4 and 2 more`.
fn names(unpriced: &[&UsageRow]) -> String {
    let shown: Vec<String> = unpriced.iter().take(NAMED).map(|row| name(row)).collect();
    let more = unpriced.len().saturating_sub(NAMED);
    match shown.split_last() {
        None => String::new(),
        Some((only, [])) if more == 0 => only.clone(),
        Some((last, rest)) if more == 0 => format!("{} and {last}", rest.join(", ")),
        Some(_) => format!("{} and {more} more", shown.join(", ")),
    }
}

/// What a partial total adds up and which rows are missing from it, and why.
fn why_partial(usage: &SessionUsage) -> String {
    let unpriced: Vec<&UsageRow> = usage.rows.iter().filter(|row| !priced(row)).collect();
    if unpriced.is_empty() {
        return "The host marked this total partial without saying which usage has no price, \
                so read it as a lower bound."
            .into();
    }
    let helpers = if unpriced.iter().any(|row| is_helper(row)) {
        " Helper usage is not counted."
    } else {
        ""
    };
    if unpriced.len() == usage.rows.len() {
        return format!("No row has a recorded price, so there is no cost to add up.{helpers}");
    }
    let (verb, spend) = if unpriced.len() == 1 {
        ("has", "its")
    } else {
        ("have", "their")
    };
    format!(
        "This total adds up only the priced rows. {} {verb} no recorded cost, so {spend} spend \
         is missing from it.{helpers}",
        names(&unpriced)
    )
}

fn facts(usage: &SessionUsage) -> Vec<(String, String)> {
    let said = |value: &Option<String>| {
        value
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| "not reported".to_owned(), str::to_owned)
    };
    vec![
        ("Model".to_owned(), said(&usage.model)),
        ("Effort".to_owned(), said(&usage.effort)),
        ("Context".to_owned(), context(usage.context.as_ref())),
    ]
}

fn context(context: Option<&ContextUse>) -> String {
    let Some(context) = context.filter(|context| context.context_window > 0) else {
        return "not reported".into();
    };
    let window = tokens(context.context_window);
    match (context.tokens, context.percent.filter(|p| p.is_finite())) {
        (Some(used), Some(percent)) => {
            format!("{} of {window} tokens ({percent:.0}%)", tokens(used))
        }
        (Some(used), None) => format!("{} of {window} tokens", tokens(used)),
        (None, Some(percent)) => format!("{percent:.0}% of {window} tokens"),
        (None, None) => format!("{window} tokens; use not reported"),
    }
}

const NOTES: [&str; 3] = [
    "Reported cost is not a verified bill.",
    "API-equivalent USD is neither billed spend nor provider quota.",
    "A figure the host did not report shows as – or unpriced, never as zero.",
];

pub fn usage_view(cx: &Cx<'_>) -> UsageView {
    let total = total(cx);
    let hue = if total.is_some_and(|total| total.partial) {
        Hue::Held
    } else {
        Hue::Neutral
    };
    let target = match cx.can(CAPABILITY) {
        Can::Absent => Err("this session's host does not report usage".to_owned()),
        _ => Ok(target(cx)),
    };
    let part = part(cx, target, Some(CAPABILITY), summary(cx), hue);
    let usage = read(cx).filter(|_| part.unavailable.is_none() && part.read.has_rows());
    let Some(usage) = usage else {
        return UsageView {
            part,
            facts: Vec::new(),
            rows: Vec::new(),
            total: None,
            empty: None,
            notes: Vec::new(),
        };
    };
    let shown = read_total(usage).or_else(|| brief_total(cx));
    UsageView {
        part,
        facts: facts(usage),
        rows: usage.rows.iter().map(line).collect(),
        total: shown.map(|total| UsageTotalLine {
            tokens: tokens(total.tokens),
            cost: cost_text(total.cost_usd, total.partial),
            partial: total.partial,
            why: total.partial.then(|| why_partial(usage)),
        }),
        empty: usage
            .rows
            .is_empty()
            .then(|| "The host reported no usage rows for this session.".to_owned()),
        notes: NOTES.to_vec(),
    }
}

/// What the view reads while it shows.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    load(cx, Ok(target(cx)), Some(CAPABILITY))
        .into_iter()
        .collect()
}

pub(super) fn act(_: &mut State, action: UsageAction, _: &Feed) -> Vec<Effect> {
    match action {}
}

/// `Open usage` among the views, where the host serves it.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if matches!(cx.can(CAPABILITY), Can::Absent) {
        return None;
    }
    Some(ActionGroup {
        title: "Views",
        rows: vec![row(
            "view-usage",
            "Open usage",
            None,
            Action::Sub {
                session: cx.id.clone(),
                sub: Some(SubView::Usage),
            },
        )],
    })
}

/// The view's own keys.
pub fn keys() -> Vec<KeyBinding> {
    Vec::new()
}

pub fn press(_: &str, _: &Cx<'_>) -> Option<Action> {
    None
}
