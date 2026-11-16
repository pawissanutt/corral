//! What the shell's top bar says in short: one word while every component is healthy (the rest in
//! its hint), the date and time, the tag over the sample preview, and a destination's bare count
//! in words. Health is never folded away while something is wrong: then every component shows by
//! name, as the full line always did.

use std::borrow::Cow;

use crate::model::daemon::Feed;
use crate::model::nav::Destination;

use super::{DataState, Snapshot, Tone, day_label};

/// The one word the line reads while every component is healthy.
pub const HEALTHY: &str = "healthy";
/// The tag over the sample preview's data, so it never reads as live.
pub const SAMPLE: &str = "sample";

/// The health line as one chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condensed {
    /// What the chip reads.
    pub word: &'static str,
    /// Each component and the reach, one sentence each:
    /// `corrald healthy · herdr-core ok · PM live · reach direct, loopback`.
    pub hint: String,
}

/// The line as one chip while it is connected and every component reads healthy; `None` as soon
/// as one does not, so that component shows by name.
pub fn condensed(snapshot: &Snapshot) -> Option<Condensed> {
    let health = &snapshot.health;
    let healthy = snapshot.connected()
        && !health.segments.is_empty()
        && health
            .segments
            .iter()
            .all(|segment| segment.tone == Tone::Ok);
    healthy.then(|| Condensed {
        word: HEALTHY,
        hint: health
            .segments
            .iter()
            .map(|segment| format!("{} {}", segment.name, segment.value))
            .chain(
                health
                    .reach
                    .as_deref()
                    .map(|reach| format!("reach {}", reach.replace(" · ", ", "))),
            )
            .collect::<Vec<_>>()
            .join(" · "),
    })
}

/// The day and time the top bar shows, `Mon 16 Nov 14:05`, from the feed's real clock; whichever
/// half is known when one is not.
pub fn stamp(feed: &Feed) -> Option<String> {
    match (day_label(&feed.date), feed.clock.is_empty()) {
        (Some(day), false) => Some(format!("{day} {}", feed.clock)),
        (Some(day), true) => Some(day),
        (None, false) => Some(feed.clock.clone()),
        (None, true) => None,
    }
}

/// The tag over the sample preview: its word, and the source's own explanation as its hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleTag {
    pub word: &'static str,
    pub hint: Cow<'static, str>,
}

/// The tag while the sample preview is the data source; nothing for daemon data.
pub fn sample_tag(snapshot: &Snapshot) -> Option<SampleTag> {
    (snapshot.state == DataState::Fixture).then(|| SampleTag {
        word: SAMPLE,
        hint: snapshot
            .health
            .source
            .clone()
            .unwrap_or(Cow::Borrowed(DataState::Fixture.label())),
    })
}

/// What waits on the owner, said in full: `1 waits on you`, `3 wait on you`.
pub fn waits_on_you(n: u32) -> String {
    format!("{n} {} on you", if n == 1 { "waits" } else { "wait" })
}

impl Snapshot {
    /// A destination's bare count said in full, for its hint and its row's name: what needs the
    /// owner as [`waits_on_you`], any other count as its figure.
    pub fn count_words(&self, destination: Destination) -> Option<String> {
        self.count(destination).map(|n| {
            if self.needs_you(destination) {
                waits_on_you(n)
            } else {
                n.to_string()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Health, Segment};

    fn snapshot(tones: &[Tone]) -> Snapshot {
        Snapshot::new(
            DataState::Daemon,
            Health {
                segments: tones
                    .iter()
                    .map(|tone| Segment {
                        name: "part".into(),
                        value: "value".into(),
                        tone: *tone,
                    })
                    .collect(),
                reach: Some("direct · loopback".into()),
                e2e: false,
                attention: None,
                clock: None,
                source: None,
            },
            None,
            Vec::new(),
        )
    }

    /// Health is never hidden behind a hint while a component is not healthy.
    #[test]
    fn only_an_all_healthy_line_condenses() {
        assert!(condensed(&snapshot(&[Tone::Ok, Tone::Ok])).is_some());
        for unwell in [Tone::Warn, Tone::Fail, Tone::Muted] {
            assert_eq!(condensed(&snapshot(&[Tone::Ok, unwell])), None);
        }
        assert_eq!(condensed(&snapshot(&[])), None);
        assert_eq!(condensed(&crate::model::disconnected::snapshot()), None);
    }
}
