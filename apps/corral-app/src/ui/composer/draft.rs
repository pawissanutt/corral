//! Keeping Ask's unsent draft on this device, under the session it goes to.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;

use crate::model::session::Action;
use crate::model::{Effect, Id};
use crate::state::AppState;

/// How long typing pauses before the draft is kept on this device.
const DRAFT_PAUSE: Duration = Duration::from_millis(800);

/// Keeps session `id`'s draft on this device a moment after typing pauses, and at once when the
/// composer goes before that moment, and brings the saved one back into an empty draft when the
/// device's outbox has it (after a restart, or the words of a send whose answer was lost). A
/// sent message's words left the draft at once and its saved copy went with it in the outbox's
/// own write, so nothing sent comes back. A composer that opens empty reports nothing until its
/// draft changes: its empty first value must not erase a saved draft not yet brought back. Every
/// write goes to the source the composer mounted on, so words typed in the sample preview never
/// reach the daemon's outbox after the data source changes, nor the other way round.
pub(super) fn keep_draft(app: AppState, id: Id) {
    let scoped = app.scope();
    let saved = {
        let id = id.clone();
        computed(move || {
            scoped
                .feed
                .with(|feed| feed.drafts.get(id.as_str()).cloned())
        })
    };
    let draft = {
        let id = id.clone();
        computed(move || {
            scoped
                .session
                .with(|state| state.ui(id.as_str()).draft.clone())
        })
    };
    {
        let id = id.clone();
        effect(move || {
            let Some(saved) = saved.get().filter(|saved| !saved.is_empty()) else {
                return;
            };
            if draft.with_untracked(String::is_empty) {
                let session = id.clone();
                untracked(|| {
                    scoped.reduce(scoped.session, move |state, feed, _, _| {
                        state.act(
                            Action::Draft {
                                session,
                                text: saved,
                            },
                            feed,
                        )
                    });
                });
            }
        });
    }
    // Plain Rust rather than a signal: the cleanup below must still read it while the owner goes.
    let unsaved: Rc<RefCell<Option<String>>> = Rc::default();
    let timer = stored(None::<Spawned<()>>);
    let started = stored(false);
    {
        let unsaved = unsaved.clone();
        let id = id.clone();
        effect(move || {
            let text = draft.get();
            if let Some(old) = timer.replace(None) {
                old.cancel();
            }
            if !started.replace(true) && text.is_empty() {
                return;
            }
            *unsaved.borrow_mut() = Some(text);
            let unsaved = unsaved.clone();
            let id = id.clone();
            let task = set_timeout(DRAFT_PAUSE, move || {
                if let Some(text) = unsaved.borrow_mut().take() {
                    keep(scoped, &id, text);
                }
            });
            let _ = timer.replace(Some(task));
        });
    }
    // Leaving ends the timer with the composer; what it would have written goes now.
    cleanup(move || {
        if let Some(text) = unsaved.borrow_mut().take() {
            keep(scoped, &id, text);
        }
    });
}

/// Writes `text` as the session's saved draft unless it already is. An untouched composer never
/// writes, and so never reports a missing device outbox before pairing. Folded pastes are kept
/// in memory only, so the saved copy carries their text in place of each `[paste #k …]` marker:
/// a draft brought back after a restart still sends what was pasted.
fn keep(app: AppState, id: &Id, text: String) {
    let text = app.expand_pastes(id, &text);
    let kept = app
        .feed
        .with_untracked(|feed| feed.drafts.get(id.as_str()).cloned());
    if kept.as_deref().unwrap_or("") == text {
        return;
    }
    app.run(vec![Effect::Draft {
        thread: id.to_string(),
        text,
    }]);
}
