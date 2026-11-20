//! Activity: the four pages under the one place. Feed is what changed, Timeline the lanes,
//! Usage the totals and Artifacts the finder; the sidebar lists them under Activity while it is
//! open, and the phone's Activity tab lists them as its root.
//!
//! This is the first cut of Activity: each page is the screen of the former destination
//! (`screens::changes`, `timeline`, `usage`, `artifacts`), under the one project scope.

use nsh_core::prelude::*;

use crate::model::Destination;
use crate::screens::{artifacts, changes, timeline, usage};
use crate::state::AppState;

/// The Activity page `page` names; Feed for a destination that is not one of Activity's.
pub fn screen(app: AppState, page: Destination) -> AnyElement {
    match page {
        Destination::Timeline => timeline::screen(app),
        Destination::Usage => usage::screen(app),
        Destination::Artifacts => artifacts::screen(app),
        _ => changes::screen(app),
    }
}
