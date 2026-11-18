//! The phone's composer: one row of "+" (the attachments menu), the field growing with the
//! draft, and Send; above it who answers as one chip ("pi-luna ▾") that opens the model list
//! in the flow, and the new model's note for a moment after a change. Held sideways the chip
//! moves into the row, so a landscape sheet spends no line of its
//! little height on it and opens with the choice folded away.

use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{Action, Composer, ModelList};
use crate::model::text::Rich;
use crate::state::AppState;
use crate::ui::ask::models::{DISCLOSURE, Picked, inline_list};
use crate::ui::ask::{act, chip_row, field_row, plus, tray};
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::page_phone::short_window;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::{To, entry_field, saved_line, send, small};

/// The touch target's floor, and the row's height with the field one line high.
pub(super) const TARGET: f32 = 44.0;
/// The field's tallest: five lines of 22 pt, its padding and its ring; then it scrolls.
pub(super) const FIELD_MAX: f32 = 5.0 * 22.0 + 2.0 * 9.0 + 3.0;
/// How long the new model's note stays after a change.
const NOTE_TIME: Duration = Duration::from_secs(4);
/// The who chip's own height, centred in its touch target.
const CHIP_HEIGHT: f32 = 30.0;

/// The phone composer: the status line, the transient model note, the who chip and its chooser,
/// the context chip and the staged tray, the open link or path field, then the row.
pub(super) fn composer(
    app: AppState,
    built: Memo<Composer>,
    entry: ElementRef,
    to: To,
    status: AnyElement,
    next: Memo<Option<Rich>>,
    menu: Memo<Option<ModelList>>,
) -> AnyElement {
    let docked = to.docked;
    let choosing = signal(false);
    let open = computed(move || choosing.get() || menu.with(Option::is_some));
    let sideways = computed(short_window);
    let shell = view().gap(px(6.0)).px(px(12.0));
    let shell = if docked {
        shell
            .pt(px(6.0))
            .pb(px(8.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
    } else {
        shell.pt(px(2.0)).pb(px(12.0))
    };
    shell
        .child(status)
        .child(note(app, next))
        .child(dynamic(move || {
            (!sideways.get()).then(|| untracked(|| who(app, built, open, choosing, true)))
        }))
        .child(chooser(app, menu, open, choosing))
        .child(chip_row(app, built))
        .child(tray(app, built))
        .child(field_row(app))
        .child(row(app, built, entry, to, (open, choosing, sideways)))
        .into_element()
}

/// The new model's line ("pi-luna Your next message is answered by…"), shown for a moment each
/// time it changes, never for the profile a thread already had when the composer opened.
fn note(app: AppState, next: Memo<Option<Rich>>) -> AnyElement {
    let shown = signal(None::<Rich>);
    let last = stored(next.with_untracked(|line| line.clone()));
    let pending = stored(None::<Spawned<()>>);
    effect(move || {
        let now = next.get();
        if last.replace(now.clone()) == now {
            return;
        }
        if let Some(old) = pending.replace(None) {
            old.cancel();
        }
        if now.is_some() {
            let _ = pending.replace(Some(set_timeout(NOTE_TIME, move || shown.set(None))));
        }
        shown.set(now);
    });
    dynamic(move || shown.get().map(|line| untracked(|| saved_line(app, &line))))
}

/// Who answers, as the model resolves it: the model of the next message, the host's own
/// profile note; whether there is anything to choose.
#[derive(Debug, Clone, PartialEq)]
struct Who {
    model: Option<String>,
    note: Option<String>,
    choosable: bool,
}

impl Who {
    fn of(composer: &Composer) -> Who {
        Who {
            model: composer.model_name.clone(),
            note: composer.profile_note.clone(),
            choosable: composer.model_pill.is_some(),
        }
    }
}

/// The who chip, with the profile note beside it when `with_note`; a session that has nothing to
/// choose shows only its note.
fn who(
    app: AppState,
    built: Memo<Composer>,
    open: Memo<bool>,
    choosing: Signal<bool>,
    with_note: bool,
) -> AnyElement {
    let who = computed(move || built.with(Who::of));
    dynamic(move || {
        let who = who.get();
        untracked(|| {
            let note = who.note.clone().filter(|_| with_note).map(|note| {
                small(Form::Phone, note)
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(color(Token::Ink3))
                    .into_element()
            });
            let chip = who
                .choosable
                .then(|| who_chip(app, built, &who, open, choosing));
            if chip.is_none() && note.is_none() {
                return None;
            }
            Some(
                view()
                    .flex_row()
                    .items_center()
                    .min_w(px(0.0))
                    .gap(px(8.0))
                    .children(chip)
                    .children(note)
                    .into_element(),
            )
        })
    })
}

/// "pi-luna ▾": a 30 pt pill in a full touch target; pressing it
/// opens or closes the chooser.
fn who_chip(
    app: AppState,
    built: Memo<Composer>,
    who: &Who,
    open: Memo<bool>,
    choosing: Signal<bool>,
) -> AnyElement {
    let spoken = who.model.clone().unwrap_or_default();
    let words = |words: String, ink: Token| {
        text(words)
            .number_of_lines(1u32)
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .text_color(color(ink))
    };
    let face = view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .min_w(px(0.0))
        .h(px(CHIP_HEIGHT))
        .px(px(11.0))
        .rounded(px(CHIP_HEIGHT / 2.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(move || {
            if open.get() {
                color(Token::Surface2)
            } else {
                color(Token::Surface)
            }
        })
        .children(who.model.clone().map(|model| {
            words(model, Token::Ink2)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate()
        }))
        .child(words(DISCLOSURE.trim().to_owned(), Token::Ink3).text_size(px(11.0)));
    view()
        .test_id("btn-who")
        .role(AccessibilityRole::Button)
        .label(spoken)
        .accessibility_state(move || AccessibilityState {
            expanded: Some(open.get()),
            ..AccessibilityState::default()
        })
        .on_press(move || toggle(app, built, open, choosing))
        .flex_row()
        .items_center()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .h(px(TARGET))
        .cursor_pointer()
        .active(|s| s.opacity(0.8))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-4.0))
        })
        .child(face)
        .into_element()
}

/// Opens the chooser with the model list open, or closes both.
fn toggle(app: AppState, built: Memo<Composer>, open: Memo<bool>, choosing: Signal<bool>) {
    if open.with_untracked(|open| *open) {
        choosing.set(false);
        app.assistant.update(|state| {
            state.close_menu();
        });
        return;
    }
    choosing.set(true);
    if built.with_untracked(|c| c.model_pill.is_some() && c.model_menu.is_none()) {
        act(app, Action::ToggleModelMenu);
    }
}

/// The chooser under the who chip while it is open: the model list. Picking a model closes it.
fn chooser(
    app: AppState,
    menu: Memo<Option<ModelList>>,
    open: Memo<bool>,
    choosing: Signal<bool>,
) -> AnyElement {
    let picked: Picked = Rc::new(move || choosing.set(false));
    when(
        move || open.get(),
        move || {
            let picked = picked.clone();
            untracked(|| {
                view()
                    .gap(px(6.0))
                    .child(inline_list(app, menu, picked))
                    .into_element()
            })
        },
    )
    .into_element()
}

/// The row: "+", the who chip while held sideways, the field taking the rest, and Send as an
/// arrow, all on the field's last line as it grows.
fn row(
    app: AppState,
    built: Memo<Composer>,
    entry: ElementRef,
    to: To,
    (open, choosing, sideways): (Memo<bool>, Signal<bool>, Memo<bool>),
) -> AnyElement {
    let send_state = computed(move || built.with(|c| (c.send.label, c.send.enabled)));
    let send_to = to.clone();
    view()
        .flex_row()
        .items_end()
        .gap(px(6.0))
        .child(plus(app, built))
        .child(dynamic(move || {
            sideways
                .get()
                .then(|| untracked(|| who(app, built, open, choosing, false)))
        }))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(entry_field(app, built, entry, to)),
        )
        .child(dynamic(move || {
            let (label, enabled) = send_state.get();
            let to = send_to.clone();
            untracked(|| {
                button_with(
                    app,
                    label,
                    ButtonKind::Primary,
                    ButtonOptions {
                        icon: Some(Icon::Send),
                        icon_only: true,
                        disabled: !enabled,
                        ..ButtonOptions::default()
                    },
                    "btn-send",
                    move || send(app, &to),
                )
            })
        }))
        .into_element()
}
