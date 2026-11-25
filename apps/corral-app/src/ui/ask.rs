//! The Ask surfaces beside the composer: the global hotkey window and the phone share sheet,
//! with the model pill and list they share.

mod attach;
mod docked;
pub(crate) mod drawer;
mod hotkey;
pub(crate) mod models;
mod share;
mod unconnected;

pub(crate) use attach::{
    chip_row, controls as attach_controls, dock_context, field_row, plus, tray,
};
pub(crate) use docked::{beside_note, open_beside};

use nsh_core::prelude::*;

use crate::model::{Id, assistant};
use crate::state::AppState;

/// Runs one of the assistant model's typed actions through the feed-backed reducer, which
/// returns the canonical effects (Remove context clears Ask's real context).
pub(crate) fn act(app: AppState, action: assistant::Action) {
    match action {
        assistant::Action::Unchip => {
            app.ask_context.set(None);
            return;
        }
        assistant::Action::UnchipProject => {
            app.remove_composer_project();
            return;
        }
        _ => {}
    }
    app.reduce(app.assistant, move |state, feed, _, _| {
        state.act_live(action, feed)
    });
}

/// Runs an attachment control of `session`'s composer: attaching and removing act on that
/// session's next message alone. Attaching runs at the root, in this press: a browser opens its
/// file picker only from inside one.
pub(crate) fn act_on(app: AppState, session: &Id, action: assistant::Action) {
    match action {
        assistant::Action::Attach(pick) => app.attach(session, pick),
        assistant::Action::AttachLink => {
            app.open_attach_field(session, assistant::AttachField::Link);
        }
        assistant::Action::Unattach(key) => app.unattach(Some(session), &Id::from(key)),
        action => act(app, action),
    }
}

/// The labelled preview's small Ask window, shown while `app.hotkey` holds: on desktop a window
/// centred over a dimmed backdrop that stands in for the desktop, closed by Esc or its Hide
/// control; in a terminal the same content in the form's modal. Draws nothing while closed.
pub fn hotkey(app: AppState) -> AnyElement {
    hotkey::hotkey(app)
}

/// The labelled preview's phone share extension over the shared page, shown while `app.share`
/// holds; it draws nothing while closed.
pub fn share(app: AppState) -> AnyElement {
    share::share(app)
}
