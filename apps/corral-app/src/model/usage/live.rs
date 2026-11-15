//! corral's recorded usage as the Usage page: provider cells grouped by connection, the profile
//! table, cost per day from the usage series, and per-session tokens and cost. Costs say whether
//! the provider reported them or corral estimated them (`~`); quota, share and headroom appear
//! only when corral holds a quota fact and are otherwise "unknown", never a guessed band.

use std::collections::BTreeMap;

use corral_protocol::board::CostBasis;
use corral_protocol::detail::{DetailRef, UsageSeries};
use corral_protocol::rows::{AgentRow, AssistantRow, LimitBasis, UsageRow};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::{
    Chart, CostBar, Costs, Level, Page, Period, PhoneProfile, ProfileRow, ProviderView, SessionRow,
    USED_LABEL, level, money, percent,
};

/// Sessions listed at most, costliest first.
const SESSION_LIMIT: usize = 20;
const LIVE_DISCLAIMER: &str = "Costs are corral's records: reported by the provider, or ~ estimated from token counts. Window share, headroom and resets show only where corral holds a provider quota fact; otherwise they are unknown.";
const LIVE_PHONE_DISCLAIMER: &str =
    "~ estimated from tokens · quota unknown unless corral records it.";

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
    pub series: Option<&'a UsageSeries>,
    /// Why the series is missing, when its request failed.
    pub series_error: Option<&'a str>,
    pub agents: Vec<&'a AgentRow>,
    pub assistants: Vec<&'a AssistantRow>,
    pub period: Period,
    /// Unix seconds.
    pub now: u64,
    pub tz: &'a TimeZone,
}

/// A sum of costs that remembers whether any part was estimated.
#[derive(Debug, Clone, Copy, Default)]
struct Sum {
    total: f64,
    known: bool,
    estimated: bool,
}

impl Sum {
    fn add(&mut self, value: Option<f64>, basis: Option<CostBasis>) {
        if let Some(value) = value {
            self.total += value;
            self.known = true;
            self.estimated |= basis != Some(CostBasis::Reported);
        }
    }

    fn label(self) -> String {
        match (self.known, self.estimated) {
            (false, _) => "unknown".into(),
            (true, true) => format!("~{}", money(self.total)),
            (true, false) => money(self.total),
        }
    }
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
    match (row.used, row.limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit) as f32),
        _ => None,
    }
}

/// A profile's cost over the period: the row's own today and week sums, or the series' daily
/// costs for thirty days.
fn row_cost(row: &UsageRow, input: &Inputs) -> Sum {
    let mut sum = Sum::default();
    match input.period {
        Period::Today => sum.add(row.today_usd, row.cost_basis),
        Period::Week => sum.add(row.week_usd, row.cost_basis),
        Period::Month => {
            if let (Some(profile), Some(series)) = (&row.profile, input.series) {
                for day in &series.days {
                    for usage in day.by_profile.iter().filter(|u| &u.profile == profile) {
                        sum.add(usage.cost_usd, usage.cost_basis);
                    }
                }
            }
        }
    }
    sum
}

fn local_clock(at: Timestamp, tz: &TimeZone) -> String {
    let local = at.to_zoned(tz.clone());
    format!("{:02}:{:02}", local.hour(), local.minute())
}

fn resets(row: Option<&UsageRow>, now: Timestamp, tz: &TimeZone) -> (String, String) {
    let Some(at) = row
        .and_then(|row| row.resets_at.as_deref())
        .and_then(|at| at.parse::<Timestamp>().ok())
    else {
        return (String::new(), String::new());
    };
    let minutes = (at.as_second() - now.as_second()).max(0) / 60;
    let phone = if minutes < 24 * 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        local_clock(at, tz)
    };
    (
        format!("resets at {}", local_clock(at, tz)),
        format!("resets {phone}"),
    )
}

/// Today's local window samples of `connection`, as fractions of its limit.
fn chart(
    series: Option<&UsageSeries>,
    connection: &str,
    label: &str,
    tz: &TimeZone,
    now: Timestamp,
) -> Option<Chart> {
    let today = now.to_zoned(tz.clone()).date();
    let (values, ticks): (Vec<f32>, Vec<String>) = series?
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
            (local.date() == today).then(|| (fraction, format!("{:02}", local.hour())))
        })
        .unzip();
    (values.len() >= 2).then(|| Chart {
        label: format!(
            "{label} window use today, {} percent at the last sample",
            values.last().map_or(0, |&v| percent(v))
        ),
        series: values,
        ticks,
    })
}

fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1}k", count as f64 / 1_000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

fn session_cost(value: Option<f64>, basis: Option<CostBasis>) -> (f64, String) {
    let mut sum = Sum::default();
    sum.add(value, basis);
    (sum.total, sum.label())
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
                profile: agent.profile.clone().unwrap_or_else(|| "unknown".into()),
                tokens: agent.tokens.map_or_else(|| "unknown".into(), tokens),
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
                profile: assistant
                    .profile
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
                tokens: "unknown".into(),
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

fn day_label(day: &str, period: Period) -> String {
    let Ok(date) = day.parse::<jiff::civil::Date>() else {
        return day.to_owned();
    };
    match period {
        Period::Month => format!("{:02}-{:02}", date.month(), date.day()),
        Period::Today | Period::Week => {
            use jiff::civil::Weekday;
            match date.weekday() {
                Weekday::Monday => "Mon",
                Weekday::Tuesday => "Tue",
                Weekday::Wednesday => "Wed",
                Weekday::Thursday => "Thu",
                Weekday::Friday => "Fri",
                Weekday::Saturday => "Sat",
                Weekday::Sunday => "Sun",
            }
            .to_owned()
        }
    }
}

/// Daily costs stacked by profile: the two costliest profiles and the rest together, or each
/// profile when there are three or fewer. Bars are scaled to the costliest day.
fn costs(input: &Inputs) -> Costs {
    let title = match input.period {
        Period::Month => "Cost per day · 30 days (UTC days)",
        Period::Today | Period::Week => "Cost per day · 7 days (UTC days)",
    }
    .to_owned();
    let Some(series) = input.series else {
        return Costs {
            title,
            legend: Vec::new(),
            bars: Vec::new(),
            empty: Some(match input.series_error {
                Some(error) => format!("Daily costs unavailable: {error}"),
                None => "Loading daily costs from corrald…".into(),
            }),
        };
    };
    let mut totals: BTreeMap<&str, f64> = BTreeMap::new();
    for usage in series.days.iter().flat_map(|day| &day.by_profile) {
        *totals.entry(usage.profile.as_str()).or_default() += usage.cost_usd.unwrap_or(0.0);
    }
    let mut ranked: Vec<(&str, f64)> = totals.into_iter().filter(|(_, v)| *v > 0.0).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    if ranked.is_empty() {
        let recorded = series.days.iter().flat_map(|day| &day.by_profile);
        let mut zero = Sum::default();
        let mut unpriced = false;
        for usage in recorded {
            zero.add(usage.cost_usd, usage.cost_basis);
            unpriced |= usage.cost_usd.is_none();
        }
        let empty = match (zero.known, unpriced) {
            (false, false) => "corral recorded no usage in this period.".to_owned(),
            (false, true) => "Usage recorded in this period has no price.".to_owned(),
            (true, false) => format!("corral recorded {} of cost in this period.", zero.label()),
            (true, true) => format!(
                "corral recorded {} of cost in this period; some usage has no price.",
                zero.label()
            ),
        };
        return Costs {
            title,
            legend: Vec::new(),
            bars: Vec::new(),
            empty: Some(empty),
        };
    }
    let grouped = ranked.len() > 3;
    let named: Vec<&str> = ranked
        .iter()
        .take(if grouped { 2 } else { 3 })
        .map(|(name, _)| *name)
        .collect();
    let mut legend: Vec<String> = named.iter().map(|&name| name.to_owned()).collect();
    if grouped {
        legend.push("other profiles".into());
    }
    let days: Vec<([f64; 3], Sum, String)> = series
        .days
        .iter()
        .map(|day| {
            let mut values = [0.0; 3];
            let mut sum = Sum::default();
            for usage in &day.by_profile {
                sum.add(usage.cost_usd, usage.cost_basis);
                let slot = named
                    .iter()
                    .position(|&name| name == usage.profile)
                    .unwrap_or(2);
                values[slot] += usage.cost_usd.unwrap_or(0.0);
            }
            (values, sum, day_label(&day.day, input.period))
        })
        .collect();
    let most = days
        .iter()
        .map(|(values, _, _)| values.iter().sum::<f64>())
        .fold(0.0, f64::max)
        .max(f64::MIN_POSITIVE);
    let bars = days
        .into_iter()
        .map(|(values, sum, day)| {
            // Widths round on the running total, so the bar's length is its total's.
            let mut widths = [0; 3];
            let mut running = 0.0;
            let mut drawn = 0;
            for (slot, value) in values.into_iter().enumerate() {
                running += value;
                let end = (running / most * 100.0).round().min(100.0) as u32;
                widths[slot] = end.saturating_sub(drawn);
                drawn = end;
            }
            CostBar {
                day,
                values: widths,
                total_label: if sum.known { sum.label() } else { "—".into() },
            }
        })
        .collect();
    Costs {
        title,
        legend,
        bars,
        empty: None,
    }
}

/// The live page.
pub fn page(input: &Inputs) -> Page {
    let now = Timestamp::from_second(i64::try_from(input.now).unwrap_or(0))
        .unwrap_or(Timestamp::UNIX_EPOCH);
    let mut connections: BTreeMap<&str, Vec<&UsageRow>> = BTreeMap::new();
    for row in &input.rows {
        connections
            .entry(row.connection.as_str())
            .or_default()
            .push(*row);
    }
    let mut providers = Vec::new();
    let mut profiles = Vec::new();
    for (connection, rows) in &connections {
        let quota_row = rows.iter().copied().find(|row| quota(row).is_some());
        let used = quota_row.and_then(quota);
        let provider_level = used.map_or(Level::None, level);
        let window = rows
            .iter()
            .find_map(|row| row.window.clone())
            .unwrap_or_else(|| "window unknown".into());
        let mut period = Sum::default();
        let mut today = Sum::default();
        let mut week = Sum::default();
        for row in rows {
            let cost = row_cost(row, input);
            period.add(
                cost.known.then_some(cost.total),
                (!cost.estimated).then_some(CostBasis::Reported),
            );
            today.add(row.today_usd, row.cost_basis);
            week.add(row.week_usd, row.cost_basis);
        }
        let (resets, phone_resets) = resets(quota_row, now, input.tz);
        let basis = match quota_row.map(|row| row.limit_basis) {
            Some(LimitBasis::Declared) => " · limit declared",
            Some(LimitBasis::Observed) => " · limit observed",
            _ => "",
        };
        let headroom = used.map_or_else(
            || "quota unknown".into(),
            |used| format!("{}% headroom", percent((1.0 - used).max(0.0))),
        );
        let chart = chart(input.series, connection, connection, input.tz, now);
        providers.push(ProviderView {
            id: (*connection).to_owned(),
            label: (*connection).to_owned(),
            window: window.clone(),
            big: used.map_or_else(|| period.label(), |used| format!("{}%", percent(used))),
            big_label: if used.is_some() {
                USED_LABEL
            } else {
                period_word(input.period)
            },
            used,
            headroom,
            level: provider_level,
            resets,
            phone_resets,
            costs: format!("today {} · 7 days {}{basis}", today.label(), week.label()),
            chart_missing: chart.is_none().then_some("no window samples recorded"),
            chart,
            profiles: rows
                .iter()
                .map(|row| {
                    let cost = row_cost(row, input).label();
                    PhoneProfile {
                        name: row
                            .profile
                            .clone()
                            .unwrap_or_else(|| "no profile recorded".into()),
                        detail: match quota(row) {
                            Some(share) => format!("{}% · {cost}", percent(share)),
                            None => cost,
                        },
                    }
                })
                .collect(),
        });
        for (i, row) in rows.iter().enumerate() {
            let share = quota(row).map(percent);
            profiles.push(ProfileRow {
                provider: if i == 0 {
                    (*connection).to_owned()
                } else {
                    String::new()
                },
                profile: row
                    .profile
                    .clone()
                    .unwrap_or_else(|| "no profile recorded".into()),
                window: row.window.clone().unwrap_or_else(|| "—".into()),
                share_percent: share,
                level: provider_level,
                today: row_cost(row, input).label(),
                headroom: match (i, used) {
                    (0, Some(used)) => format!("{}%", percent((1.0 - used).max(0.0))),
                    (0, None) => "unknown".into(),
                    _ => String::new(),
                },
            });
        }
    }
    let (sessions, sessions_note) = sessions(input);
    Page {
        providers_empty: providers
            .is_empty()
            .then_some("corral has recorded no provider usage yet."),
        providers,
        profile_headers: [
            "Provider",
            "Profile",
            "Window",
            "Share of window",
            match input.period {
                Period::Today => "Today",
                Period::Week => "7 days",
                Period::Month => "30 days",
            },
            "Headroom",
        ],
        profiles,
        costs: costs(input),
        sessions: Some(sessions),
        sessions_note,
        disclaimer: LIVE_DISCLAIMER,
        phone_disclaimer: LIVE_PHONE_DISCLAIMER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            series: None,
            series_error: None,
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
                series: None,
                series_error: None,
                agents: agents.iter().collect(),
                assistants,
                period: Period::Today,
                now: 1_700_000_000,
                tz: &tz,
            })
        };
        let both = build(assistants.iter().collect());
        let agents_only = build(vec![]);
        let rows = both.sessions.as_deref().unwrap_or_default();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session, "A-1");
        assert_eq!(rows[0].tokens, tokens(30));
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
