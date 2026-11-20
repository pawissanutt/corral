//! Activity: the four pages under the one place. Feed is every event, Timeline the lanes and
//! the burn-up, Usage the totals and Artifacts the finder, each under the one project scope.
//! The sidebar lists them under Activity while one shows, and on desktop and in a terminal a
//! strip under each page's head (`subnav`) moves between them; the phone's Activity tab lists
//! them as its root and pushes each.

mod feed;

use nsh_core::prelude::*;

use crate::model::{Destination, Form};
use crate::screens::{artifacts, timeline, usage};
use crate::state::AppState;
use crate::ui::kit::button::tab_strip;
use crate::ui::term;

/// The Activity page `page` names; Feed for a destination that is not one of Activity's.
pub fn screen(app: AppState, page: Destination) -> AnyElement {
    match page {
        Destination::Timeline => timeline::screen(app),
        Destination::Usage => usage::screen(app),
        Destination::Artifacts => artifacts::screen(app),
        _ => feed::screen(app),
    }
}

/// Activity's pages in the strip's order.
const PAGES: [Destination; 4] = [
    Destination::Feed,
    Destination::Timeline,
    Destination::Usage,
    Destination::Artifacts,
];

/// The strip under an Activity page's head (`.subnav`): Feed · Timeline · Usage · Artifacts,
/// the page shown selected, each opening its page (`subnav-<n>`). The phone has none: its
/// Activity tab lists the pages and pushes each.
pub(crate) fn subnav(app: AppState) -> Option<AnyElement> {
    let form = app.form;
    if form == Form::Phone {
        return None;
    }
    let options: Vec<(Destination, &'static str)> =
        PAGES.iter().map(|page| (*page, page.label())).collect();
    let strip = tab_strip(
        app,
        &options,
        move || app.destination(),
        move |page| app.go(page),
        "subnav",
    );
    let row = view().flex_none();
    Some(match form {
        Form::Terminal => row.px(term::cells(1.0)).child(strip).into_element(),
        Form::Desktop | Form::Phone => row.px(px(10.0)).child(strip).into_element(),
    })
}
