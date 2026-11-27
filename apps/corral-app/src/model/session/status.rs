//! The status line under the composer: the role pill, the session's cost, the context meter, the
//! model and effort, how long the agent has been quiet, how much input is queued, and the host's
//! standing alerts. A cost the host did not report is `$–`, never `$0.00`; a cost with an
//! unpriced member carries `+`; a context window the host did not report is `ctx –`; a quiet
//! time or a queue the host did not report is not shown.

use corral_protocol::session_state::{PlanModePhase, SessionState, UsageBrief};

use crate::model::hue::Hue;

use super::notices;
use super::{Action, Cx, SessionRole, SubView};
use crate::model::Id;

/// Opens the session's Usage sub-view.
fn usage_view(id: &Id) -> Action {
    Action::Sub {
        session: id.clone(),
        sub: Some(SubView::Usage),
    }
}

/// Cells of the context meter's bar.
pub const METER_CELLS: u8 = 12;

#[derive(Debug, Clone, PartialEq)]
pub struct StatusBar {
    pub role: RolePill,
    /// `PLAN`, `PLAN · review`: the session is in plan mode.
    pub plan: Option<&'static str>,
    pub cost: Cost,
    pub meter: ContextMeter,
    /// `<provider>/<model> · effort high`.
    pub model_effort: Option<String>,
    /// `quiet 12s`: a run's time since its last output, as corrald reports it.
    pub quiet: Option<String>,
    /// `2 queued`: the host's queue, while it holds input.
    pub queued: Option<String>,
    pub alerts: Vec<Alert>,
    /// The values are the last ones received while corrald is unreachable.
    pub stale: bool,
}

/// `[BUILD]`, `[BLD]` below 64 Cells / 420 pt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolePill {
    pub full: String,
    pub short: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cost {
    /// `$1.24`, `$1.24+`, `$–`.
    pub text: String,
    /// A member's cost is unknown, so the total is a lower bound.
    pub partial: bool,
    /// Opens the Usage sub-view.
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextMeter {
    /// Filled cells of `METER_CELLS`; none when the window is unknown.
    pub filled: Option<u8>,
    /// The share of the window in use, rounded; none when the window is unknown.
    pub percent: Option<u32>,
    /// `ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 24% · 48.0k / 200k`, `ctx –`.
    pub full: String,
    /// `ctx 24%`, never dropped.
    pub short: String,
    /// Held from 80 %, failing from 95 %.
    pub hue: Hue,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub hue: Hue,
    pub text: String,
}

/// `PLAN` while the session plans, `PLAN · review` while its plan waits for the owner; none
/// outside plan mode.
pub fn plan_tag(state: &SessionState) -> Option<&'static str> {
    state.plan_mode.as_ref().map(|plan| match plan.phase {
        PlanModePhase::Review => "PLAN · review",
        PlanModePhase::Planning | PlanModePhase::Other => "PLAN",
    })
}

pub fn role_pill(role: &SessionRole) -> RolePill {
    let (full, short) = match role {
        SessionRole::Build => ("BUILD", "BLD"),
        SessionRole::Code => ("CODE", "CODE"),
        SessionRole::Assistant => ("ASSISTANT", "AST"),
        SessionRole::Worker => ("WORKER", "WRK"),
        SessionRole::Reviewer => ("REVIEWER", "REV"),
        SessionRole::Planner => ("PLANNER", "PLN"),
        SessionRole::PlanReviewer => ("PLAN REVIEWER", "PRV"),
        SessionRole::Child => ("CHILD", "CHD"),
        SessionRole::Other(role) => {
            let full = role.replace('_', " ").to_uppercase();
            let short: String = full
                .chars()
                .filter(|c| !c.is_whitespace())
                .take(3)
                .collect();
            return RolePill { full, short };
        }
    };
    RolePill {
        full: full.into(),
        short: short.into(),
    }
}

/// `$1.24`, with `+` when a member's cost is unknown; `<$0.01` for a known cost above zero and
/// below one cent, so `$0.00` is only a cost of exactly zero; `$–` when nothing is known.
pub fn cost_text(cost: Option<f64>, partial: bool) -> String {
    let mark = if partial { "+" } else { "" };
    match cost {
        Some(value) if value.is_finite() && value > 0.0 && value < 0.005 => {
            format!("<$0.01{mark}")
        }
        Some(value) if value.is_finite() => format!("${value:.2}{mark}"),
        _ => "$–".into(),
    }
}

/// `48.0k`, `200k`, `1.2M`.
pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..100_000 => format!("{:.1}k", n as f64 / 1_000.0),
        100_000..1_000_000 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// The meter of a usage brief: a window of zero is unknown.
pub fn meter(id: &Id, usage: Option<&UsageBrief>) -> ContextMeter {
    let usage = usage.filter(|usage| usage.context_window > 0);
    let percent = usage.and_then(|usage| {
        usage
            .context_percent
            .or_else(|| Some(usage.context_tokens as f32 * 100.0 / usage.context_window as f32))
    });
    meter_of(
        id,
        percent,
        usage.map(|usage| (usage.context_tokens, usage.context_window)),
    )
}

/// The meter of a context share, with its tokens and window when they are known
/// (`ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 24% · 48.0k / 200k`, else `ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 24%`).
fn meter_of(id: &Id, percent: Option<f32>, window: Option<(u64, u64)>) -> ContextMeter {
    let action = usage_view(id);
    let Some(percent) = percent.filter(|value| value.is_finite()) else {
        return ContextMeter {
            filled: None,
            percent: None,
            full: "ctx –".into(),
            short: "ctx –".into(),
            hue: Hue::Neutral,
            action,
        };
    };
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * f32::from(METER_CELLS)).round() as u8;
    let bar: String = (0..METER_CELLS)
        .map(|cell| if cell < filled { '▰' } else { '▱' })
        .collect();
    let shown = clamped.round() as u32;
    ContextMeter {
        filled: Some(filled),
        percent: Some(shown),
        full: match window {
            Some((used, window)) => {
                format!("ctx {bar} {shown}% · {} / {}", tokens(used), tokens(window))
            }
            None => format!("ctx {bar} {shown}%"),
        },
        short: format!("ctx {shown}%"),
        hue: if clamped >= 95.0 {
            Hue::Fail
        } else if clamped >= 80.0 {
            Hue::Held
        } else {
            Hue::Neutral
        },
        action,
    }
}

/// The status bar of the session `cx` reads. Without a host state, the row's recorded cost.
pub fn status(cx: &Cx<'_>) -> StatusBar {
    let usage = cx.state.map(|state| &state.usage);
    let (cost, partial) = spent(cx);
    let mut alerts = Vec::new();
    if let Some(error) = cx.state.and_then(|state| state.last_error.as_deref()) {
        alerts.push(Alert {
            hue: Hue::Held,
            text: format!("! {error}"),
        });
    }
    alerts.extend(notices::standing(cx.state).into_iter().map(|notice| Alert {
        hue: if notice.hue == Hue::Neutral {
            Hue::Held
        } else {
            notice.hue
        },
        text: notice.text,
    }));
    let with_effort = |model: String, effort: Option<String>| match effort {
        Some(effort) => format!("{model} · effort {effort}"),
        None => model,
    };
    // An open agent conversation reports that agent's facts; the host's alerts stay.
    let opened = super::fleet::opened_agent(cx);
    let viewing_child = opened.is_some();
    let (role, cost, partial, meter, model_effort) = match opened {
        Some(agent) => (
            RolePill {
                full: format!("AGENT {}", agent.id),
                short: "AGT".into(),
            },
            agent.cost_usd,
            false,
            meter_of(cx.id, agent.context_percent, None),
            Some(with_effort(agent.model.label, agent.effort)),
        ),
        None => (
            role_pill(cx.role),
            cost,
            partial,
            meter(cx.id, usage),
            cx.model().map(|model| with_effort(model, cx.effort())),
        ),
    };
    StatusBar {
        role,
        plan: cx.state.and_then(plan_tag),
        cost: Cost {
            text: cost_text(cost, partial),
            partial,
            action: usage_view(cx.id),
        },
        meter,
        model_effort,
        quiet: if viewing_child { None } else { quiet(cx) },
        queued: cx
            .state
            .filter(|_| !viewing_child)
            .map(|state| state.queue_total)
            .filter(|queued| *queued > 0)
            .map(|queued| format!("{queued} queued")),
        alerts,
        stale: cx.feed.link != crate::model::daemon::Link::Up,
    }
}

/// What the session spent, and whether a member's cost is unknown so the sum is a lower bound:
/// the host's usage, else the row's recorded cost.
pub fn spent(cx: &Cx<'_>) -> (Option<f64>, bool) {
    match super::usage::total(cx) {
        Some(total) => (total.cost_usd, total.partial),
        None => (cx.row.and_then(|row| row.cost_usd), false),
    }
}

/// `quiet 12s`: how long a working run has said nothing, from corrald's row.
pub fn quiet(cx: &Cx<'_>) -> Option<String> {
    let run = cx
        .run
        .filter(|run| matches!(run.status, corral_types::SessionStatus::Working))?;
    let since = u64::try_from(run.run.quiet_since?.as_second()).ok()?;
    (cx.feed.now > 0 && since <= cx.feed.now).then(|| {
        format!(
            "quiet {}",
            crate::model::daemon::age(Some(since), cx.feed.now)
        )
    })
}
