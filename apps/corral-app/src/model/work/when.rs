//! The daemon's RFC 3339 instants in this device's wall clock, without reading a time zone
//! database: the client's tick carries both the Unix time and the local `HH:MM` it reads, and
//! their difference is the local offset.

use crate::model::daemon::Feed;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The local offset from UTC in seconds, from the feed's last tick; zero before the first tick.
fn offset(feed: &Feed) -> i64 {
    let Some((hours, minutes)) = feed.clock.split_once(':') else {
        return 0;
    };
    let (Ok(hours), Ok(minutes)) = (hours.parse::<i64>(), minutes.parse::<i64>()) else {
        return 0;
    };
    let utc = i64::try_from((feed.now / 60) % 1440).unwrap_or(0);
    let mut minutes = hours * 60 + minutes - utc;
    // Offsets run from UTC−12 to UTC+14; anything else crossed midnight.
    if minutes > 14 * 60 {
        minutes -= 1440;
    } else if minutes < -12 * 60 {
        minutes += 1440;
    }
    minutes * 60
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

/// Whole-unit age of a daemon instant: `12s`, `4m`, `3h`, `2d`; empty when unparseable.
pub fn ago(at: &str, feed: &Feed) -> String {
    let since = seconds(at).and_then(|at| u64::try_from(at).ok());
    crate::model::daemon::age(since, feed.now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instants_read_in_the_ticks_local_offset_across_midnight() {
        let mut feed = Feed::default();
        // 2026-11-14T23:30:00Z read as 00:30 local: UTC+1, already the 15th.
        feed.tick(1_794_699_000, "00:30".into());
        assert_eq!(stamp("2026-11-14T23:10:00Z", &feed), "00:10");
        assert_eq!(stamp("2026-11-14T22:10:00Z", &feed), "Nov 14");
        assert_eq!(when("2026-11-14T22:10:00Z", &feed), "Nov 14 23:10");
        assert_eq!(stamp("not a time", &feed), "");
    }
}
