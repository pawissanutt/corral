//! The period the Usage page totals: a preset run of local days through today, or a custom range
//! of at most [`MOST_DAYS`] local days, and the series request it makes in the device's local
//! time. Days are calendar days where the device is; the request carries the device's offset
//! from UTC now, and corrald buckets the whole range at that offset.

use corral_protocol::detail::DetailRef;
use corral_types::UsageBy;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};

use crate::model::daemon::Feed;

/// The most local days one series spans (the wire's bound).
pub const MOST_DAYS: i64 = 366;

/// Which days the page totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    /// The last seven local days, today included.
    Week,
    /// The last thirty local days, today included.
    #[default]
    Month,
    /// From the first of this local month through today.
    ThisMonth,
    /// The range typed in the From and To fields.
    Custom,
}

/// The periods in display order, with their switch labels.
pub const PERIODS: [(Period, &str); 4] = [
    (Period::Week, "7 days"),
    (Period::Month, "30 days"),
    (Period::ThisMonth, "This month"),
    (Period::Custom, "Custom"),
];

/// A run of local calendar days, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Days {
    pub first: Date,
    pub last: Date,
}

impl Days {
    /// How many days the run holds.
    pub fn count(self) -> i64 {
        self.first
            .until(self.last)
            .map_or(0, |span| i64::from(span.get_days()))
            + 1
    }

    /// The `n` days ending with `today`.
    fn through(today: Date, n: i64) -> Self {
        Self {
            first: today.checked_sub(Span::new().days(n - 1)).unwrap_or(today),
            last: today,
        }
    }

    /// Every day of the run, first to last.
    pub fn each(self) -> impl Iterator<Item = Date> {
        std::iter::successors(Some(self.first), move |day| {
            day.tomorrow().ok().filter(|next| *next <= self.last)
        })
    }
}

/// The device's clock as the page uses it: today's local date, the offset east of UTC now and
/// the zone's name when the platform gave one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    pub today: Date,
    pub offset_minutes: i16,
    pub zone: Option<String>,
}

/// The device's clock from the feed: the driver's real time in the viewer's zone, UTC when the
/// platform could not tell.
pub fn local(feed: &Feed) -> Local {
    let now = Timestamp::from_second(i64::try_from(feed.now).unwrap_or(0))
        .unwrap_or(Timestamp::UNIX_EPOCH);
    let zone = feed.time_zone.clone().unwrap_or(TimeZone::UTC);
    let at = now.to_zoned(zone.clone());
    Local {
        today: at.date(),
        offset_minutes: i16::try_from(at.offset().seconds() / 60).unwrap_or(0),
        zone: zone.iana_name().map(str::to_owned),
    }
}

/// Selection state for the Usage page: the grouping, the period, and the custom range as
/// applied and as typed.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub by: UsageBy,
    pub period: Period,
    /// The custom range last applied; kept while a preset is chosen.
    pub custom: Option<Days>,
    /// The From and To fields as typed (`YYYY-MM-DD`, both days included).
    pub from: String,
    pub to: String,
    /// Why the typed range was not applied, until the fields change.
    pub refused: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            by: UsageBy::Profile,
            period: Period::default(),
            custom: None,
            from: String::new(),
            to: String::new(),
            refused: None,
        }
    }
}

impl State {
    /// The days the page totals while `today` is the local date.
    pub fn days(&self, today: Date) -> Days {
        match (self.period, self.custom) {
            (Period::Week, _) => Days::through(today, 7),
            (Period::ThisMonth, _) => Days {
                first: today.first_of_month(),
                last: today,
            },
            (Period::Custom, Some(custom)) => custom,
            (Period::Month | Period::Custom, _) => Days::through(today, 30),
        }
    }

    /// Choose a grouping.
    pub fn select_by(&mut self, by: UsageBy) {
        self.by = by;
    }

    /// Choose a period. Custom opens on the days shown until now, typed into its fields, so the
    /// owner edits a range rather than starting from blanks.
    pub fn select_period(&mut self, period: Period, today: Date) {
        if period == Period::Custom && self.custom.is_none() {
            let shown = self.days(today);
            self.custom = Some(shown);
            self.from = shown.first.to_string();
            self.to = shown.last.to_string();
        }
        self.period = period;
        self.refused = None;
    }

    /// The From field changed.
    pub fn type_from(&mut self, text: &str) {
        text.clone_into(&mut self.from);
        self.refused = None;
    }

    /// The To field changed.
    pub fn type_to(&mut self, text: &str) {
        text.clone_into(&mut self.to);
        self.refused = None;
    }

    /// Apply the typed range, or say why it cannot be applied and keep the one shown.
    pub fn apply_custom(&mut self) {
        match typed(&self.from, &self.to) {
            Ok(days) => {
                self.custom = Some(days);
                self.period = Period::Custom;
                self.refused = None;
            }
            Err(why) => self.refused = Some(why),
        }
    }
}

/// The typed range as days, or why it is not one.
fn typed(from: &str, to: &str) -> Result<Days, String> {
    let day = |name: &str, text: &str| {
        text.trim()
            .parse::<Date>()
            .map_err(|_| format!("{name} is not a date; write it as 2026-11-01."))
    };
    let days = Days {
        first: day("From", from)?,
        last: day("To", to)?,
    };
    if days.last < days.first {
        return Err("To comes before From; the range needs at least one day.".into());
    }
    let count = days.count();
    if count > MOST_DAYS {
        return Err(format!(
            "A range spans at most {MOST_DAYS} days; this one spans {count}."
        ));
    }
    Ok(days)
}

/// The usage series for the page's period and grouping: from the first local day (included) to
/// the day after the last (excluded), cut at the device's IANA zone when the platform names one
/// (so a clock change mid-range keeps its days whole), else at its offset from UTC now.
pub fn series_request(state: &State, local: &Local) -> DetailRef {
    let days = state.days(local.today);
    DetailRef::UsageSeries {
        from: days.first.to_string(),
        to: days.last.tomorrow().unwrap_or(days.last).to_string(),
        by: Some(state.by),
        utc_offset_minutes: local.offset_minutes,
        time_zone: local.zone.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request names the chosen local days, the device's zone and its offset: New York in
    /// winter is five hours behind UTC, so at 02:00 UTC it is still the day before there. A
    /// custom range is sent as typed, To included; one past 366 days is refused and the shown
    /// range stays.
    #[test]
    fn the_series_request_carries_the_chosen_local_days_and_the_device_zone() {
        let feed = Feed {
            now: 1_763_431_200,
            time_zone: Some(TimeZone::get("America/New_York").unwrap()),
            ..Feed::default()
        };
        let local = local(&feed);
        assert_eq!(local.today.to_string(), "2025-11-17");
        assert_eq!(local.offset_minutes, -300);
        let mut state = State::default();
        state.select_period(Period::Week, local.today);
        let range = |state: &State| match series_request(state, &local) {
            DetailRef::UsageSeries {
                from,
                to,
                utc_offset_minutes,
                time_zone,
                ..
            } => (from, to, utc_offset_minutes, time_zone),
            other => panic!("not a usage series: {other:?}"),
        };
        let zone = Some("America/New_York".to_owned());
        assert_eq!(
            range(&state),
            ("2025-11-11".into(), "2025-11-18".into(), -300, zone.clone())
        );
        state.select_period(Period::ThisMonth, local.today);
        assert_eq!(range(&state).0, "2025-11-01");
        state.select_period(Period::Custom, local.today);
        state.type_from("2025-01-01");
        state.type_to("2025-02-28");
        state.apply_custom();
        assert_eq!(
            range(&state),
            ("2025-01-01".into(), "2025-03-01".into(), -300, zone)
        );
        state.type_from("2024-01-01");
        state.apply_custom();
        assert!(state.refused.is_some());
        assert_eq!(range(&state).0, "2025-01-01");
    }
}
