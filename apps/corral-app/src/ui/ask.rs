//! The Ask surfaces beside the composer: the composer docked under a thread, the global hotkey
//! window and the phone share sheet, with the model pill and list they share.

mod attach;
pub(crate) mod drawer;
mod hotkey;
pub(crate) mod models;
mod share;

pub(crate) use attach::{chip_row, field_row, foot_controls, tray};

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{self, ThreadRef};
use crate::state::AppState;
use crate::ui::composer::{Host, ListAt, field};

/// Runs one of the assistant model's typed actions. With the daemon the live reducer returns the
/// canonical effects (Remove context clears Ask's real context); in the fixture the preview's
/// reducer gets the app's draft and target. Attaching runs at the root, in this press: a
/// browser opens its file picker only from inside one.
pub(crate) fn act(app: AppState, action: assistant::Action) {
    if untracked(|| app.daemon()) {
        match action {
            assistant::Action::Unchip => {
                app.ask_context.set(None);
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
        return;
    }
    let target = app.ask_target.get_untracked();
    let text = app.ask_draft.get_untracked();
    app.act(app.assistant, move |state, env| {
        state.act(action, &text, target, env)
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

/// The small Ask window the global shortcut opens, shown while `app.hotkey` holds: on desktop a
/// window centred over a dimmed backdrop that stands in for the desktop, closed by Esc or its
/// Hide control; on a phone and in a terminal the same content in the form's modal. Draws nothing
/// while closed. A fixture preview demo only.
pub fn hotkey(app: AppState) -> AnyElement {
    hotkey::hotkey(app)
}

/// The phone share sheet's steps over the shared page, shown while `app.share` holds. A fixture
/// preview demo only; it draws nothing while closed.
pub fn share(app: AppState) -> AnyElement {
    share::share(app)
}
