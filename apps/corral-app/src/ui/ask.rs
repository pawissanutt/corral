//! The Ask surfaces beside the composer: the composer docked under a thread, the global hotkey
//! window and the phone share sheet, with the model pill and list they share.

mod attach;
pub(crate) mod drawer;
mod hotkey;
pub(crate) mod models;
mod share;
mod unconnected;

pub(crate) use attach::{chip_row, field_row, foot_controls, plus, tray};

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{self, ThreadRef};
use crate::state::AppState;
use crate::ui::composer::{Host, ListAt, field};

/// Runs one of the assistant model's typed actions through the feed-backed reducer, which
/// returns the canonical effects (Remove context clears Ask's real context). Attaching runs at
/// the root, in this press: a browser opens its file picker only from inside one.
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
        assistant::Action::Attach(pick) => {
            app.attach(pick);
            return;
        }
        assistant::Action::AttachLink => {
            app.open_attach_field(assistant::AttachField::Link);
            return;
        }
        _ => {}
    }
    app.reduce(app.assistant, move |state, feed, _, _| {
        state.act_live(action, feed)
    });
}

/// The composer docked under a thread page. The quick thread keeps the Assistant | PM switch
/// and the model pill; a named session talks to that session only, so it has no switch, and a
/// pane session has no pill (Pi's own picker). Its model list opens above the field on desktop
/// and in a terminal, inline under the pill on a phone.
pub fn docked(app: AppState, thread: ThreadRef) -> AnyElement {
    let list = if app.form == Form::Phone {
        ListAt::Inline
    } else {
        ListAt::Above
    };
    field(
        app,
        Host {
            thread,
            list,
            docked: true,
        },
    )
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
