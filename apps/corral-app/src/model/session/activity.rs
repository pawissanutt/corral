//! The working indicator: what the session's agent is doing now, in the host's words (its
//! activity phase), with how long it has been at it; its short form for narrow places; and for a
//! few seconds after a turn ends, how it ended. Nothing is synthesised: without a phase from the
//! host an open turn says `working`, and a closed one says nothing.

use corral_protocol::session_state::{Activity, SessionState};

use crate::model::daemon::Feed;
use crate::model::hue::Hue;

use super::transcript::LiveSession;

/// Seconds a settled turn's ending stays in the indicator.
const SETTLED_FOR: u64 = 4;

/// One working-indicator line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityLine {
    /// Drawn with the running spinner (a steady `●` under reduced motion).
    pub spinner: bool,
    /// The glyph instead of the spinner: `▲` waiting for you, `!` stalled, `✓` done, `✗` stopped.
    pub glyph: Option<&'static str>,
    pub hue: Hue,
    /// `Working · running bash`, `▲ waiting for you`, `✓ done`.
    pub phase: String,
    /// The phase cut for narrow places: `bash`, `waiting`, `done`.
    pub short: String,
    /// `12s`, `3m 12s`; none when the host stamped no start.
    pub elapsed: Option<String>,
}

/// Seconds as the indicator writes them: `42s`, `3m 12s`, `1h 04m`.
pub fn duration(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

fn since(at: Option<&str>, feed: &Feed) -> Option<u64> {
    let start = crate::model::agents::unix(at?)?;
    (feed.now > 0 && start <= feed.now).then(|| feed.now - start)
}

/// The phase's hue: the owner's colour while it waits on the owner, held while it stalls or
/// retries, the worker's colour otherwise.
fn tone(phase: &str) -> (Hue, Option<&'static str>, bool) {
    let lower = phase.to_ascii_lowercase();
    if lower.starts_with("retrying") {
        (Hue::Held, Some("!"), false)
    } else if lower.contains("waiting for you") || lower.contains("needs you") {
        (Hue::You, Some("▲"), false)
    } else if lower.starts_with("no data") || lower.contains("retrying") || lower.contains("stall")
    {
        (Hue::Held, Some("!"), false)
    } else {
        (Hue::Worker, None, true)
    }
}

/// Seconds until the host's next retry, from its optional `retry_at` stamp: bounded at zero once
/// the moment passes, none when the host gave no stamp, it does not parse, or the clock is unknown.
fn retry_in(activity: &Activity, feed: &Feed) -> Option<u64> {
    let at = crate::model::agents::unix(activity.retry_at.as_deref()?)?;
    (feed.now > 0).then(|| at.saturating_sub(feed.now))
}

/// The host's phase, its held tone, and the retry countdown when the host stamped one. The phase
/// text (the error and attempt) stays the host's; the countdown is appended to it.
fn shown(activity: &Activity, feed: &Feed) -> (String, Hue, Option<&'static str>, bool) {
    let (mut hue, mut glyph, mut spinner) = tone(&activity.phase);
    let mut phase = activity.phase.clone();
    if let Some(left) = retry_in(activity, feed) {
        (hue, glyph, spinner) = (Hue::Held, Some("!"), false);
        phase = format!("{phase} · retry in {}", duration(left));
    }
    (phase, hue, glyph, spinner)
}

/// The phase for narrow places: its last word after `running`, else its first two words.
pub fn short(phase: &str) -> String {
    if let Some(rest) = phase.strip_prefix("running ") {
        return rest.rsplit(' ').next().unwrap_or(rest).to_owned();
    }
    let words: Vec<&str> = phase.split_whitespace().take(2).collect();
    words.join(" ")
}

/// The indicator for a session, from its host state and live turn.
pub fn line(
    state: Option<&SessionState>,
    live: Option<&LiveSession>,
    feed: &Feed,
) -> Option<ActivityLine> {
    let open = state.is_some_and(|state| state.open) || live.is_some_and(|live| live.open);
    if open {
        let activity = state.and_then(|state| state.activity.as_ref());
        let base = activity.map_or("working", |activity| activity.phase.as_str());
        let (phase, hue, glyph, spinner) = match activity {
            Some(activity) => shown(activity, feed),
            None => {
                let (hue, glyph, spinner) = tone(base);
                (base.to_owned(), hue, glyph, spinner)
            }
        };
        let started = live
            .and_then(|live| live.started_at.as_deref())
            .or(activity.map(|activity| activity.since.as_str()));
        return Some(ActivityLine {
            spinner,
            glyph,
            hue,
            phase: if spinner {
                format!("Working · {phase}")
            } else {
                phase
            },
            short: short(base),
            elapsed: since(started, feed).map(duration),
        });
    }
    // Work the host reports with no turn open (compaction, idle or after a turn): its phase
    // alone, as the host says it.
    if let Some(activity) = state.and_then(|state| state.activity.as_ref()) {
        let (shown_phase, hue, glyph, spinner) = shown(activity, feed);
        let mut letters = shown_phase.chars();
        let phase: String = letters
            .next()
            .map(|first| first.to_uppercase().chain(letters).collect())
            .unwrap_or_default();
        return Some(ActivityLine {
            spinner,
            glyph,
            hue,
            phase,
            short: short(&activity.phase),
            elapsed: since(Some(activity.since.as_str()), feed).map(duration),
        });
    }
    let live = live?;
    let end = live.ended.as_ref()?;
    let ended = crate::model::agents::unix(live.ended_at.as_deref()?)?;
    if feed.now == 0 || feed.now >= ended + SETTLED_FOR {
        return None;
    }
    let ran = live
        .started_at
        .as_deref()
        .and_then(crate::model::agents::unix)
        .filter(|start| *start <= ended)
        .map(|start| duration(ended - start));
    let (glyph, hue, word) = if end.error.is_some() || end.aborted {
        ("✗", Hue::Fail, "stopped")
    } else {
        ("✓", Hue::Ok, "done")
    };
    Some(ActivityLine {
        spinner: false,
        glyph: Some(glyph),
        hue,
        phase: word.to_owned(),
        short: word.to_owned(),
        elapsed: ran,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compaction runs outside any turn (a manual `/compact` while idle, or after a turn ends).
    /// The host reports it as the session's activity with no open turn, and the indicator
    /// stayed blank, so the owner never saw it.
    #[test]
    fn an_idle_session_with_compacting_activity_shows_an_indicator() {
        let state = SessionState {
            open: false,
            activity: Some(Activity {
                phase: "compacting".into(),
                since: "2026-01-01T00:00:00Z".into(),
                retry_at: None,
            }),
            ..SessionState::default()
        };
        let line = line(Some(&state), None, &Feed::default())
            .expect("compaction with no open turn is shown");
        assert!(line.spinner);
        assert_eq!(line.short, "compacting");
    }

    fn retrying(retry_at: Option<&str>) -> SessionState {
        SessionState {
            open: true,
            activity: Some(Activity {
                phase: "retrying after 529 (attempt 2/5)".into(),
                since: "2026-01-01T00:00:00Z".into(),
                retry_at: retry_at.map(str::to_owned),
            }),
            ..SessionState::default()
        }
    }

    fn at(now: &str) -> Feed {
        Feed {
            now: crate::model::agents::unix(now).expect("a valid stamp"),
            ..Feed::default()
        }
    }

    /// A retrying host may stamp when the next attempt is due. The indicator keeps the host's
    /// held tone and its error and attempt text, counts down from `feed.now`, never goes below
    /// zero, and shows no countdown when the host gave no stamp or the clock is unknown.
    #[test]
    fn a_retrying_session_counts_down_to_the_next_attempt_and_holds() {
        let state = retrying(Some("2026-01-01T00:01:10Z"));

        let early = line(Some(&state), None, &at("2026-01-01T00:00:00Z")).expect("shown");
        assert_eq!(early.hue, Hue::Held);
        assert_eq!(early.glyph, Some("!"));
        assert!(!early.spinner);
        assert_eq!(
            early.phase,
            "retrying after 529 (attempt 2/5) · retry in 1m 10s"
        );

        let later = line(Some(&state), None, &at("2026-01-01T00:01:00Z")).expect("shown");
        assert_eq!(
            later.phase,
            "retrying after 529 (attempt 2/5) · retry in 10s"
        );
        assert_eq!(later.hue, Hue::Held);

        let past = line(Some(&state), None, &at("2026-01-01T00:02:00Z")).expect("shown");
        assert_eq!(past.phase, "retrying after 529 (attempt 2/5) · retry in 0s");
        assert_eq!(past.hue, Hue::Held);

        let unknown = line(Some(&state), None, &Feed::default()).expect("shown");
        assert_eq!(unknown.phase, "retrying after 529 (attempt 2/5)");
        assert_eq!(unknown.hue, Hue::Held);

        let absent = line(Some(&retrying(None)), None, &at("2026-01-01T00:00:00Z")).expect("shown");
        assert_eq!(absent.phase, "retrying after 529 (attempt 2/5)");
        assert_eq!(absent.hue, Hue::Held);
        assert_eq!(absent.short, early.short);
    }
}
