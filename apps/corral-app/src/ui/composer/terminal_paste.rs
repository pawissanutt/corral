//! A consumed, focus-scoped shortcut for a local display. Terminal-emulator bracketed paste
//! goes directly to NSH's input and never comes through this path.

use std::rc::Rc;
use std::time::{Duration, Instant};

use nsh_core::element::{Input, input};
use nsh_core::keymap::When;
use nsh_core::prelude::*;
use nsh_core::text_edit::{pasted, utf16_to_byte};

use crate::model::session::{Action, SessionComposer};
use crate::model::{Form, Id};
use crate::state::AppState;

use super::clipboard::{self, ReadError};
use super::{Attach, session::act, stage_paste};

// The desktop can still hold old text or old files after tmux swallows OSC 52. There is no
// common change counter, so reserve one nearby paste for the copy we actually saw, then return
// to the desktop. Never ask NSH's private cache via clipboard_string: on 0.6.1 that can run
// unbounded synchronous helper reads when no copy has initialized it.
const COPY_WINDOW: Duration = Duration::from_secs(15);
type CopiedText = Result<Rc<str>, ReadError>;

#[derive(Clone, PartialEq, Eq)]
struct PendingCopy {
    text: CopiedText,
    source: u64,
    at: Instant,
}

fn take_copy(copy: &mut Option<PendingCopy>, source: u64, now: Instant) -> Option<CopiedText> {
    let copy = copy.take()?;
    (copy.source == source && now.duration_since(copy.at) < COPY_WINDOW).then_some(copy.text)
}

#[derive(Clone, Copy)]
struct CopyCache(Signal<Option<PendingCopy>>);

impl CopyCache {
    fn remember(self, text: CopiedText, source: u64) {
        self.0.set(Some(PendingCopy {
            text,
            source,
            at: Instant::now(),
        }));
    }

    fn clear(self) {
        self.0.set(None);
    }

    fn take(self, source: u64) -> Option<CopiedText> {
        let mut copy = self.0.get_untracked();
        if copy.is_some() {
            self.clear();
        }
        take_copy(&mut copy, source, Instant::now())
    }

    fn has_copy(self, source: u64) -> bool {
        self.0
            .with_untracked(|copy| copy.as_ref().is_some_and(|copy| copy.source == source))
    }
}

const COPY: &[&str] = &["ctrl+c", "ctrl+shift+c", "cmd+c", "ctrl+insert", "alt+w"];
const CUT: &[&str] = &["ctrl+shift+x", "cmd+x", "shift+delete"];

fn copy_keys(keys: &[&str], when: When, handler: impl Fn() + Clone + 'static) {
    for key in keys {
        let handler = handler.clone();
        on_key_with(
            key,
            KeyOptions {
                when,
                pass_through: true,
                ..Default::default()
            },
            move |_| handler(),
        );
    }
}

pub(super) fn install(app: AppState) {
    if app.form != Form::Terminal || clipboard::helpers().is_empty() {
        return;
    }
    let cache = CopyCache(signal(None));
    provide_context(cache);
    // An unobserved copy in another native input must not leave the earlier mirrored copy
    // preferred. A selection-specific observer mounted later takes precedence over this one.
    copy_keys(COPY, When::Always, move || cache.clear());
    copy_keys(CUT, When::Always, move || cache.clear());
}

/// A native, non-secret input that contributes to the same bounded copy cache as the composer.
pub(super) struct InputCopy {
    app: AppState,
    cache: CopyCache,
    entry: ElementRef,
}

impl InputCopy {
    pub(super) fn new(app: AppState, entry: Option<ElementRef>) -> Option<Self> {
        if app.form != Form::Terminal {
            return None;
        }
        Some(Self {
            app,
            cache: use_context::<CopyCache>()?,
            entry: entry.unwrap_or_else(element_ref),
        })
    }

    pub(super) fn controlled(self, value: impl Fn() -> String + 'static) -> Input {
        let value = Rc::new(value);
        let current = value.clone();
        self.observe(move |range| selected_text(&current(), range))
            .value(move || value())
    }

    pub(super) fn uncontrolled(self) -> Input {
        // NSH reports only the range at selection time. Keep a bounded snapshot for the
        // confirmation word, without turning it into a controlled field or capping its edits.
        let draft = signal(None::<CopiedText>);
        self.observe(move |range| {
            draft.with_untracked(|draft| match draft.as_ref()? {
                Ok(text) => selected_text(text, range),
                Err(error) => Some(Err(*error)),
            })
        })
        .on_change(move |text: &str| draft.set(bounded_text(text)))
    }

    fn observe(self, selected: impl Fn(Selection) -> Option<CopiedText> + 'static) -> Input {
        let Self { app, cache, entry } = self;
        let focused = signal(false);
        let range = signal(None::<(Selection, u64)>);
        entry.on_focus_in(move || focused.set(true));
        entry.on_focus_out(move || focused.set(false));
        let selected = Rc::new(selected);
        effect(move || {
            if !focused.get() {
                return;
            }
            let selected = selected.clone();
            let remember = move || {
                if !focused.get_untracked() {
                    return;
                }
                let source = app.source_revision.get_untracked();
                match range.get_untracked() {
                    Some((range, revision)) if source == revision && range.start != range.end => {
                        // The host reports post-cut edits after this key, so the selected span
                        // still refers to the text before native editing.
                        if let Some(text) = selected(range) {
                            cache.remember(text, source);
                        } else {
                            cache.clear();
                        }
                    }
                    _ => cache.clear(),
                }
            };
            copy_keys(COPY, When::Always, remember.clone());
            copy_keys(CUT, When::Always, remember);
        });
        input()
            .element_ref(&entry)
            .on_selection_change(move |selection: &Selection| {
                range.set(Some((*selection, app.source_revision.get_untracked())));
            })
    }
}

pub(super) fn observe_selection(app: AppState, view: View) -> View {
    let Some(cache) = use_context::<CopyCache>() else {
        return view;
    };
    let selected = signal(None::<(u64, CopiedText)>);
    let active = computed(move || selected.with(Option::is_some));
    effect(move || {
        if active.get() {
            // A native field takes copy before read-only selection. Run only while not
            // editing and let NSH perform the real copy (including its OSC 52 output).
            copy_keys(COPY, When::NotEditing, move || {
                if let Some((source, text)) = selected.get_untracked()
                    && source == app.source_revision.get_untracked()
                {
                    cache.remember(text, source);
                }
            });
        }
    });
    view.on_text_selection_change(move |event: &TextSelectionEvent| {
        selected
            .set(bounded_text(&event.text).map(|text| (app.source_revision.get_untracked(), text)));
    })
}

fn bounded_text(text: &str) -> Option<CopiedText> {
    if text.is_empty() {
        None
    } else if text.len() > clipboard::TEXT_LIMIT {
        Some(Err(ReadError::TooLarge))
    } else {
        Some(Ok(Rc::from(text)))
    }
}

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
    let cache = use_context::<CopyCache>()?;
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
        // For a cut the host reports the changed draft after dispatching this key, so the
        // selected span here is still its pre-cut text.
        let remember = move || {
            if focused.get_untracked()
                && let Some(text) = composer.with_untracked(|c| {
                    let draft = &c.as_ref()?.draft;
                    selected_text(draft, selection.get_untracked())
                })
            {
                cache.remember(text, app.source_revision.get_untracked());
            }
        };
        copy_keys(COPY, When::Always, remember);
        copy_keys(CUT, When::Always, remember);
        // NSH's TUI keymap runs before native editing. Consume the key instead of reading the
        // clipboard again after the native editor has already pasted it.
        on_key_with(
            "ctrl+v",
            KeyOptions {
                when: When::Always,
                ..Default::default()
            },
            move |_| {
                if !focused.get_untracked() || busy.get_untracked() {
                    return;
                }
                let Some(before) = draft() else {
                    return;
                };
                let range = selection.get_untracked();
                let source = app.source_revision.get_untracked();
                if let Some(copy) = cache.take(source) {
                    match copy {
                        Ok(text) => insert(app, &session, entry, selection, before, range, &text),
                        Err(_) => app.toast("Clipboard paste is too large."),
                    }
                    return;
                }
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
                        || cache.has_copy(source)
                        || draft().as_ref() != Some(&before)
                        || selection.get_untracked() != range
                    {
                        return;
                    }
                    match result {
                        Ok(Ok(Some(paste))) if !paste.text.is_empty() => {
                            insert(app, &session, entry, selection, before, range, &paste.text);
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

fn insert(
    app: AppState,
    session: &Id,
    entry: ElementRef,
    selection: Signal<Selection>,
    before: String,
    range: Selection,
    paste: &str,
) {
    let (text, caret) = splice(before, range, paste);
    // The 0.6.1 TUI host ignores InsertText but mounts controlled input values followed
    // by SetSelection and reports the resulting caret.
    act(
        app,
        Action::Draft {
            session: session.clone(),
            text,
        },
    );
    selection.set(Selection {
        start: caret,
        end: caret,
    });
    entry.set_selection(caret, caret);
}

fn selected_text(draft: &str, range: Selection) -> Option<CopiedText> {
    bounded_text(&draft[selection_bytes(draft, range)])
}

fn selection_bytes(draft: &str, range: Selection) -> std::ops::Range<usize> {
    let start = range.start.min(range.end);
    let end = range.start.max(range.end);
    utf16_to_byte(draft, start)..utf16_to_byte(draft, end)
}

fn splice(mut draft: String, range: Selection, text: &str) -> (String, u32) {
    let bytes = selection_bytes(&draft, range);
    let at = bytes.start;
    let text = pasted(text, true);
    draft.replace_range(bytes, &text);
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

    #[test]
    fn copy_uses_the_selected_span_only_and_refuses_oversize() {
        let draft = "a🌱b";
        let selected = selection_bytes(draft, Selection { start: 3, end: 1 });
        assert_eq!(
            bounded_text(&draft[selected]).unwrap().unwrap().as_ref(),
            "🌱"
        );
        assert!(bounded_text("").is_none());
        assert_eq!(
            bounded_text(&"x".repeat(clipboard::TEXT_LIMIT + 1)),
            Some(Err(ReadError::TooLarge))
        );
    }

    #[test]
    fn known_copy_takes_only_one_paste_in_its_own_source_and_window() {
        let at = Instant::now();
        let copy = || {
            Some(PendingCopy {
                text: Ok(Rc::from("native copy")),
                source: 1,
                at,
            })
        };
        let mut pending = copy();
        assert_eq!(
            take_copy(&mut pending, 1, at).unwrap().unwrap().as_ref(),
            "native copy"
        );
        assert!(take_copy(&mut pending, 1, at).is_none());
        assert!(take_copy(&mut copy(), 2, at).is_none());
        assert!(take_copy(&mut copy(), 1, at + COPY_WINDOW).is_none());
    }
}
