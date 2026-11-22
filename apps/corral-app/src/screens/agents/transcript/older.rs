//! Reading back through a transcript: as the reader scrolls up near its first row, the page
//! before it is asked for once, with no press; when it arrives above, the row the reader is at
//! stays where it was on screen (`jump::follow` holds it). The Show earlier messages control
//! stays for a page that failed or a transcript too short to scroll.

use crate::model::session::Action;
use crate::model::session::transcript::Transcript;

/// How near the top, in heights of the view, the reader is when the page before is asked for:
/// one view's worth ahead, so it usually lands before the reader reaches the first row.
const NEAR: f32 = 1.0;

/// What a transcript offers before its first row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Earlier {
    /// The request for the page before the shown messages; none at the transcript's start.
    pub older: Option<Action>,
    /// Where that page starts: each page read gives the next one its own.
    pub cursor: Option<String>,
    /// A read of the transcript is on its way: nothing more is asked until it lands.
    pub paging: bool,
    /// The first row shown (`Transcript::first`): it changes when rows arrive above, also
    /// inside a record split across pages.
    pub first: Option<String>,
}

impl Earlier {
    pub fn of(transcript: &Transcript) -> Self {
        Self {
            older: transcript.older.clone(),
            cursor: transcript.older_cursor.clone(),
            paging: transcript.paging,
            first: transcript.first.clone(),
        }
    }
}

/// When the page before is asked for: once per page. A page that failed, or a re-read that
/// offers the same page, is asked for again only after the reader has scrolled away from the
/// top and back, so reading near the top never asks in a loop; a page that showed nothing new
/// above offers the next one, which is asked for while the reader stays near the top.
#[derive(Debug, Default)]
pub(super) struct Reach {
    /// The page last asked for.
    asked: Option<String>,
    /// The reader was away from the top since then.
    away: bool,
    /// The first row as last seen.
    first: Option<String>,
    /// The last scroll offset and view height, until the first row changes.
    seen: Option<(f32, f32)>,
}

impl Reach {
    /// The request to run for the reader at `offset` in a view `height` high, at the end or
    /// not (`at_end`): the page before, when it is near, offered and not asked for yet.
    pub fn scrolled(
        &mut self,
        earlier: Option<&Earlier>,
        offset: f32,
        height: f32,
        at_end: bool,
    ) -> Option<Action> {
        self.seen = Some((offset, height));
        self.wanted(earlier, at_end)
    }

    /// The request to run now that `earlier` changed, from where the reader was last seen: a
    /// read that held the request back has landed. Rows that arrived above move the reader, so
    /// then nothing is asked until the reader's next scroll says where they are.
    pub fn changed(&mut self, earlier: Option<&Earlier>, at_end: bool) -> Option<Action> {
        let first = earlier.and_then(|earlier| earlier.first.clone());
        if first != self.first {
            self.first = first;
            self.seen = None;
        }
        self.wanted(earlier, at_end)
    }

    /// Another scope shows: nothing was asked of it yet.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn wanted(&mut self, earlier: Option<&Earlier>, at_end: bool) -> Option<Action> {
        let (offset, height) = self.seen?;
        if offset > height * NEAR {
            self.away = true;
            return None;
        }
        // Opening at the end of a short transcript is not reading back through it.
        let earlier = earlier.filter(|earlier| !at_end && !earlier.paging)?;
        if !self.away && self.asked == earlier.cursor {
            return None;
        }
        let older = earlier.older.clone()?;
        self.asked = earlier.cursor.clone();
        self.away = false;
        Some(older)
    }
}
