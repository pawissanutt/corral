//! Keeping the unsent draft of a served thread on this device.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;

use super::pending;
use crate::model::{Effect, Id};
use crate::state::AppState;

/// How long typing pauses before the draft is kept on this device.
const DRAFT_PAUSE: Duration = Duration::from_millis(800);

/// Keeps the unsent draft of a served thread on this device a moment after typing pauses, and
/// at once when the composer goes before that moment, and restores the saved one into an empty
/// composer when the session brings it back. Only the scope's own saved draft is watched, so
/// the accepted message's other updates never re-run the restore, and a draft whose words were
/// already sent is never brought back. A composer that opens empty reports nothing until its
/// draft changes: its empty first value must not erase a saved draft the session has yet to
/// bring back. Restoring, the paused write and the final write all go to the source the
/// composer mounted on, so words typed in the sample preview never reach the daemon's outbox
/// after the data source changes, nor the other way round.
pub(super) fn keep_draft(app: AppState, scope: Id, draft: Signal<String>) {
    let scoped = app.scope();
    let watched = scope.clone();
    let saved = computed(move || {
        scoped
            .feed
            .with(|feed| feed.drafts.get(watched.as_str()).cloned())
    });
    let restore = scope.clone();
    effect(move || {
        let saved = saved.get();
        let handed = scoped
            .assistant
            .with_untracked(|state| state.live.handed.get(restore.as_str()).cloned());
        if let Some(sent) = handed {
            if saved.as_ref() == Some(&sent) {
                return;
            }
            untracked(|| {
                scoped.assistant.update(|state| {
                    state.already_sent(restore.as_str(), saved.as_deref());
                });
            });
        }
        if let Some(saved) = saved.filter(|saved| !saved.is_empty())
            && draft.with_untracked(String::is_empty)
        {
            draft.set(saved);
        }
    });
    // Plain Rust rather than a signal: the cleanup below must still read it while the owner goes.
    let unsaved: Rc<RefCell<Option<String>>> = Rc::default();
    let timer = stored(None::<Spawned<()>>);
    let started = stored(false);
    {
        let unsaved = unsaved.clone();
        let scope = scope.clone();
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
            let scope = scope.clone();
            let task = set_timeout(DRAFT_PAUSE, move || {
                if let Some(text) = unsaved.borrow_mut().take() {
                    keep(scoped, &scope, text);
                }
            });
            let _ = timer.replace(Some(task));
        });
    }
    // Leaving ends the timer with the composer; what it would have written goes now.
    cleanup(move || {
        if let Some(text) = unsaved.borrow_mut().take() {
            keep(scoped, &scope, text);
        }
    });
}

/// Writes `text` as the scope's draft unless it already is, or its words are a message the
/// client has, or is about to have, taken.
pub(super) fn keep(app: AppState, scope: &Id, text: String) {
    // Nothing to keep or remove: an untouched composer never writes, and so never
    // reports a missing device outbox before pairing.
    let kept = app
        .feed
        .with_untracked(|feed| feed.drafts.get(scope.as_str()).cloned());
    if kept.as_deref().unwrap_or("") == text {
        return;
    }
    let handed = app.assistant.with_untracked(|state| {
        let sent = state.live.handed.get(scope.as_str());
        sent.is_some_and(|sent| *sent == text)
    });
    if handed || pending::sending(app, scope, &text) {
        return;
    }
    app.run(vec![Effect::Draft {
        thread: scope.to_string(),
        text,
    }]);
}
