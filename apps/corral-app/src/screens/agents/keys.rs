//! The keys bar and type field under a pane session's terminal (`PaneInput`): the keys an
//! agent's screen asks for, one press each, then words typed into the pane as they are, alone
//! (Type) or followed by Enter (Type + Enter, and Return in the field). Each press goes out as
//! a pane request under its own id, and the latest one's outcome shows here in its tone, so a
//! failure is never lost. A headless session's log takes no keys; the region says so instead.
//! Desktop: under the dark surface, one row of keys, the field row, the outcome or the hint.
//! Phone: the same in the docked bar, the keys scrolling sideways. Terminal: a rule, then rows.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Action, PaneInput, Session};
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

use super::run;

/// The region for the session's pane, redrawn only when its shape changes (another session,
/// or whether it takes keys), not with each outcome, so the field keeps its focus and draft.
pub(super) fn region(app: AppState, session: Memo<Session>) -> AnyElement {
    let shown = computed(move || session.with(|s| s.pane.clone()));
    let shape = computed(move || {
        shown.with(|pane| {
            pane.as_ref()
                .map(|pane| (pane.session.clone(), pane.closed.clone()))
        })
    });
    let delivered =
        computed(move || shown.with(|pane| pane.as_ref().and_then(|pane| pane.delivered.clone())));
    let field = Field {
        draft: signal(String::new()),
        handle: element_ref(),
    };
    // Delivered words leave the field once, and the focus goes back to it, so the next words
    // can be typed at once even when a send button was pressed; refused or failed words stay
    // to be sent again.
    let seen = stored(None::<Option<String>>);
    effect(move || {
        let now = delivered.get();
        if let Some(before) = seen.replace(Some(now.clone()))
            && before != now
            && now.is_some()
        {
            field.draft.set(String::new());
            after_answer(move || field.handle.focus());
        }
    });
    let status =
        computed(move || shown.with(|pane| pane.as_ref().and_then(|pane| pane.status.clone())));
    dynamic(move || {
        shape.with(|shape| {
            shape.as_ref().map(|_| {
                untracked(|| match shown.get_untracked() {
                    Some(pane) => body(app, &pane, field, status),
                    None => view().into_element(),
                })
            })
        })
    })
}

/// The type field's draft and the field itself, kept across the region's redraws.
#[derive(Clone, Copy)]
struct Field {
    draft: Signal<String>,
    handle: ElementRef,
}

fn body(
    app: AppState,
    pane: &PaneInput,
    field: Field,
    status: Memo<Option<(Hue, String)>>,
) -> AnyElement {
    let form = app.form;
    let outcome = outcome(app, status);
    let content: Vec<AnyElement> = match &pane.closed {
        Some(why) => vec![closed(app, why.clone()), outcome],
        None => vec![
            keys(app, pane),
            field_row(app, pane, field),
            outcome,
            hint(app, pane.hint.clone(), status),
        ],
    };
    let region = view()
        .test_id("session-keys")
        .role(AccessibilityRole::Toolbar)
        .label("Pane keys")
        .flex_none();
    match form {
        Form::Terminal => region
            .child(term::hrule())
            .child(view().px(term::cells(1.0)).children(content))
            .into_element(),
        Form::Phone => region
            .flex_1()
            .min_w(px(0.0))
            .gap(px(8.0))
            .children(content)
            .into_element(),
        Form::Desktop => region
            .gap(px(8.0))
            .px(px(18.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .bg(color(Token::Bg))
            .border_t_1()
            .border_t_color(color(Token::Rule))
            .children(content)
            .into_element(),
    }
}

/// The keys bar: "Keys", then one ghost button per key (`btn-key-<name>`).
fn keys(app: AppState, pane: &PaneInput) -> AnyElement {
    let form = app.form;
    let caps = pane.keys.iter().map(|key| {
        let action = key.action.clone();
        button_with(
            app,
            key.cap,
            ButtonKind::Ghost,
            ButtonOptions::default(),
            key.test_id.clone(),
            move || run(app, action.clone()),
        )
    });
    let label = text("Keys")
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }));
    match form {
        Form::Terminal => row.child(label).children(caps).into_element(),
        Form::Desktop => row
            .flex_wrap()
            .child(label.font_family(FONT_TEXT).text_size(px(12.0)).mr(px(2.0)))
            .children(caps)
            .into_element(),
        Form::Phone => scroll()
            .horizontal()
            .flex_shrink_0()
            .child(row.flex_shrink_0().children(caps))
            .into_element(),
    }
}

/// The field and its two sends: Type, and Type + Enter (also Return in the field).
fn field_row(app: AppState, pane: &PaneInput, field: Field) -> AnyElement {
    let Field { draft, handle } = field;
    let form = app.form;
    let session = pane.session.clone();
    let send = move |enter: bool| {
        let text = draft.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        run(
            app,
            Action::Pane {
                session: session.clone(),
                text,
                keys: if enter {
                    vec!["enter".to_owned()]
                } else {
                    Vec::new()
                },
            },
        );
    };
    let send_enter = send.clone();
    let send_plain = send.clone();
    let empty = computed(move || draft.with(|text| text.trim().is_empty()));
    let size = match form {
        Form::Phone => 15.0,
        Form::Desktop | Form::Terminal => text_size(form),
    };
    let entry = input()
        .element_ref(&handle)
        .test_id("field-pane-text")
        .label(pane.accessibility_label)
        .placeholder(pane.placeholder.clone())
        .value(move || draft.get())
        .on_change(move |text: &str| draft.set(text.to_owned()))
        .on_submit(move |_: &str| send(true))
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT)
        .border_1()
        .border_color(color(Token::Rule2))
        .focus(|s| s.border_color(color(Token::Focus)));
    let entry = match form {
        Form::Terminal => entry.h(term::rows(1.0)).px(term::cells(1.0)),
        Form::Desktop => entry
            .font_family(FONT_TEXT)
            .text_size(px(size))
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(6.0)),
        Form::Phone => entry
            .font_family(FONT_TEXT)
            .text_size(px(size))
            .min_h(px(44.0))
            .px(px(12.0))
            .rounded(px(10.0)),
    };
    let (plain, enter) = (pane.type_label, pane.enter_label);
    let sends = dynamic(move || {
        let disabled = empty.get();
        let send_plain = send_plain.clone();
        let send_enter = send_enter.clone();
        view()
            .flex_row()
            .items_center()
            .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
            .child(button_with(
                app,
                plain,
                ButtonKind::Ghost,
                ButtonOptions {
                    disabled,
                    ..ButtonOptions::default()
                },
                "btn-pane-type",
                move || send_plain(false),
            ))
            .child(button_with(
                app,
                enter,
                ButtonKind::Primary,
                ButtonOptions {
                    disabled,
                    ..ButtonOptions::default()
                },
                "btn-pane-type-enter",
                move || send_enter(true),
            ))
            .into_element()
    });
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(entry)
        .child(sends)
        .into_element()
}

/// How the latest pane request ended, in its tone (`pane-status`); nothing before the first.
fn outcome(app: AppState, status: Memo<Option<(Hue, String)>>) -> AnyElement {
    let form = app.form;
    dynamic(move || {
        status.with(|status| {
            status.as_ref().map(|(hue, words)| {
                let ink = match hue {
                    Hue::Neutral => color(Token::Ink3),
                    hue => hue_ink(*hue),
                };
                let line = text(words.clone())
                    .test_id("pane-status")
                    .number_of_lines(2u32)
                    .text_color(ink);
                match form {
                    Form::Terminal => line.into_element(),
                    _ => line
                        .font_family(FONT_TEXT)
                        .text_size(px(12.0))
                        .into_element(),
                }
            })
        })
    })
}

/// How words reach the pane, quiet, until a request has an outcome to show in its place.
fn hint(app: AppState, words: String, status: Memo<Option<(Hue, String)>>) -> AnyElement {
    let form = app.form;
    let quiet = computed(move || status.with(Option::is_none));
    dynamic(move || {
        quiet.get().then(|| {
            let line = text(words.clone())
                .number_of_lines(1u32)
                .truncate()
                .text_color(color(Token::Ink3));
            match form {
                Form::Terminal => line.into_element(),
                _ => line
                    .font_family(FONT_TEXT)
                    .text_size(px(11.5))
                    .into_element(),
            }
        })
    })
}

/// Why a headless session takes no keys, in the sunk box the reply's closed state uses.
fn closed(app: AppState, why: String) -> AnyElement {
    if app.form == Form::Terminal {
        return term::quiet(why).into_element();
    }
    view()
        .test_id("session-keys-closed")
        .px(px(12.0))
        .py(px(9.0))
        .rounded(px(if app.form == Form::Phone { 12.0 } else { 8.0 }))
        .bg(color(Token::BgSunk))
        .border_1()
        .border_color(color(Token::Rule))
        .child(
            text(why)
                .font_family(FONT_TEXT)
                .text_size(px(if app.form == Form::Phone { 14.0 } else { 12.5 }))
                .text_color(color(Token::Ink2)),
        )
        .into_element()
}
