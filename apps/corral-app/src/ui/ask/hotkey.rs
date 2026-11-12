//! The small Ask window of the global shortcut (`.hkwin`): one field; the result is an answer, a
//! filed card, or both; the model list opens below the field. On desktop it floats over a dimmed
//! backdrop standing in for the desktop the window opens on; on a phone and in a terminal it is
//! the form's modal, so the demo still closes cleanly there.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::assistant::{
    CompactFiled, HotkeyResult, HotkeyWindow, KeyHint, ModelList, TargetOption, hotkey_window,
};
use crate::model::{Form, Tone};
use crate::state::AppState;
use crate::ui::ask::act;
use crate::ui::ask::models::pill;
use crate::ui::atoms::key_hint;
use crate::ui::composer::{ListAt, state_mark};
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::chip;
use crate::ui::kit::overlay::{Modal, modal};
use crate::ui::kit::text::{dim, rich};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size, tone};

/// The window's width on the illustrated desktop (`.win10 .hkwin`).
const WIDTH: f32 = 720.0;
/// Room above the window, where the desktop shows.
const TOP: f32 = 96.0;

pub(super) fn hotkey(app: AppState) -> AnyElement {
    // The window's own field. `None` shows the fixture's words for the current state; typing
    // makes it the owner's, kept while the window hides and opens again.
    let draft = signal(None::<String>);
    if app.form != Form::Desktop {
        return modal(
            app,
            move || app.hotkey.get(),
            move || hide(app),
            move || {
                let window = untracked(|| {
                    let env = app.env();
                    app.assistant.with(|state| hotkey_window(state, env))
                });
                Modal {
                    test_id: "hotkey".into(),
                    title: window.title.into(),
                    sub: Some(window.lifetime.into()),
                    body: content(app, draft, tracked_window(app)),
                    done: Some(window.hide.into()),
                    confirm: None,
                }
            },
        );
    }
    when(
        move || app.hotkey.get(),
        move || untracked(|| desktop(app, draft)),
    )
    .into_element()
}

/// The window's content, tracked: the state, the model, the connection.
fn tracked_window(app: AppState) -> Memo<HotkeyWindow> {
    computed(move || {
        let env = app.env();
        app.assistant.with(|state| hotkey_window(state, env))
    })
}

/// Hides the window, closing its model list with it.
fn hide(app: AppState) {
    app.assistant.update(|state| {
        state.close_menu();
    });
    app.hotkey.set(false);
}

/// The desktop: the dimmed backdrop standing in for the desktop (none of it is drawn, so its
/// illustration caption is left out), the window horizontally centred near the top. Esc closes
/// the model list first, then the window.
fn desktop(app: AppState, draft: Signal<Option<String>>) -> AnyElement {
    let win = tracked_window(app);
    let anywhere = KeyOptions {
        when: When::Always,
        ..KeyOptions::default()
    };
    // Bound while the window shows; the newest binding wins over the app's own Escape.
    on_key_with("escape", anywhere, move |_| {
        let mut closed = false;
        app.assistant.update(|state| closed = state.close_menu());
        if !closed {
            hide(app);
        }
    });
    let title = win.with_untracked(|w| w.title);
    let panel = view()
        .test_id("hotkey")
        .role(AccessibilityRole::Region)
        .label(title)
        .w(px(WIDTH))
        .max_w(move || px((window().width.0 - 32.0).max(320.0)))
        .gap(px(10.0))
        .pb(px(8.0))
        .rounded(px(12.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .child(dynamic(move || {
            let w = win.get();
            untracked(|| head(app, &w))
        }))
        .child(content(app, draft, win))
        .child(dynamic(move || {
            let (keys, lifetime) = win.with(|w| (w.keys, w.lifetime));
            untracked(|| foot(keys, lifetime))
        }));
    overlay()
        .inset_0()
        .items_center()
        .pt(px(TOP))
        .bg(color(Token::Shell).opacity(0.55))
        .child(panel)
        .into_element()
}

/// `.hkhead`: the brand, "Ask", the health in small, the reach, the Hide control.
fn head(app: AppState, window: &HotkeyWindow) -> AnyElement {
    let hide_label = window.hide;
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(14.0))
        .pt(px(10.0))
        .child(icon(Icon::Brand, 16.0, color(Token::Ink)))
        .child(
            text(window.brand)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(
            text(window.name)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .text_color(color(Token::Ink3)),
        )
        .child(health(app))
        .child(view().flex_1())
        .child(
            text(window.reach)
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .text_size(px(11.5))
                .text_color(color(Token::Ink3)),
        )
        .child(
            view()
                .test_id("btn-hide")
                .role(AccessibilityRole::Button)
                .label(hide_label)
                .on_press(move || hide(app))
                .w(px(26.0))
                .h(px(26.0))
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(|s| {
                    s.outline(px(2.0))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(1.0))
                })
                .child(icon(Icon::Close, 12.0, color(Token::Ink3))),
        )
        .into_element()
}

/// `.health.mini`: each segment's dot in its tone, its name quiet, its value, ruled apart.
fn health(app: AppState) -> AnyElement {
    dynamic(move || {
        let segments = app.snapshot().health.segments;
        untracked(|| {
            view()
                .flex_row()
                .items_center()
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .children(segments.into_iter().enumerate().map(|(index, segment)| {
                    segment_view(index > 0, segment.name, segment.value, segment.tone)
                }))
                .into_element()
        })
    })
}

fn segment_view(
    ruled: bool,
    name: std::borrow::Cow<'static, str>,
    value: std::borrow::Cow<'static, str>,
    segment: Tone,
) -> AnyElement {
    let words = |content: std::borrow::Cow<'static, str>, ink: Token| {
        text(content.into_owned())
            .number_of_lines(1u32)
            .font_family(FONT_TEXT)
            .text_size(px(11.0))
            .text_color(color(ink))
    };
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(4.0))
        .h(px(18.0))
        .px(px(6.0))
        .when(ruled, |v| {
            v.border_l(px(1.0)).border_l_color(color(Token::Rule2))
        })
        .child(
            view()
                .w(px(6.0))
                .h(px(6.0))
                .rounded(px(3.0))
                .bg(tone(segment))
                .accessibility_elements_hidden(true),
        )
        .child(words(name, Token::Ink3))
        .child(words(value, Token::Ink2))
        .into_element()
}

/// The switch (fixed on the assistant, as the window's model is), the pill, the field, the state
/// of what was just sent, the result.
fn content(app: AppState, draft: Signal<Option<String>>, window: Memo<HotkeyWindow>) -> AnyElement {
    let form = app.form;
    let row = element_ref();
    let entry = element_ref();
    let menu: Memo<Option<ModelList>> =
        computed(move || window.with(|w| w.composer.model_menu.clone()));
    let model = computed(move || {
        window.with(|w| {
            w.composer
                .model_pill
                .as_ref()
                .map(|b| b.label)
                .zip(w.composer.model_name)
        })
    });
    let sent = computed(move || window.with(|w| w.sent.clone()));
    let inset = if form == Form::Desktop { 14.0 } else { 0.0 };
    after_answer(move || entry.focus());
    let shown = move || {
        draft
            .get()
            .unwrap_or_else(|| window.with(|w| w.input.to_owned()))
    };
    let submit = move || {
        let words = untracked(shown);
        let before = app.assistant.with_untracked(|state| state.sent.len());
        app.act(app.assistant, |state, env| {
            state.send_from_hotkey(&words, env)
        });
        if app.assistant.with_untracked(|state| state.sent.len()) > before {
            draft.set(Some(String::new()));
        }
    };
    let (height, size) = match form {
        Form::Desktop => (44.0, 17.0),
        Form::Phone => (48.0, 16.0),
        Form::Terminal => (16.0, text_size(form)),
    };
    let field = input()
        .element_ref(&entry)
        .test_id("hotkey-input")
        .label(window.with_untracked(|w| w.name))
        .submit_behavior(SubmitBehavior::Submit)
        .return_key_type(ReturnKeyType::Send)
        .value(shown)
        .on_change(move |text: &str| draft.set(Some(text.to_owned())))
        .on_submit(move |_: &str| submit())
        .text_color(color(Token::Ink))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .h(px(height))
        .mx(px(inset))
        .px(px(if form == Form::Terminal { 8.0 } else { 14.0 }));
    let field = if form == Form::Terminal {
        field.bg(AdaptiveColor::TRANSPARENT)
    } else {
        field
            .font_family(FONT_TEXT)
            .text_size(px(size))
            .rounded(px(8.0))
            .bg(color(Token::BgSunk))
            .border(px(1.5))
            .border_color(color(Token::Focus))
    };
    view()
        .gap(px(10.0))
        .child(
            view()
                .element_ref(&row)
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(inset))
                .child(dynamic(move || {
                    let targets = window.with(|w| w.composer.targets.clone());
                    untracked(|| fixed_switch(app, &targets))
                }))
                .child(dynamic(move || {
                    model.get().map(|(label, name)| {
                        untracked(|| pill(app, label, name, ListAt::Below, row, menu))
                    })
                })),
        )
        .child(field)
        .child(dynamic(move || {
            sent.get().map(|state| {
                untracked(|| {
                    view()
                        .flex_row()
                        .px(px(inset))
                        .child(chip(app, state.label, state.hue))
                        .into_element()
                })
            })
        }))
        .child(dynamic(move || {
            let w = window.get();
            untracked(|| result(app, &w, inset))
        }))
        .into_element()
}

/// The window's Assistant | PM, shown as the mockup draws it: the assistant pressed, the PM with
/// its dot. The window always asks the assistant, so neither is a control.
fn fixed_switch(app: AppState, targets: &[TargetOption]) -> AnyElement {
    let form = app.form;
    view()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(px(7.0))
        .when(form != Form::Terminal, |v| {
            v.bg(color(Token::BgSunk))
                .border_1()
                .border_color(color(Token::Rule))
        })
        .children(targets.iter().map(|option| {
            let words = text(option.label)
                .number_of_lines(1u32)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(if option.selected || form == Form::Terminal {
                    Token::Ink
                } else {
                    Token::Ink3
                }));
            let words = if form == Form::Terminal {
                words
            } else {
                words.font_family(FONT_TEXT).text_size(px(12.0))
            };
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .h(px(if form == Form::Terminal { 16.0 } else { 24.0 }))
                .px(px(10.0))
                .rounded(px(5.0))
                .accessibility_state(AccessibilityState {
                    selected: option.selected,
                    ..AccessibilityState::default()
                })
                .when(option.selected && form != Form::Terminal, |v| {
                    v.bg(color(Token::Surface))
                        .border_1()
                        .border_color(color(Token::Rule2))
                })
                .child(words)
                .children(
                    option
                        .state
                        .as_ref()
                        .map(|state| state_mark(app, "", state.hue)),
                )
        }))
        .into_element()
}

/// `.hkres`: who answered and what it cost, the answer with the records it rests on, the filed
/// card, and the way back to the quick thread. Nothing while the owner is typing.
fn result(app: AppState, window: &HotkeyWindow, inset: f32) -> AnyElement {
    if window.result == HotkeyResult::Typing {
        return view().into_element();
    }
    let form = app.form;
    let mut body = view()
        .gap(px(4.0))
        .mx(px(inset))
        .when(form != Form::Terminal, |v| {
            v.px(px(12.0))
                .py(px(10.0))
                .rounded(px(8.0))
                .bg(color(Token::BgSunk))
        })
        .children(window.attribution.map(|line| dim(app, line)));
    match &window.result {
        HotkeyResult::Typing => {}
        HotkeyResult::AnswerAndFiled {
            answer,
            sources,
            filed,
        } => {
            body = body
                .child(rich(app, answer))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .children(window.result.sources_label().map(|label| dim(app, label)))
                        .children(sources.iter().map(|source| rich(app, source))),
                )
                .child(compact(app, filed));
        }
        HotkeyResult::Filed(filed) => body = body.child(compact(app, filed)),
    }
    body.child(
        view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .children(window.reply_note.map(|note| dim(app, note)))
            .child(view().flex_1())
            .children(window.open.as_ref().map(|open| {
                let action = open.action.clone();
                button(
                    app,
                    open.label,
                    ButtonKind::Quiet,
                    "btn-open-in-corral",
                    move || {
                        act(app, action.clone());
                        hide(app);
                    },
                )
            })),
    )
    .into_element()
}

/// `.hkfiled`: the filed chip, the idea's title, how the words were kept.
fn compact(app: AppState, filed: &CompactFiled) -> AnyElement {
    let form = app.form;
    let title = text(filed.title)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    let title = if form == Form::Terminal {
        title
    } else {
        title.font_family(FONT_TEXT).text_size(px(12.5))
    };
    view()
        .gap(px(2.0))
        .items_start()
        .mt(px(4.0))
        .when(form != Form::Terminal, |v| {
            v.px(px(10.0))
                .py(px(8.0))
                .rounded(px(6.0))
                .bg(color(Token::Surface))
                .border_1()
                .border_color(color(Token::Rule2))
        })
        .child(chip(app, filed.chip.label.clone(), filed.chip.hue))
        .child(title)
        .child(dim(app, filed.note))
        .into_element()
}

/// `.hkfoot`: the keys as caps with their words, and why the window outlives the main one.
fn foot(keys: &'static [KeyHint], lifetime: &'static str) -> AnyElement {
    let quiet = |words: &'static str| {
        text(words)
            .number_of_lines(1u32)
            .font_family(FONT_TEXT)
            .text_size(px(11.5))
            .text_color(color(Token::Ink3))
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(14.0))
        .children(keys.iter().map(|hint| {
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(key_hint(hint.keys))
                .child(quiet(hint.action))
        }))
        .child(view().flex_1())
        .child(quiet(lifetime).flex_shrink(1.0).min_w(px(0.0)))
        .into_element()
}
