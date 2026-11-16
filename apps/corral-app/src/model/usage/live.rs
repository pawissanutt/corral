//! corral's recorded usage as the Usage page: a summary card per provider connection, one profile
//! table grouped by provider, cost per day from the usage series, and per-session tokens and
//! cost. Costs say whether the provider reported them or corral estimated them (`~`). A card's
//! window, use, headroom and reset all come from one quota observation (`window::header`);
//! without one the card says [`NO_QUOTA`] once, never a guessed band. How complete the totals
//! are is said once for the page (`coverage`); a provider or profile adds only what is its own.

use std::collections::BTreeMap;

use corral_protocol::board::CostBasis;
use corral_protocol::detail::{DetailRef, UsageCompleteness, UsageSeries};
use corral_protocol::rows::{AgentRow, AssistantRow, UsageRow};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::totals::Sum;
use super::{
    Chart, Level, NO_PROFILE, NO_QUOTA, Page, Period, PhoneProfile, ProfileGroup, ProfileRow,
    ProviderView, SessionRow, UNATTRIBUTED, UNATTRIBUTED_ASIDE, UNKNOWN, USED_LABEL, coverage,
    level, percent, quota, window,
};

/// Sessions listed at most, costliest first.
const SESSION_LIMIT: usize = 20;
const DISCLAIMER: &str = "~ marks costs estimated from token counts; — marks a figure corral has \
                          not recorded. Usage is visibility only: nothing here routes work.";

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

/// The period after a figure: "$7.60 today".
fn period_word(period: Period) -> &'static str {
    match period {
        Period::Today => "today",
        Period::Week => "7 days",
        Period::Month => "30 days",
    }
}

/// The period after "no cost recorded".
fn period_phrase(period: Period) -> &'static str {
    match period {
        Period::Today => "today",
        Period::Week => "in 7 days",
        Period::Month => "in 30 days",
    }
}

/// A connection's title; usage recorded without one is its own, named group.
pub(super) fn provider_label(connection: &str) -> &str {
    if connection.is_empty() {
        UNATTRIBUTED
    } else {
        connection
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

/// Thirty-day costs read a series corrald did not refresh.
fn stale_month(input: &Inputs) -> bool {
    input.period == Period::Month && (input.series_stale || input.series_error.is_some())
}

/// A profile's name, and whether corral recorded one.
fn profile_name(row: &UsageRow) -> (String, bool) {
    match &row.profile {
        Some(profile) => (profile.clone(), true),
        None => (NO_PROFILE.into(), false),
    }
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
    (values.len() >= 2).then(|| Chart {
        label: format!(
            "{label} window use today, {} percent at the last sample",
            values.last().map_or(0, |&v| percent(v)),
        ),
        series: values,
        hours,
    })
}

pub(crate) fn tokens(count: u64) -> String {
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
        (sum.total, Some(sum.figure()))
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

/// The costs a card's big figure does not already say: both today and seven days under a quota
/// figure, else the one the period's cost leaves out (both for thirty days).
fn other_costs(today: &Sum, week: &Sum, quota_figure: bool, period: Period) -> String {
    let mut parts = Vec::with_capacity(2);
    if quota_figure || period != Period::Today {
        parts.push(format!("today {}", today.figure()));
    }
    if quota_figure || period != Period::Week {
        parts.push(format!("7 days {}", week.figure()));
    }
    parts.join(" · ")
}

/// One provider's card and its profile group; its coverage, for the page's comparison.
fn provider(
    connection: &str,
    rows: &[(&UsageRow, Sum)],
    input: &Inputs,
    now: Timestamp,
) -> (ProviderView, ProfileGroup, Sum) {
    let label = provider_label(connection);
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
        for other in row.quotas.iter().filter(|fact| quota::informative(fact)) {
            if !fact.is_some_and(|fact| std::ptr::eq(fact, other)) {
                quotas.push(window::line(row, other, true, now, input.tz));
            }
        }
    }
    let (big, big_label) = match used {
        Some(used) => (format!("{}%", percent(used)), USED_LABEL.to_owned()),
        None if period.known => (period.figure(), period_word(input.period).to_owned()),
        None => (
            UNKNOWN.to_owned(),
            format!("no cost recorded {}", period_phrase(input.period)),
        ),
    };
    let mut profiles = Vec::with_capacity(rows.len());
    let mut table = Vec::with_capacity(rows.len());
    for (row, cost) in rows {
        let share = quota(row).map(percent);
        let (name, named) = profile_name(row);
        let gaps = cost.coverage.as_ref().and_then(coverage::row_gaps);
        let mut detail: Vec<String> = share.map(|share| format!("{share}%")).into_iter().collect();
        detail.push(cost.figure());
        detail.extend(gaps.clone());
        profiles.push(PhoneProfile {
            name: name.clone(),
            named,
            detail: detail.join(" · "),
        });
        table.push(ProfileRow {
            profile: name,
            named,
            gaps,
            share_percent: share,
            share: share.map_or_else(|| UNKNOWN.to_owned(), |share| format!("{share}%")),
            level: provider_level,
            cost: cost.figure(),
        });
    }
    let card = ProviderView {
        id: if connection.is_empty() {
            "unattributed".into()
        } else {
            connection.to_owned()
        },
        label: label.to_owned(),
        aside: if connection.is_empty() {
            Some(UNATTRIBUTED_ASIDE.into())
        } else {
            window::window(fact)
        },
        big,
        big_label,
        used,
        level: provider_level,
        quota_known: fact.is_some(),
        quota_line: fact.map_or_else(
            || NO_QUOTA.into(),
            |fact| window::amounts(fact, now, input.tz),
        ),
        exhausted: fact.is_some_and(|fact| fact.exhausted == Some(true)),
        headroom: window::headroom(fact),
        costs: other_costs(&today, &week, used.is_some(), input.period),
        note: None,
        chart: chart(input.series, connection, label, input.tz, now),
        profiles,
        quotas,
    };
    let group = ProfileGroup {
        id: card.id.clone(),
        label: label.to_owned(),
        cost: period.figure(),
        rows: table,
    };
    (card, group, period)
}

/// The live page.
pub fn page(input: &Inputs) -> Page {
    let now = Timestamp::from_second(i64::try_from(input.now).unwrap_or(0))
        .unwrap_or(Timestamp::UNIX_EPOCH);
    // Connections in name order, the unattributed group last.
    let mut connections: BTreeMap<(bool, &str), Vec<(&UsageRow, Sum)>> = BTreeMap::new();
    for row in &input.rows {
        connections
            .entry((row.connection.is_empty(), row.connection.as_str()))
            .or_default()
            .push((*row, row_cost(row, input)));
    }
    let mut providers = Vec::with_capacity(connections.len());
    let mut groups = Vec::with_capacity(connections.len());
    let mut own: Vec<Option<UsageCompleteness>> = Vec::with_capacity(connections.len());
    let mut total = Sum::default();
    for ((_, connection), rows) in &connections {
        let (card, group, period) = provider(connection, rows, input, now);
        total.add_sum(&period);
        own.push(period.coverage);
        providers.push(card);
        groups.push(group);
    }
    // A provider differs from the rows' common scan, not from the series the bars add to it.
    let rows_scan = total.coverage.clone();
    for (card, coverage) in providers.iter_mut().zip(&own) {
        card.note = coverage
            .as_ref()
            .and_then(|own| coverage::differing_scan(own, rows_scan.as_ref(), now, input.tz));
    }
    if let Some(series) = input.series {
        total.qualify(&series.completeness);
    }
    let unrecorded = connections
        .values()
        .flatten()
        .filter(|(_, cost)| !cost.known)
        .count() as u64;
    let (sessions, sessions_note) = sessions(input);
    Page {
        providers_empty: (providers.is_empty()
            && input.rows_error.is_none()
            && !input.rows_loading
            && !input.series_stale
            && input.series_error.is_none()
            && input
                .series
                .is_some_and(|series| !coverage::incomplete(&series.completeness))
            && sessions.is_empty())
        .then_some("corral has recorded no provider usage yet."),
        coverage: coverage::page_note(
            total.coverage.as_ref(),
            unrecorded,
            stale_month(input),
            now,
            input.tz,
        ),
        providers,
        profile_headers: ["Profile", "Share of window", period_word(input.period)],
        shares_known: groups
            .iter()
            .flat_map(|group| &group.rows)
            .any(|row| row.share_percent.is_some()),
        groups,
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
        assert!(!provider.quota_known);
        assert_eq!(provider.big, "~$2.50");
        assert_eq!(page.groups[0].rows[0].share_percent, None);
        assert!(!page.shares_known);
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
