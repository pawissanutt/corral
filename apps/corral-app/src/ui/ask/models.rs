//! The model pill and the model list it opens (`.pick`, `.modelmenu`): a popup above the
//! composer's control row in the drawer and under a thread, below it in the hotkey window, inline
//! under the row in the phone sheet.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{Action, ModelList, ModelOption};
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::ask::act;
use crate::ui::composer::ListAt;
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::mark;
use crate::ui::kit::text::rich_small;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// The list's widest extent (`width: min(440px, 100%)`) on desktop.
const POPUP_WIDTH: f32 = 440.0;
/// The disclosure triangle after the profile name: drawn chrome, not wording.
const DISCLOSURE: &str = " ▾";

/// The pill naming the model of the next message, `label` its accessible name. Pressing it opens
/// or closes the list; while `at` is a popup the open list hangs from `row` (the control row the
/// pill sits in), so it lines up with the row's start as the mockup's does. Pressing outside the
/// list closes it; Esc closes it before it closes Ask (the app's Escape order).
pub(crate) fn pill(
    app: AppState,
    label: &'static str,
    name: &'static str,
    at: ListAt,
    row: ElementRef,
    menu: Memo<Option<ModelList>>,
) -> AnyElement {
    let form = app.form;
    let open = move || menu.with(Option::is_some);
    let words = text(format!("{name}{DISCLOSURE}"))
        .number_of_lines(1u32)
        .text_color(move || {
            if open() || form == Form::Terminal {
                color(Token::Ink)
            } else {
                color(Token::Ink2)
            }
        });
    let face = view()
        .test_id("btn-model")
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(move || AccessibilityState {
            expanded: Some(open()),
            has_popup: true,
            ..AccessibilityState::default()
        })
        .on_press(move || act(app, Action::ToggleModelMenu))
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer();
    let face = match form {
        Form::Terminal => face
            .px(crate::ui::term::cells(1.0))
            .bg(color(Token::Surface))
            .child(words),
        Form::Desktop | Form::Phone => {
            let phone = form == Form::Phone;
            let (height, size, pad) = if phone {
                (32.0, 14.0, 12.0)
            } else {
                (26.0, 12.0, 10.0)
            };
            face.h(px(height))
                .px(px(pad))
                .rounded(px(6.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .bg(move || {
                    if open() {
                        color(Token::Surface2)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(|s| {
                    s.outline(px(2.0))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(2.0))
                })
                // 32 pt high, reaching the 44 pt target above and below.
                .when(phone, |v| {
                    v.hit_slop(EdgeInsets {
                        top: px(6.0),
                        right: px(0.0),
                        bottom: px(6.0),
                        left: px(0.0),
                    })
                })
                .child(words.font_family(FONT_TEXT).text_size(px(size)))
        }
    };
    let popup = match at {
        ListAt::Inline => None,
        ListAt::Above | ListAt::Below => Some(when(open, move || {
            overlay()
                .anchor(&row)
                .placement(if at == ListAt::Above {
                    Placement::Top
                } else {
                    Placement::Bottom
                })
                .align(Align::Start)
                .offset(px(6.0))
                .margin(px(8.0))
                .on_press_outside(move |_: &Point<Points>| {
                    app.assistant.update(|state| {
                        state.close_menu();
                    });
                })
                .child(dynamic(move || {
                    menu.get()
                        .map(|list| untracked(|| model_list(app, &list, false)))
                }))
        })),
    };
    view()
        .flex_row()
        .flex_shrink_0()
        .child(face)
        .children(popup)
        .into_element()
}

/// The open list inline under the control row (the phone sheet's), nothing while it is closed.
pub(crate) fn inline_list(app: AppState, menu: Memo<Option<ModelList>>) -> AnyElement {
    dynamic(move || {
        menu.get()
            .map(|list| untracked(|| model_list(app, &list, true)))
    })
}

/// The profiles in cycling order, the current one checked, then the footer naming where the list
/// is configured and the illustrative tag. A listbox; each profile an option with its selected
/// state, `model-<n>`.
fn model_list(app: AppState, list: &ModelList, inline: bool) -> AnyElement {
    let form = app.form;
    let rows = list
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| row(app, index, option, inline));
    let foot = view()
        .flex_row()
        .flex_wrap()
        .items_end()
        .gap(px(6.0))
        .child(rich_small(app, &list.footer))
        .child(illus(app, list.illustrative));
    let shell = view()
        .role(AccessibilityRole::Listbox)
        .label(list.label)
        .children(rows);
    if form == Form::Terminal {
        return shell
            .w(crate::ui::term::cells(56.0))
            .bg(color(Token::Surface))
            .child(crate::ui::term::hrule())
            .child(foot.px(crate::ui::term::cells(1.0)))
            .into_element();
    }
    let foot = foot
        .mt(px(3.0))
        .pt(px(6.0))
        .pb(px(4.0))
        .px(px(8.0))
        .border_t(px(1.0))
        .border_color(color(Token::Rule));
    let shell = shell
        .gap(px(1.0))
        .p(px(4.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(color(Token::Rule2));
    if inline {
        shell.bg(color(Token::Surface2)).child(foot).into_element()
    } else {
        shell
            .w(px(POPUP_WIDTH))
            .max_w(move || px((window().width.0 - 16.0).max(200.0)))
            .bg(color(Token::Surface))
            .shadow_xl()
            .shadow_color(color(Token::Shell))
            .child(foot)
            .into_element()
    }
}

/// One profile: the check on the current one, its name over the model and effort, its note at the
/// trailing edge. Picking it closes the list and shows the model's toast.
fn row(app: AppState, index: usize, option: &ModelOption, inline: bool) -> AnyElement {
    let form = app.form;
    let action = option.action.clone();
    let profile = option.profile;
    let check = if option.selected {
        mark(app, Mark::Done, Hue::Ok)
    } else {
        view().w(px(12.0)).into_element()
    };
    let item = view()
        .test_id(&format!("model-{index}"))
        .role(AccessibilityRole::Option)
        .label(profile.name)
        .accessibility_state(AccessibilityState {
            selected: option.selected,
            ..AccessibilityState::default()
        })
        .on_press(move || act(app, action.clone()))
        .flex_row()
        .items_center()
        .cursor_pointer();
    if form == Form::Terminal {
        return item
            .gap(crate::ui::term::cells(1.0))
            .px(crate::ui::term::cells(1.0))
            .child(check)
            .child(
                text(profile.name)
                    .number_of_lines(1u32)
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .child(
                text(profile.model)
                    .flex_1()
                    .min_w(px(0.0))
                    .number_of_lines(1u32)
                    .text_color(color(Token::Ink3)),
            )
            .child(
                text(profile.note)
                    .number_of_lines(1u32)
                    .text_color(color(Token::Ink3)),
            )
            .into_element();
    }
    let (name_size, model_size, note_size) = if inline {
        (15.0, 12.5, 12.5)
    } else {
        (text_size(form), 11.5, 11.5)
    };
    item.gap(px(8.0))
        .px(px(8.0))
        .py(px(7.0))
        .when(inline, |v| v.min_h(px(44.0)))
        .rounded(px(7.0))
        .when(option.selected, |v| v.bg(color(Token::Hover)))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-2.0))
        })
        .child(view().w(px(16.0)).items_center().child(check))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(
                    text(profile.name)
                        .number_of_lines(1u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(name_size))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
                .child(
                    text(profile.model)
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .text_size(px(model_size))
                        .text_color(color(Token::Ink3)),
                ),
        )
        .child(
            text(profile.note)
                .flex_shrink(1.0)
                .text_right()
                .font_family(FONT_TEXT)
                .text_size(px(note_size))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}
