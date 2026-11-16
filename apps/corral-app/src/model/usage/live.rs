//! corral's recorded usage as the Usage page: provider cards grouped by connection, the profile
//! table, cost per day from the usage series, and per-session tokens and cost. Costs say whether
//! the provider reported them or corral estimated them (`~`). A card's window, use, headroom and
//! reset all come from one quota observation (`window::header`); without one they are said to be
//! unknown, never a guessed band.

use std::collections::BTreeMap;

use corral_protocol::board::CostBasis;
use corral_protocol::detail::{DetailRef, UsageSeries};
use corral_protocol::rows::{AgentRow, AssistantRow, UsageRow};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::totals::Sum;
use super::{
    Chart, Level, Page, Period, PhoneProfile, ProfileRow, ProviderView, SessionRow, UNKNOWN,
    USED_LABEL, completeness_label, level, percent, quota, window,
};

/// Sessions listed at most, costliest first.
const SESSION_LIMIT: usize = 20;
const DISCLAIMER: &str =
    "~ marks costs estimated from token counts; quota facts show only where corral records them.";

/// The usage series to request: seven days carry today's samples and the week's daily costs.
pub fn series_request(period: Period) -> DetailRef {
    DetailRef::UsageSeries {
        period: match period {
            Period::Today | Period::Week => "7d",
            Period::Month => "30d",
        }
        .into(),
    }
}

/// What the live page is built from.
pub struct Inputs<'a> {
    pub rows: Vec<&'a UsageRow>,
    pub rows_error: Option<&'a str>,
    pub rows_loading: bool,
    pub series: Option<&'a UsageSeries>,
    /// Includes failed refreshes even when a retained series is available.
    pub series_error: Option<&'a str>,
    pub series_stale: bool,
    pub agents: Vec<&'a AgentRow>,
    pub assistants: Vec<&'a AssistantRow>,
    pub period: Period,
    /// Unix seconds.
    pub now: u64,
    pub tz: &'a TimeZone,
}

fn period_word(period: Period) -> &'static str {
    match period {
        Period::Today => "today",
        Period::Week => "7 days",
        Period::Month => "30 days",
    }
}

/// The window fraction a row's quota fact gives, when it has one.
fn quota(row: &UsageRow) -> Option<f32> {
    row.quotas.iter().find_map(quota::fraction)
}

/// A profile's cost over the period: the row's own today and week sums, or the series' daily
/// costs for thirty days.
fn row_cost(row: &UsageRow, input: &Inputs) -> Sum {
    let mut sum = Sum::default();
    match input.period {
        Period::Today => sum.add_total(row.today_usd, row.cost_basis, &row.today_completeness),
        Period::Week => sum.add_total(row.week_usd, row.cost_basis, &row.week_completeness),
        Period::Month => {
            if let Some(series) = input.series {
                for usage in series
                    .days
                    .iter()
                    .flat_map(|day| &day.by_profile)
                    .filter(|usage| {
                        usage.connection == row.connection
                            && usage.profile == row.profile.as_deref().unwrap_or_default()
                    })
                {
                    sum.add_total(usage.cost_usd, usage.cost_basis, &usage.completeness);
                }
                sum.qualify(&series.completeness);
            } else {
                sum.qualify(&Default::default());
            }
        }
    }
    sum
}

fn period_label(sum: &Sum, input: &Inputs) -> String {
    let figure = sum.figure();
    match period_note(sum, input) {
        Some(note) => format!("{figure} · {note}"),
        None => figure,
    }
}

fn period_note(sum: &Sum, input: &Inputs) -> Option<String> {
    let note = sum.note();
    if input.period == Period::Month && (input.series_stale || input.series_error.is_some()) {
        Some(note.map_or_else(
            || "stale series".into(),
            |note| format!("{note} · stale series"),
        ))
    } else {
        note
    }
}

/// A profile's name, or that corral recorded none.
fn profile_name(row: &UsageRow) -> String {
    row.profile
        .clone()
        .unwrap_or_else(|| "no profile recorded".into())
}

/// Today's local window samples of `connection`, as fractions of its limit at their local hours
/// of the day, in time order.
fn chart(
    series: Option<&UsageSeries>,
    connection: &str,
    label: &str,
    tz: &TimeZone,
    now: Timestamp,
) -> Option<Chart> {
    let today = now.to_zoned(tz.clone()).date();
    let mut samples: Vec<(f32, f32)> = series?
        .providers
        .iter()
        .filter(|provider| provider.id == connection)
        .flat_map(|provider| provider.points.iter())
        .filter_map(|point| {
            let at = point.at.parse::<Timestamp>().ok()?;
            let local = at.to_zoned(tz.clone());
            let fraction = match (point.used, point.limit) {
                (Some(used), Some(limit)) if limit > 0.0 => (used / limit) as f32,
                _ => return None,
            };
            let hour = f32::from(local.hour())
                + f32::from(local.minute()) / 60.0
                + f32::from(local.second()) / 3600.0;
            (local.date() == today).then_some((hour, fraction))
        })
        .collect();
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (hours, values): (Vec<f32>, Vec<f32>) = samples.into_iter().unzip();
    let coverage = series.and_then(|series| completeness_label(&series.completeness));
    (values.len() >= 2).then(|| Chart {
        label: format!(
            "{label} window use today, {} percent at the last sample{}",
            values.last().map_or(0, |&v| percent(v)),
            coverage
                .map(|note| format!(" · {note}"))
                .unwrap_or_default()
        ),
        series: values,
        hours,
    })
}

fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1}k", count as f64 / 1_000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

fn session_cost(value: Option<f64>, basis: Option<CostBasis>) -> (f64, Option<String>) {
    value.map_or((0.0, None), |_| {
        let mut sum = Sum::default();
        sum.add(value, basis);
        (sum.total, Some(sum.label()))
    })
}

fn sessions(input: &Inputs) -> (Vec<SessionRow>, Option<String>) {
    let mut rows: Vec<(f64, SessionRow)> = Vec::new();
    let mut silent = 0;
    for agent in &input.agents {
        if agent.cost_usd.is_none() && agent.tokens.is_none() {
            silent += 1;
            continue;
        }
        let (order, cost) = session_cost(agent.cost_usd, agent.cost_basis);
        rows.push((
            order,
            SessionRow {
                session: agent.id.clone(),
                what: match &agent.task {
                    Some(task) => format!("{} · {task}", agent.role),
                    None => format!("{} · {}", agent.role, agent.title),
                },
                profile: agent.profile.clone(),
                tokens: agent.tokens.map(tokens),
                cost,
            },
        ));
    }
    // corrald also lists each assistant as an agent session, which carries its tokens; an
    // assistant row stands in only when the agents feed has not listed that session.
    for assistant in &input.assistants {
        if input.agents.iter().any(|agent| agent.id == assistant.id) {
            continue;
        }
        if assistant.cost_usd.is_none() {
            silent += 1;
            continue;
        }
        let (order, cost) = session_cost(assistant.cost_usd, assistant.cost_basis);
        rows.push((
            order,
            SessionRow {
                session: assistant.id.clone(),
                what: format!("assistant · {}", assistant.title),
                profile: assistant.profile.clone(),
                tokens: None,
                cost,
            },
        ));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let hidden = rows.len().saturating_sub(SESSION_LIMIT);
    let note = match (hidden, silent) {
        (0, 0) => None,
        (0, silent) => Some(format!("{silent} more sessions have no recorded usage.")),
        (hidden, 0) => Some(format!("{hidden} cheaper sessions not listed.")),
        (hidden, silent) => Some(format!(
            "{hidden} cheaper sessions not listed; {silent} more have no recorded usage."
        )),
    };
    (
        rows.into_iter()
            .take(SESSION_LIMIT)
            .map(|(_, row)| row)
            .collect(),
        note,
    )
}

/// The live page.
pub fn page(input: &Inputs) -> Page {
    let now = Timestamp::from_second(i64::try_from(input.now).unwrap_or(0))
        .unwrap_or(Timestamp::UNIX_EPOCH);
    let mut connections: BTreeMap<&str, Vec<(&UsageRow, Sum)>> = BTreeMap::new();
    for row in &input.rows {
        connections
            .entry(row.connection.as_str())
            .or_default()
            .push((*row, row_cost(row, input)));
    }
    let mut providers = Vec::new();
    let mut profiles = Vec::new();
    for (connection, rows) in &connections {
        let header = window::header(rows.iter().map(|(row, _)| *row));
        let fact = header.map(|(_, fact)| fact);
        let used = fact.and_then(quota::fraction);
        let provider_level = used.map_or(Level::None, level);
        let mut period = Sum::default();
        let mut today = Sum::default();
        let mut week = Sum::default();
        for (row, cost) in rows {
            period.add_sum(cost);
            today.add_total(row.today_usd, row.cost_basis, &row.today_completeness);
            week.add_total(row.week_usd, row.cost_basis, &row.week_completeness);
        }
        let mut quotas: Vec<_> = header
            .map(|(row, fact)| window::line(row, fact, false, now, input.tz))
            .into_iter()
            .collect();
        for (row, _) in rows {
            for other in &row.quotas {
                if !fact.is_some_and(|fact| std::ptr::eq(fact, other)) {
                    quotas.push(window::line(row, other, true, now, input.tz));
                }
            }
        }
        let chart = chart(input.series, connection, connection, input.tz, now);
        providers.push(ProviderView {
            id: (*connection).to_owned(),
            label: (*connection).to_owned(),
            window: window::window(fact),
            big: used.map_or_else(|| period.figure(), |used| format!("{}%", percent(used))),
            big_note: if used.is_some() {
                fact.and_then(|fact| completeness_label(&fact.completeness))
            } else {
                period_note(&period, input)
            },
            big_label: if used.is_some() {
                USED_LABEL
            } else {
                period_word(input.period)
            },
            used,
            level: provider_level,
            quota_line: fact
                .map_or_else(window::missing, |fact| window::amounts(fact, now, input.tz)),
            exhausted: fact.is_some_and(|fact| fact.exhausted == Some(true)),
            headroom: window::headroom(fact),
            costs: format!("today {} · 7 days {}", today.label(), week.label()),
            chart,
            profiles: rows
                .iter()
                .map(|(row, cost)| {
                    let cost = period_label(cost, input);
                    PhoneProfile {
                        name: profile_name(row),
                        detail: match quota(row) {
                            Some(share) => format!("{}% · {cost}", percent(share)),
                            None => cost,
                        },
                    }
                })
                .collect(),
            quotas,
        });
        for (i, (row, cost)) in rows.iter().enumerate() {
            let share = quota(row).map(percent);
            let first = i == 0;
            profiles.push(ProfileRow {
                provider: if first {
                    (*connection).to_owned()
                } else {
                    String::new()
                },
                profile: profile_name(row),
                share_percent: share,
                share: share.map_or_else(|| UNKNOWN.to_owned(), |share| format!("{share}%")),
                level: provider_level,
                cost: cost.figure(),
                cost_note: period_note(cost, input),
                headroom: first.then(|| quota::number(fact.and_then(|fact| fact.headroom))),
            });
        }
    }
    let (sessions, sessions_note) = sessions(input);
    Page {
        providers_empty: (providers.is_empty()
            && input.rows_error.is_none()
            && !input.rows_loading
            && !input.series_stale
            && input.series_error.is_none()
            && input
                .series
                .is_some_and(|series| completeness_label(&series.completeness).is_none())
            && sessions.is_empty())
        .then_some("corral has recorded no provider usage yet."),
        providers,
        profile_headers: [
            "Provider",
            "Profile",
            "Share of window",
            match input.period {
                Period::Today => "Today",
                Period::Week => "7 days",
                Period::Month => "30 days",
            },
            "Headroom",
        ],
        profiles,
        costs: super::daily::costs(input),
        sessions,
        sessions_note,
        disclaimer: DISCLAIMER,
        notices: notices(input),
    }
}

/// Suppress repeated transport wrappers without hiding the actual daemon cause.
fn cause(mut error: &str) -> &str {
    while let Some((wrapper, rest)) = error.split_once(": ") {
        if ![
            "app topic unavailable",
            "unavailable",
            "usage unavailable",
            "daily costs unavailable",
        ]
        .iter()
        .any(|prefix| wrapper.eq_ignore_ascii_case(prefix))
        {
            break;
        }
        error = rest;
    }
    error
}

fn notices(input: &Inputs) -> Vec<String> {
    let rows = input.rows_error.map(cause);
    let series = input.series_error.map(cause);
    let text = match (rows, series) {
        (None, None) => return vec![],
        (Some(rows), Some(series)) if rows != series => format!("{rows}; daily costs: {series}"),
        (Some(error), _) | (_, Some(error)) => error.to_owned(),
    };
    vec![format!("Couldn't read usage: {text}")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::rows::LimitBasis;

    #[test]
    fn missing_quota_is_unknown_not_a_band() {
        let row = UsageRow {
            id: "c/p".into(),
            connection: "c".into(),
            profile: Some("p".into()),
            window: None,
            used: None,
            limit: None,
            limit_basis: LimitBasis::None,
            resets_at: None,
            today_usd: Some(2.5),
            week_usd: None,
            cost_basis: Some(CostBasis::Estimated),
            quotas: Vec::new(),
            today_completeness: Default::default(),
            week_completeness: Default::default(),
            rev: 1,
        };
        let tz = TimeZone::UTC;
        let page = page(&Inputs {
            rows: vec![&row],
            rows_error: None,
            rows_loading: false,
            series: None,
            series_error: None,
            series_stale: false,
            agents: vec![],
            assistants: vec![],
            period: Period::Today,
            now: 1_700_000_000,
            tz: &tz,
        });
        let provider = &page.providers[0];
        assert_eq!(provider.used, None);
        assert_eq!(provider.headroom, "quota unknown");
        assert_eq!(provider.big, "~$2.50");
        assert_eq!(page.profiles[0].share_percent, None);
    }

    fn agent(id: &str, tokens: Option<u64>, cost_usd: Option<f64>) -> AgentRow {
        AgentRow {
            id: id.into(),
            role: "assistant".into(),
            task: None,
            run: None,
            title: format!("{id} · workspace"),
            profile: Some("app-smoke".into()),
            runtime: Some("corral-pi".into()),
            machine: None,
            pane: None,
            status: corral_protocol::rows::AgentStatus::Live,
            now: String::new(),
            since: None,
            tail: Vec::new(),
            cost_usd,
            cost_basis: cost_usd.map(|_| CostBasis::Reported),
            tokens,
            rev: 1,
            generation: None,
            turn: None,
            controls: Default::default(),
        }
    }

    fn assistant(id: &str, cost_usd: Option<f64>) -> AssistantRow {
        AssistantRow {
            id: id.into(),
            kind: corral_protocol::rows::AssistantKind::Session,
            title: format!("{id} · workspace"),
            state: corral_protocol::rows::AssistantState::Ready,
            live_in: corral_protocol::rows::AssistantLocation::Daemon,
            profile: Some("app-smoke".into()),
            focus: None,
            pending: 0,
            turn: Some(1),
            cost_usd,
            cost_basis: cost_usd.map(|_| CostBasis::Reported),
            wait: None,
            rev: 1,
        }
    }

    /// corrald lists each assistant both as an agent session (with its tokens) and as an
    /// assistant row; one session is one row, and a reported zero is a known cost, not unknown.
    #[test]
    fn assistant_sessions_list_once_with_reported_zero() {
        let agents = [agent("A-1", Some(30), Some(0.0)), agent("A-2", None, None)];
        let assistants = [assistant("A-1", Some(0.0)), assistant("A-2", None)];
        let tz = TimeZone::UTC;
        let build = |assistants: Vec<&AssistantRow>| {
            page(&Inputs {
                rows: vec![],
                rows_error: None,
                rows_loading: false,
                series: None,
                series_error: None,
                series_stale: false,
                agents: agents.iter().collect(),
                assistants,
                period: Period::Today,
                now: 1_700_000_000,
                tz: &tz,
            })
        };
        let both = build(assistants.iter().collect());
        let agents_only = build(vec![]);
        let rows = &both.sessions;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session, "A-1");
        assert_eq!(rows[0].tokens, Some(tokens(30)));
        assert_eq!(
            rows[0].cost,
            session_cost(Some(0.0), Some(CostBasis::Reported)).1
        );
        assert_ne!(rows[0].cost, session_cost(None, None).1);
        // A-2 is still counted as a session without usage, once.
        assert!(both.sessions_note.is_some());
        assert_eq!(both.sessions_note, agents_only.sessions_note);
    }
}

#[cfg(test)]
#[path = "regression.rs"]
mod regression;
