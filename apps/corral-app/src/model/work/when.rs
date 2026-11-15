//! Shared live date/age formatting uses the client-fed IANA zone for each recorded instant.
//! Before a zone arrives, a complete local date/clock tick supplies the fallback offset.

use crate::model::daemon::Feed;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The actual local offset at the last tick; UTC before a complete local tick arrives.
fn offset(feed: &Feed) -> i64 {
    let day = feed
        .date
        .parse::<jiff::civil::Date>()
        .ok()
        .and_then(|date| date.to_zoned(jiff::tz::TimeZone::UTC).ok())
        .map(|date| date.timestamp().as_second());
    let Some(day) = day else { return 0 };
    let Some((hours, minutes)) = feed.clock.split_once(':') else {
        return 0;
    };
    let (Ok(hours), Ok(minutes)) = (hours.parse::<i8>(), minutes.parse::<i8>()) else {
        return 0;
    };
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) {
        return 0;
    }
    let local = day + i64::from(hours) * 3600 + i64::from(minutes) * 60;
    let utc = i64::try_from(feed.now / 60 * 60).unwrap_or(0);
    local - utc
}

fn seconds(at: &str) -> Option<i64> {
    at.parse::<jiff::Timestamp>().ok().map(|at| at.as_second())
}

/// `HH:MM` of `secs` in local time.
fn clock(local: i64) -> String {
    let minute = local.rem_euclid(86_400) / 60;
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// `Nov 12` of a local day count since the epoch.
fn day(local: i64) -> String {
    jiff::Timestamp::from_second(local)
        .ok()
        .map(|at| at.to_zoned(jiff::tz::TimeZone::UTC))
        .and_then(|zoned| {
            let month = MONTHS.get(usize::try_from(zoned.month() - 1).ok()?)?;
            Some(format!("{month} {}", zoned.day()))
        })
        .unwrap_or_default()
}

/// A daemon instant as a feed time: `18:40` today, `Nov 12` before; empty when unparseable.
pub fn stamp(at: &str, feed: &Feed) -> String {
    if let Some(zone) = &feed.time_zone {
        let Some(at) = at.parse::<jiff::Timestamp>().ok() else {
            return String::new();
        };
        let at = at.to_zoned(zone.clone());
        let today = i64::try_from(feed.now)
            .ok()
            .and_then(|now| jiff::Timestamp::from_second(now).ok())
            .map(|now| now.to_zoned(zone.clone()).date());
        return if Some(at.date()) == today {
            at.strftime("%H:%M").to_string()
        } else {
            at.strftime("%b %-d").to_string()
        };
    }
    let Some(at) = seconds(at) else {
        return String::new();
    };
    let offset = offset(feed);
    let local = at + offset;
    let today = (i64::try_from(feed.now).unwrap_or(0) + offset).div_euclid(86_400);
    if local.div_euclid(86_400) == today {
        clock(local)
    } else {
        day(local)
    }
}

/// A daemon instant with its day and time, for attribution: `Nov 12 18:40`, or `18:40` today.
pub fn when(at: &str, feed: &Feed) -> String {
    if let Some(zone) = &feed.time_zone {
        let Some(at) = at.parse::<jiff::Timestamp>().ok() else {
            return String::new();
        };
        let at = at.to_zoned(zone.clone());
        let today = i64::try_from(feed.now)
            .ok()
            .and_then(|now| jiff::Timestamp::from_second(now).ok())
            .map(|now| now.to_zoned(zone.clone()).date());
        return if Some(at.date()) == today {
            at.strftime("%H:%M").to_string()
        } else {
            at.strftime("%b %-d %H:%M").to_string()
        };
    }
    let Some(at) = seconds(at) else {
        return String::new();
    };
    let offset = offset(feed);
    let local = at + offset;
    let today = (i64::try_from(feed.now).unwrap_or(0) + offset).div_euclid(86_400);
    if local.div_euclid(86_400) == today {
        clock(local)
    } else {
        format!("{} {}", day(local), clock(local))
    }
}

/// The CLI's elapsed-age units, including minutes for ages under a day.
pub fn age(since: Option<u64>, now: u64) -> String {
    let Some(since) = since else {
        return String::new();
    };
    let seconds = now.saturating_sub(since);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d", seconds / 86400),
    }
}

/// Age of a daemon instant; empty when unparseable.
pub fn ago(at: &str, feed: &Feed) -> String {
    age(seconds(at).and_then(|at| u64::try_from(at).ok()), feed.now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instants_read_in_the_ticks_local_offset_across_midnight() {
        // 2026-11-14T23:30:00Z read as 00:30 local: UTC+1, already the 15th.
        let mut feed = Feed {
            date: "2026-11-15".into(),
            ..Feed::default()
        };
        feed.tick(1_794_699_000, "00:30".into());
        assert_eq!(stamp("2026-11-14T23:10:00Z", &feed), "00:10");
        assert_eq!(stamp("2026-11-14T22:10:00Z", &feed), "Nov 14");
        assert_eq!(when("2026-11-14T22:10:00Z", &feed), "Nov 14 23:10");
        assert_eq!(stamp("not a time", &feed), "");
    }

    #[test]
    fn local_dates_disambiguate_offsets_twenty_four_hours_apart() {
        let mut feed = Feed {
            now: "2026-11-14T12:00:00Z"
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_second() as u64,
            clock: "02:00".into(),
            date: "2026-11-15".into(),
            ..Feed::default()
        };
        assert_eq!(when("2026-11-13T12:00:00Z", &feed), "Nov 14 02:00");
        feed.date = "2026-11-14".into();
        assert_eq!(when("2026-11-13T12:00:00Z", &feed), "Nov 13 02:00");
        assert_eq!(age(Some(feed.now - 11_100), feed.now), "3h05m");
    }
}
