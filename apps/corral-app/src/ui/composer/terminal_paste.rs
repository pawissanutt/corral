//! A consumed, focus-scoped shortcut for a local display. Terminal-emulator bracketed paste
//! goes directly to NSH's input and never comes through this path.

use nsh_core::keymap::When;
use nsh_core::prelude::*;
use nsh_core::text_edit::{pasted, utf16_to_byte};

use crate::model::session::{Action, SessionComposer};
use crate::model::{Form, Id};
use crate::state::AppState;

use super::clipboard::{self, ReadError};
use super::{Attach, session::act, stage_paste};

pub(super) fn bind(
    app: AppState,
    id: &Id,
    attach: Attach,
    entry: ElementRef,
    composer: Memo<Option<SessionComposer>>,
) -> Option<Signal<Selection>> {
    if app.form != Form::Terminal {
        return None;
    }
    let helpers = clipboard::helpers();
    if helpers.is_empty() {
        return None;
    }
    let focused = signal(false);
    let selection = signal(Selection::default());
    let session = id.clone();
    entry.on_focus_in(move || focused.set(true));
    entry.on_focus_out(move || focused.set(false));
    let draft = move || composer.with_untracked(|c| c.as_ref().map(|c| c.draft.clone()));
    effect(move || {
        if !focused.get() {
            return;
        }
        let helpers = helpers.clone();
        let session = session.clone();
        let busy = signal(false);
        // NSH's TUI keymap runs before native editing. Consume the key instead of reading the
        // clipboard again after the native editor has already pasted it.
        on_key_with(
            "ctrl+v",
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| {
                if busy.get_untracked() {
                    return;
                }
                let Some(before) = draft() else {
                    return;
                };
                let range = selection.get_untracked();
                let source = app.source_revision.get_untracked();
                busy.set(true);
                let session = session.clone();
                let helpers = helpers.clone();
                let read = blocking(move || clipboard::read(helpers));
                // The effect owns the task: blur/unmount disposes it. Do not apply a delayed
                // paste at a different selection, over a newer draft or in another source.
                let _ = spawn(async move {
                    let result = read.await;
                    busy.set(false);
                    if !focused.get_untracked()
                        || app.source_revision.get_untracked() != source
                        || draft().as_ref() != Some(&before)
                        || selection.get_untracked() != range
                    {
                        return;
                    }
                    match result {
                        Ok(Ok(Some(paste))) if !paste.text.is_empty() => {
                            let (text, caret) = splice(before, range, &paste.text);
                            // The 0.6.1 TUI host ignores InsertText but mounts controlled input
                            // values followed by SetSelection and reports the resulting caret.
                            act(app, Action::Draft { session, text });
                            selection.set(Selection {
                                start: caret,
                                end: caret,
                            });
                            entry.set_selection(caret, caret);
                        }
                        Ok(Ok(Some(paste))) => stage_paste(app, &session, attach, &paste),
                        Ok(Err(ReadError::TooLarge)) => app.toast("Clipboard paste is too large."),
                        Ok(Err(ReadError::Timeout)) => app.toast("Clipboard read timed out."),
                        Ok(Ok(None)) | Err(_) => {}
                    }
                });
            },
        );
    });
    Some(selection)
}

fn splice(mut draft: String, range: Selection, text: &str) -> (String, u32) {
    let start = range.start.min(range.end);
    let end = range.end.max(range.start);
    let at = utf16_to_byte(&draft, start);
    let until = utf16_to_byte(&draft, end);
    let text = pasted(text, true);
    draft.replace_range(at..until, &text);
    let caret = nsh_core::text_edit::byte_to_utf16(&draft, at + text.len());
    (draft, caret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_terminal_paste_replaces_the_utf16_selection_once_and_leaves_the_caret_after_it() {
        let range = Selection { start: 3, end: 4 };
        let (draft, caret) = splice("a🌱btail".to_owned(), range, "🌻\r\n");
        assert_eq!(draft, "a🌱🌻\ntail");
        assert_eq!(caret, 6);
    }
}
