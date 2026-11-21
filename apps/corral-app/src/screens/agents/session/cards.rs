//! The cards that wait on the owner (`session::Dock`): what they share (the frame, numbered
//! options and their keys, the foot row) and the docked choice at the top of the dock (the
//! build's exit choice, a child's question, a proposed board plan). The cards drawn in the
//! composer's place are in `decision.rs` (select, confirm, input; the one confirmation of a
//! destructive option) and `questions.rs` (an owner question's answer card).
//!
//! A card's frame is built once per card and its parts follow the model, so a ticking countdown
//! or a pick never takes the keyboard focus off an option. Options are numbered rows; 1–9 choose
//! one and the arrows walk them while the card has the keys and no field does. Desktop and phone:
//! a raised card, its head on surface-2, a card in the composer's place edged in the focus
//! colour; terminal: a framed region a Cell in. test ids `dock-<card>`, `opt-<card>-<n>`,
//! `btn-card-<verb>`.

use std::cell::Cell;
use std::rc::Rc;

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::commands::{MENU_WALK, menu_step};
use crate::model::hue::Hue;
use crate::model::session::{Action, CardsRead, ChoiceCard, ChoiceOption, Focus, SessionPage};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::exit;

/// The phone's least touch target.
pub(super) const TARGET: f32 = 44.0;
const DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Words at a card's size in `ink`, wrapping.
pub(super) fn words(form: Form, content: impl Into<SharedString>, ink: AdaptiveColor) -> Text {
    let words = text(content.into()).text_color(ink);
    match form {
        Form::Terminal => words,
        Form::Desktop => words
            .font_family(FONT_TEXT)
            .text_size(px(12.5))
            .line_height(px(18.0)),
        Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(14.5))
            .line_height(px(20.0)),
    }
}

/// Quiet words at a card's size.
pub(super) fn quiet(form: Form, content: impl Into<SharedString>) -> Text {
    words(form, content, color(Token::Ink3))
}

/// A tone's words: ink-3 when neutral.
pub(super) fn tint(hue: Hue) -> AdaptiveColor {
    if hue == Hue::Neutral {
        color(Token::Ink3)
    } else {
        hue_ink(hue)
    }
}

/// A card's frame: the title bold in its head row with `trailing` at its end, then `rows`.
/// Desktop and phone: a raised surface with 10 pt corners, the head on surface-2 over a hairline,
/// an `accent` card edged in the focus colour. Terminal: a framed region a Cell in.
pub(super) fn frame(
    app: AppState,
    test_id: &str,
    title: impl Fn() -> String + 'static,
    trailing: Option<AnyElement>,
    rows: Vec<AnyElement>,
    accent: bool,
) -> AnyElement {
    let form = app.form;
    let title = text(title)
        .role(AccessibilityRole::Header)
        .flex_1()
        .min_w(px(0.0))
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    if form == Form::Terminal {
        let head = view()
            .flex_row()
            .items_center()
            .gap(px(16.0))
            .child(title)
            .children(trailing);
        return term::framed(view().px(term::cells(1.0)).child(head).children(rows))
            .test_id(test_id)
            .flex_none()
            .into_element();
    }
    let phone = form == Form::Phone;
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .px(px(12.0))
        .py(px(8.0))
        .bg(color(Token::Surface2))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .child(
            title
                .font_family(FONT_TEXT)
                .text_size(px(if phone { 15.0 } else { 13.5 }))
                .line_height(px(if phone { 20.0 } else { 18.0 })),
        )
        .children(trailing);
    view()
        .test_id(test_id)
        .flex_none()
        .rounded(px(10.0))
        .border(px(if accent { 1.5 } else { 1.0 }))
        .border_color(color(if accent { Token::Focus } else { Token::Rule2 }))
        .bg(color(Token::Surface))
        .overflow_hidden()
        .child(head)
        .child(
            view()
                .gap(px(6.0))
                .px(px(if phone { 10.0 } else { 12.0 }))
                .py(px(10.0))
                .children(rows),
        )
        .into_element()
}

/// The legend of a card's keys (`↑↓ select  1-3 pick  ⏎ choose  esc not now`); none on a phone.
pub(super) fn hint(app: AppState, keys: &[(&'static str, &'static str)]) -> Option<AnyElement> {
    let form = app.form;
    if keys.is_empty() || form == Form::Phone {
        return None;
    }
    let caps: Vec<AnyElement> = keys
        .iter()
        .map(|&(keys, name)| {
            if form == Form::Terminal {
                return term::legend(keys, name);
            }
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(crate::ui::atoms::key_hint(keys))
                .child(
                    text(name)
                        .font_family(FONT_TEXT)
                        .text_size(px(11.0))
                        .text_color(color(Token::Ink3)),
                )
                .into_element()
        })
        .collect();
    Some(
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
            .children(caps)
            .into_element(),
    )
}

/// One option: its number, `●`/`○` where a question shows what is chosen, the label bold and its
/// detail muted. An unavailable option keeps its row and number, muted with its reason; one that
/// needs the owner's reason says so. Pressing it chooses it once `armed`.
pub(super) fn option(
    app: AppState,
    option: &ChoiceOption,
    marks: bool,
    armed: Rc<Cell<bool>>,
    handle: ElementRef,
) -> AnyElement {
    let form = app.form;
    let enabled = option.action.is_some();
    let number = text(option.number.to_string())
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let number = match form {
        Form::Terminal => number.w(term::cells(3.0)),
        _ => number
            .w(px(16.0))
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(12.0)),
    };
    let glyph = marks.then(|| {
        words(
            form,
            if option.chosen { "●" } else { "○" },
            color(Token::Ink),
        )
        .flex_shrink_0()
    });
    let label = words(
        form,
        option.label.clone(),
        color(if option.unavailable.is_some() {
            Token::Ink3
        } else {
            Token::Ink
        }),
    )
    .flex_shrink_0()
    .font_weight(FontWeight::SEMIBOLD);
    let detail = option
        .detail
        .clone()
        .map(|detail| quiet(form, detail).flex_1().min_w(px(0.0)));
    let unavailable = option
        .unavailable
        .as_ref()
        .map(|why| format!("unavailable: {why}"));
    let notes: Vec<AnyElement> = [
        unavailable.clone(),
        option
            .needs_text
            .as_ref()
            .map(|why| format!("needs your reason: {why}")),
    ]
    .into_iter()
    .flatten()
    .map(|note| words(form, note, hue_ink(Hue::Held)).into_element())
    .collect();
    let spoken = [
        Some(format!("{}. {}", option.number, option.label)),
        option.detail.clone(),
        unavailable,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(number)
        .children(glyph)
        .child(label)
        .children(detail);
    let body = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(line)
        .children(notes);
    let action = option.action.clone();
    let row = view()
        .element_ref(&handle)
        .test_id(&option.id)
        .role(AccessibilityRole::Button)
        .label(spoken)
        .accessibility_state(AccessibilityState {
            selected: option.chosen,
            disabled: !enabled,
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .focusable(enabled)
        .when(enabled, |row| {
            row.cursor_pointer().on_press(move || {
                if armed.get()
                    && let Some(action) = &action
                {
                    run(app, action.clone());
                }
            })
        });
    let ring = |s: nsh_core::element::StyleRefinement| {
        s.outline(px(2.0))
            .outline_color(color(Token::Focus))
            .outline_offset(px(-2.0))
    };
    match form {
        Form::Terminal => row
            .child(term::focus_cell(term::focus_of(handle)))
            .child(body)
            .into_element(),
        Form::Desktop => row
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .when(option.chosen, |row| row.bg(color(Token::Sel)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring)
            .child(body)
            .into_element(),
        Form::Phone => row
            .min_h(px(TARGET))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .when(option.chosen, |row| row.bg(color(Token::Sel)))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring)
            .child(body)
            .into_element(),
    }
}

/// The keys of a card's options while it is mounted and `gate` holds: the arrows walk them, 1–9
/// choose one once `armed`. `pass` lets the keys on to the page as well, for a docked card that
/// shares the page with the composer.
fn option_keys(
    app: AppState,
    options: Rc<Vec<(Option<Action>, ElementRef)>>,
    armed: Rc<Cell<bool>>,
    gate: Rc<dyn Fn() -> bool>,
    pass: bool,
) {
    let walk = KeyOptions {
        when: When::NotEditing,
        repeat: true,
        pass_through: pass,
        ..KeyOptions::default()
    };
    let at = Rc::new(Cell::new(0usize));
    for (index, (_, handle)) in options.iter().enumerate() {
        let at = at.clone();
        handle.on_focus_in(move || at.set(index));
    }
    for key in MENU_WALK {
        let (options, at, gate) = (options.clone(), at.clone(), gate.clone());
        on_key_with(key, walk, move |_| {
            if !gate() {
                return;
            }
            if let Some(next) = menu_step(key, at.get(), options.len())
                && let Some((_, handle)) = options.get(next)
            {
                at.set(next);
                handle.focus();
            }
        });
    }
    let pick = KeyOptions {
        repeat: false,
        ..walk
    };
    for (index, key) in DIGITS.into_iter().enumerate() {
        let (options, armed, gate) = (options.clone(), armed.clone(), gate.clone());
        on_key_with(key, pick, move |_| {
            if gate()
                && armed.get()
                && let Some((Some(action), _)) = options.get(index)
            {
                run(app, action.clone());
            }
        });
    }
}

/// A card's options, rebuilt as they change, with their keys. `marks` shows `●`/`○`. A card in
/// the composer's place takes the page's keys: `keyed` is none and `focus` gives option `n` the
/// keys when the list is first drawn. A docked card has them only while `keyed` holds (the dock
/// gives them to it); then the option that had the focus, else the first, takes them.
pub(super) fn options_list(
    app: AppState,
    options: Memo<Vec<ChoiceOption>>,
    armed: Rc<Cell<bool>>,
    keyed: Option<Memo<bool>>,
    marks: bool,
    focus: Option<usize>,
) -> AnyElement {
    let had = Rc::new(Cell::new(None::<usize>));
    let first = Rc::new(Cell::new(focus));
    let gate: Rc<dyn Fn() -> bool> =
        Rc::new(move || keyed.is_none_or(|keyed| keyed.get_untracked()));
    dynamic(move || {
        let options = options.get();
        if options.is_empty() {
            return None;
        }
        let (armed, gate, had, first) = (armed.clone(), gate.clone(), had.clone(), first.clone());
        Some(untracked(|| {
            let handles: Vec<ElementRef> = options.iter().map(|_| element_ref()).collect();
            let rows: Vec<AnyElement> = options
                .iter()
                .zip(&handles)
                .map(|(choice, handle)| option(app, choice, marks, armed.clone(), *handle))
                .collect();
            for (index, handle) in handles.iter().enumerate() {
                let had = had.clone();
                handle.on_focus_in(move || had.set(Some(index)));
            }
            let last = handles.len().saturating_sub(1);
            let shown = Rc::new(handles.clone());
            let take = move |at: usize| {
                if let Some(handle) = shown.get(at.min(last)).copied() {
                    after_answer(move || handle.focus());
                }
            };
            match keyed {
                Some(keyed) => {
                    let had = had.clone();
                    effect(move || {
                        if keyed.get() {
                            take(had.get().unwrap_or(0));
                        }
                    });
                }
                None => {
                    if let Some(at) = first.take() {
                        take(at);
                    }
                }
            }
            let actions = options
                .iter()
                .map(|choice| choice.action.clone())
                .zip(handles)
                .collect();
            option_keys(app, Rc::new(actions), armed, gate, keyed.is_some());
            view()
                .role(AccessibilityRole::List)
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 2.0 }))
                .children(rows)
                .into_element()
        }))
    })
}

/// A card's button: `kind`, `test_id`, running `action`; full width on the phone.
pub(super) fn press(
    app: AppState,
    label: impl Into<String>,
    kind: ButtonKind,
    test_id: impl Into<String>,
    action: Action,
    options: ButtonOptions,
) -> AnyElement {
    button_with(
        app,
        label,
        kind,
        ButtonOptions {
            wide: app.form == Form::Phone,
            ..options
        },
        test_id,
        move || run(app, action.clone()),
    )
}

/// A card's last row: its keys, then its buttons at the trailing edge.
pub(super) fn foot_row(
    app: AppState,
    keys: Option<AnyElement>,
    pressed: Vec<AnyElement>,
) -> AnyElement {
    let form = app.form;
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(view().flex_1().min_w(px(0.0)).children(keys))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_end()
                .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
                .when(form == Form::Phone, |row| row.w_full())
                .children(pressed),
        )
        .into_element()
}

/// Why the card cannot be answered from here, `sending…`, or, while its body is not here,
/// where the read stands (`reading…`, the failure) with Retry after a failure (`btn-cards-retry`).
pub(super) fn standing(
    app: AppState,
    read: Option<CardsRead>,
    sending: bool,
    inert: Option<&str>,
) -> Option<AnyElement> {
    let form = app.form;
    let read = match (inert, sending, read) {
        (Some(why), _, _) => return Some(quiet(form, why.to_owned()).into_element()),
        (None, true, _) => return Some(quiet(form, "sending…").into_element()),
        (None, false, read) => read?,
    };
    let retry = read.retry.map(|action| {
        button(
            app,
            "Retry",
            ButtonKind::Quiet,
            "btn-cards-retry",
            move || run(app, action.clone()),
        )
    });
    Some(
        view()
            .test_id("card-read")
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
            .child(
                words(form, read.words, tint(read.hue))
                    .flex_1()
                    .min_w(px(0.0)),
            )
            .children(retry)
            .into_element(),
    )
}

/// The docked choice (`dock.choice`): the title, the exit's landing policy at its end, the exit's
/// summary or the card's body, the host's options in its order, Write my own answer for a
/// child's question, its keys and Not now. Its keys work while the dock has them.
pub(super) fn choice(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let card = computed(move || page.with(|p| p.dock.choice.clone()));
    let id = computed(move || card.with(|c| c.as_ref().map(|c| c.card.clone())));
    dynamic(move || id.get().map(|_| untracked(|| choice_card(app, page, card))))
}

fn choice_card(
    app: AppState,
    page: Memo<SessionPage>,
    card: Memo<Option<ChoiceCard>>,
) -> AnyElement {
    let form = app.form;
    let test_id = untracked(|| card.with(|c| c.as_ref().map(|c| c.id.clone()).unwrap_or_default()));
    let policy = computed(move || card.with(|c| c.as_ref().and_then(|c| c.policy.clone())));
    let policy = dynamic(move || {
        policy
            .get()
            .map(|policy| untracked(|| quiet(form, policy).number_of_lines(2u32).into_element()))
    });
    let about = computed(move || {
        card.with(|c| {
            c.as_ref()
                .map(|c| (c.summary.clone(), c.body.clone(), c.more))
        })
    });
    let about = dynamic(move || {
        let (summary, body, more) = about.get()?;
        untracked(|| {
            let mut rows = summary
                .map(|summary| exit::summary(app, &summary))
                .unwrap_or_default();
            rows.extend(body.map(|body| words(form, body, color(Token::Ink2)).into_element()));
            if more {
                rows.push(quiet(form, "… more in the transcript").into_element());
            }
            (!rows.is_empty()).then(|| view().gap(px(4.0)).children(rows).into_element())
        })
    });
    let options =
        computed(move || card.with(|c| c.as_ref().map(|c| c.options.clone()).unwrap_or_default()));
    let keyed = computed(move || page.with(|p| p.focus == Focus::Dock));
    let rows = options_list(
        app,
        options,
        Rc::new(Cell::new(true)),
        Some(keyed),
        false,
        None,
    );
    let state = computed(move || {
        card.with(|c| {
            c.as_ref()
                .map(|c| (c.read.clone(), c.sending, c.inert.clone()))
        })
    });
    let state = dynamic(move || {
        let (read, sending, inert) = state.get()?;
        untracked(|| standing(app, read, sending, inert.as_deref()))
    });
    let foot = computed(move || {
        card.with(|c| {
            c.as_ref()
                .map(|c| (c.own.clone(), c.not_now.clone(), c.hint.clone()))
        })
    });
    let foot = dynamic(move || {
        let (own, not_now, keys) = foot.get()?;
        Some(untracked(|| {
            let own = own.map(|(label, action)| {
                press(
                    app,
                    label,
                    ButtonKind::Ghost,
                    "btn-card-own",
                    action,
                    ButtonOptions::default(),
                )
            });
            let not_now = press(
                app,
                "Not now",
                ButtonKind::Quiet,
                "btn-card-not-now",
                not_now,
                ButtonOptions {
                    key_hint: Some("Esc"),
                    ..ButtonOptions::default()
                },
            );
            foot_row(
                app,
                hint(app, &keys),
                own.into_iter().chain([not_now]).collect(),
            )
        }))
    });
    frame(
        app,
        &test_id,
        move || card.with(|c| c.as_ref().map(|c| c.title.clone()).unwrap_or_default()),
        Some(policy),
        vec![about, rows, state, foot],
        false,
    )
}

/// The composer's place: inset as the composer is, so a card there lines up with it.
pub(super) fn slot(app: AppState, card: AnyElement) -> AnyElement {
    let slot = view().flex_none().child(card);
    match app.form {
        Form::Terminal => slot.px(term::cells(1.0)).into_element(),
        Form::Desktop => slot.px(px(14.0)).py(px(8.0)).into_element(),
        Form::Phone => slot.px(px(12.0)).py(px(6.0)).into_element(),
    }
}
