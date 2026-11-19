//! The one read-state vocabulary every session part shares: not read yet, reading, read at a
//! time with its age, stale since a time, failed with corrald's reason (nothing is inferred
//! then), and does not apply. A check that ran on an older commit is a different stale and is
//! said by the part that has it, never merged with this copy being old.

use crate::model::daemon::{DetailCache, Feed, Link};
use crate::model::hue::Hue;

/// Where one read part stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadState {
    /// Never asked for.
    NotRead,
    /// Asked for; nothing to show yet.
    Reading,
    /// Asked for again; the rows shown are the last ones read.
    ReadingKept,
    /// Read at `at` (`12:04`), `ago` old (`2m`).
    Read { at: String, ago: String },
    /// The rows shown were read at `at` and are no longer current.
    Stale { at: String },
    /// The read failed; no rows are shown and nothing is inferred.
    Failed(String),
    /// The part does not exist for this session.
    DoesNotApply(String),
}

impl ReadState {
    /// The state of the detail `cache` as `feed` holds it.
    pub fn of(cache: Option<&DetailCache>, feed: &Feed) -> Self {
        let Some(cache) = cache else {
            return Self::NotRead;
        };
        let read_at = cache
            .pages
            .first()
            .map(|page| page.as_of.clone())
            .unwrap_or_default();
        let at = crate::model::agents::clock(Some(&read_at), feed)
            .unwrap_or_else(|| "an unknown time".into());
        let has_rows = cache.body().is_some();
        match (&cache.error, has_rows) {
            (Some(error), false) => {
                return Self::Failed(format!("{} ({})", error.message, error.code));
            }
            (Some(_), true) => return Self::Stale { at },
            (None, _) => {}
        }
        if cache.loading {
            return if has_rows {
                Self::ReadingKept
            } else {
                Self::Reading
            };
        }
        if !has_rows {
            return Self::NotRead;
        }
        if cache.stale || feed.link != Link::Up {
            return Self::Stale { at };
        }
        let ago = crate::model::agents::unix(&read_at)
            .filter(|_| feed.now > 0)
            .map(|when| crate::model::daemon::age(Some(when), feed.now))
            .unwrap_or_default();
        Self::Read { at, ago }
    }

    /// The words, as a chip or a line says them.
    pub fn words(&self) -> String {
        match self {
            Self::NotRead => "not read yet".into(),
            Self::Reading => "reading…".into(),
            Self::ReadingKept => "reading… rows kept".into(),
            Self::Read { at, ago } if ago.is_empty() => format!("read {at}"),
            Self::Read { at, ago } => format!("read {at} ({ago} ago)"),
            Self::Stale { at } => format!("! stale · {at}"),
            Self::Failed(why) => format!("! read failed: {why} · nothing is inferred"),
            Self::DoesNotApply(why) => format!("○ Does not apply: {why}"),
        }
    }

    pub fn hue(&self) -> Hue {
        match self {
            Self::Stale { .. } => Hue::Held,
            Self::Failed(_) => Hue::Fail,
            _ => Hue::Neutral,
        }
    }

    /// Whether rows are there to show.
    pub fn has_rows(&self) -> bool {
        matches!(
            self,
            Self::ReadingKept | Self::Read { .. } | Self::Stale { .. }
        )
    }
}
