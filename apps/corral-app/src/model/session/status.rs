//! The status bar under the composer: the role pill, the session's cost, the context meter, the
//! model and effort while the composer bar is folded, and the host's standing alerts. A cost the
//! host did not report is `$–`, never `$0.00`; a cost with an unpriced member carries `+`; a
//! context window the host did not report is `ctx –`.

use corral_protocol::session_state::UsageBrief;

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
    pub cost: Cost,
    pub meter: ContextMeter,
    /// `<provider>/<model> · effort high`, shown only while the composer bar is folded.
    pub model_effort: Option<String>,
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

pub fn role_pill(role: &SessionRole) -> RolePill {
    let (full, short) = match role {
        SessionRole::Build => ("BUILD", "BLD"),
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

/// `$1.24`, with `+` when a member's cost is unknown; `$–` when nothing is known.
pub fn cost_text(cost: Option<f64>, partial: bool) -> String {
    match cost {
        Some(value) if value.is_finite() => {
            format!("${value:.2}{}", if partial { "+" } else { "" })
        }
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
    let action = usage_view(id);
    let (Some(usage), Some(percent)) = (usage, percent.filter(|value| value.is_finite())) else {
        return ContextMeter {
            filled: None,
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
        full: format!(
            "ctx {bar} {shown}% · {} / {}",
            tokens(usage.context_tokens),
            tokens(usage.context_window)
        ),
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
    let (cost, partial) = match usage {
        Some(usage) => (usage.cost_usd, usage.cost_partial),
        None => (cx.row.and_then(|row| row.cost_usd), false),
    };
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
    StatusBar {
        role: role_pill(&cx.role),
        cost: Cost {
            text: cost_text(cost, partial),
            partial,
            action: usage_view(cx.id),
        },
        meter: meter(cx.id, usage),
        model_effort: cx.model().map(|model| match cx.effort() {
            Some(effort) => format!("{model} · effort {effort}"),
            None => model,
        }),
        alerts,
        stale: cx.feed.link != crate::model::daemon::Link::Up,
    }
}
