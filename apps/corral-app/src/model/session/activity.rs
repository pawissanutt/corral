//! The working indicator: what the session's agent is doing now, in the host's words (its
//! activity phase), with how long it has been at it; its short form for narrow places; and for a
//! few seconds after a turn ends, how it ended. Nothing is synthesised: without a phase from the
//! host an open turn says `working`, and a closed one says nothing.

use corral_protocol::session_state::SessionState;

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
    if lower.contains("waiting for you") || lower.contains("needs you") {
        (Hue::You, Some("▲"), false)
    } else if lower.starts_with("no data") || lower.contains("retrying") || lower.contains("stall")
    {
        (Hue::Held, Some("!"), false)
    } else {
        (Hue::Worker, None, true)
    }
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
        let phase = activity.map_or("working", |activity| activity.phase.as_str());
        let (hue, glyph, spinner) = tone(phase);
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
                phase.to_owned()
            },
            short: short(phase),
            elapsed: since(started, feed).map(duration),
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
