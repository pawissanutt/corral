//! Activity: the five pages under the one place. Feed is every event, Timeline the lanes and
//! the burn-up, Usage the totals, Artifacts the finder and System the project's map, each under the one project scope.
//! The sidebar lists them under Activity while one shows (its rail too), and is the only way
//! between them on desktop and in a terminal; the phone's Activity tab lists them as its root
//! and pushes each. On desktop every page keeps its head and content to `MEASURE_PAGE`, centred,
//! so the title stays put from one page to the next.

mod feed;
mod feed_filter;

use nsh_core::prelude::*;

use crate::model::Destination;
use crate::screens::{artifacts, system, timeline, usage};
use crate::state::AppState;

/// The Activity page `page` names; Feed for a destination that is not one of Activity's.
pub fn screen(app: AppState, page: Destination) -> AnyElement {
    match page {
        Destination::Timeline => timeline::screen(app),
        Destination::Usage => usage::screen(app),
        Destination::Artifacts => artifacts::screen(app),
        Destination::System => system::screen(app),
        _ => feed::screen(app),
    }
}
