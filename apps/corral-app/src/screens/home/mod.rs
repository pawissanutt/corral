//! Home (`model::home`): what waits on the owner, what is working and what just finished,
//! tasks and sessions alike. On the desktop and a terminal wide enough it is a dashboard
//! (`dash`): tiles, the Needs you cards with one open to decide in (`cards`) and the panels of
//! what Corral is doing (`panels`), with Ask's composer docked under it. A narrower terminal
//! falls back to the three sections as one list with each row's next step (`list`); the phone
//! keeps that list and answers small things in place (`phone`). In a workspace with no projects
//! Home is the assistant's landing (`screens::landing`, which the router shows in its place).

mod cards;
mod dash;
mod list;
mod panels;
mod parts;
mod phone;

pub(crate) use dash::tile;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::term;

/// The least page, in Cells, a terminal draws the dashboard on: two halves of 48 and the
/// frame between them.
const DASH_FROM_CELLS: f32 = 100.0;

pub fn screen(app: AppState) -> AnyElement {
    let scope = computed(move || app.project_scope());
    if app.form == Form::Phone {
        return phone::screen(app, scope);
    }
    let wide = computed(move || match app.form {
        Form::Terminal => term::page_columns(app) >= DASH_FROM_CELLS,
        Form::Desktop | Form::Phone => true,
    });
    // The keys walk the cards on the dashboard and the rows of the list.
    effect(move || {
        let wide = wide.get();
        if untracked(|| app.home.with(|state| state.wide != wide)) {
            untracked(|| app.home.update(|state| state.wide = wide));
        }
    });
    dynamic(move || {
        let wide = wide.get();
        untracked(move || {
            if wide {
                dash::dashboard(app, scope)
            } else {
                list::screen(app, scope)
            }
        })
    })
    .into_element()
}
