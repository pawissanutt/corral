//! The phone share extension (`.psheet.osshare`, `.asksheet`): another app's page behind, the
//! system share sheet, then Corral's caption and Send, the item saved on this phone, and the
//! filed card once the app delivered it. The extension only writes an outbox item; nothing here
//! sends. Off the phone the same steps show in the form's modal, so the demo closes cleanly.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{
    Action, Button, FILED_LABELS, ShareApp, ShareContent, ShareSheet, ShareStep, SharedFiledCard,
    SharedPage, share_sheet,
};
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::ask::act;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::hue_fill;
use crate::ui::kit::overlay::{Modal, modal};
use crate::ui::kit::text::{dim, label_value, rich, rich_small};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

pub(super) fn share(app: AppState) -> AnyElement {
    // The optional caption: the extension's own field, gone with the extension.
    let caption = signal(String::new());
    // The demo switch only says "show the share sheet"; the extension starts at the system sheet.
    effect(move || {
        if app.share.get() && app.assistant.with_untracked(|state| state.share.is_none()) {
            untracked(|| act(app, Action::Share(Some(ShareStep::Sheet))));
        }
    });
    let shown = move || app.share.get() && app.assistant.with(|state| state.share.is_some());
    if app.form != Form::Phone {
        return modal(
            app,
            shown,
            move || close(app, caption),
            move || {
                let title = untracked(|| {
                    sheet_memo(app).with(|sheet| sheet.as_ref().map_or("", |sheet| sheet.title))
                });
                Modal {
                    test_id: "share".into(),
                    title: title.into(),
                    sub: None,
                    body: steps(app, caption),
                    done: None,
                    confirm: None,
                }
            },
        );
    }
    when(shown, move || untracked(|| phone(app, caption))).into_element()
}

/// The extension's content for the current step, tracked; the caption stays out of it so typing
/// never rebuilds the field.
fn sheet_memo(app: AppState) -> Memo<Option<ShareSheet>> {
    computed(move || {
        let env = app.env();
        app.assistant.with(|state| share_sheet(state, "", env))
    })
}

/// Leaves the extension: the model's step and the demo switch both close, the caption clears.
fn close(app: AppState, caption: Signal<String>) {
    act(app, Action::Share(None));
    app.share.set(false);
    caption.set(String::new());
}

/// Runs a step's action; one that ends the extension (Cancel, Done) also ends the demo.
fn dispatch(app: AppState, caption: Signal<String>, action: Action) {
    act(app, action);
    if app.assistant.with_untracked(|state| state.share.is_none()) {
        close(app, caption);
    }
}

/// The phone: the shared page fills the screen, a scrim over it, the sheet at the bottom riding
/// on the keyboard while the caption has it. Back closes the extension.
fn phone(app: AppState, caption: Signal<String>) -> AnyElement {
    let page = sheet_memo(app).with_untracked(|sheet| sheet.as_ref().map(|sheet| sheet.page));
    overlay()
        .inset_0()
        .on_cancel(move || close(app, caption))
        .children(page.map(|page| shared_page(&page)))
        .child(
            view()
                .absolute()
                .inset_0()
                .bg(color(Token::Shell).opacity(0.5))
                .focusable(false)
                .accessibility_elements_hidden(true),
        )
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .pb_keyboard()
                .bg(color(Token::Surface))
                .rounded_t(px(14.0))
                .border_t(px(1.0))
                .border_color(color(Token::Rule2))
                .shadow_2xl()
                .shadow_color(color(Token::Shell))
                .child(
                    view()
                        .pb(move || {
                            if keyboard_height() > px(0.0) {
                                px(0.0)
                            } else {
                                safe_area().bottom
                            }
                        })
                        .child(
                            view()
                                .self_center()
                                .w(px(36.0))
                                .h(px(5.0))
                                .mt(px(8.0))
                                .mb(px(6.0))
                                .rounded(px(3.0))
                                .bg(color(Token::Rule2))
                                .accessibility_elements_hidden(true),
                        )
                        .child(steps(app, caption)),
                ),
        )
        .into_element()
}

/// The browser page being shared (`.sharepage`): its address, title, excerpt and the note that
/// this is another app.
fn shared_page(page: &SharedPage) -> AnyElement {
    let words = |content: &'static str, size: f32, ink: Token| {
        text(content)
            .font_family(FONT_TEXT)
            .text_size(px(size))
            .line_height(px(size * 1.5))
            .text_color(color(ink))
    };
    view()
        .absolute()
        .inset_0()
        .px(px(18.0))
        .pt(move || safe_area().top + px(12.0))
        .gap(px(12.0))
        .bg(color(Token::Bg))
        .accessibility_elements_hidden(true)
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .h(px(36.0))
                .rounded(px(10.0))
                .bg(color(Token::BgSunk))
                .child(icon(Icon::Lock, 12.0, color(Token::Ink3)))
                .child(words(page.url, 14.0, Token::Ink2)),
        )
        .child(
            text(page.title)
                .font_family(FONT_TEXT)
                .text_size(px(26.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(words(page.excerpt, 15.0, Token::Ink2))
        .child(words(page.note, 15.0, Token::Ink3))
        .into_element()
}

/// The current step, rebuilt only when the step or the connection changes.
fn steps(app: AppState, caption: Signal<String>) -> AnyElement {
    let sheet = sheet_memo(app);
    dynamic(move || {
        sheet
            .get()
            .map(|sheet| untracked(|| step(app, caption, &sheet)))
    })
}

fn step(app: AppState, caption: Signal<String>, sheet: &ShareSheet) -> AnyElement {
    if let ShareContent::Sheet {
        apps,
        actions,
        illustrative,
    } = &sheet.content
    {
        return system_sheet(app, caption, sheet, apps, actions, illustrative);
    }
    let phone = app.form == Form::Phone;
    let body = view()
        .gap(px(10.0))
        .px(px(if phone { 16.0 } else { 0.0 }))
        .pb(px(10.0));
    let body = match &sheet.content {
        ShareContent::Saved { state, note, open } => {
            body.child(saved(app, caption, state.label.clone(), *note, open))
        }
        ShareContent::Filed { note, .. } => body.child(dim(app, *note)),
        _ => body,
    };
    let body = body.when(sheet.attachment_visible, |b| {
        b.child(attachment(app, &sheet.page))
    });
    let body = match &sheet.content {
        ShareContent::Compose {
            placeholder, note, ..
        } => body
            .child(caption_field(app, caption, placeholder))
            .child(rich_small(app, note)),
        ShareContent::Filed { card, done, .. } => body.child(filed_card(app, caption, card)).child(
            view()
                .flex_row()
                .child(step_button(app, caption, done, "btn-done")),
        ),
        _ => body,
    };
    view()
        .child(extension_head(app, caption, sheet))
        .child(body)
        .into_element()
}

/// The system sheet (`.osshare`): the shared item, the apps (Corral opens its extension), the
/// system actions, the illustration tag.
fn system_sheet(
    app: AppState,
    caption: Signal<String>,
    sheet: &ShareSheet,
    apps: &[ShareApp],
    actions: &[&'static str],
    illustrative: &'static str,
) -> AnyElement {
    let page = sheet.page;
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .px(px(16.0))
                .pt(px(6.0))
                .pb(px(12.0))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .child(
                    view()
                        .w(px(44.0))
                        .h(px(44.0))
                        .rounded(px(8.0))
                        .items_center()
                        .justify_center()
                        .bg(color(Token::Surface2))
                        .children(glyph(app, Icon::Review, 20.0, Token::Ink2)),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            text(page.attachment_title)
                                .number_of_lines(1u32)
                                .font_family(FONT_TEXT)
                                .text_size(px(15.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(color(Token::Ink)),
                        )
                        .child(
                            text(page.url)
                                .number_of_lines(1u32)
                                .font_family(FONT_TEXT)
                                .text_size(px(13.0))
                                .text_color(color(Token::Ink3)),
                        ),
                ),
        )
        .child(
            view()
                .flex_row()
                .gap(px(14.0))
                .px(px(16.0))
                .py(px(14.0))
                .children(
                    apps.iter()
                        .map(|share_app| app_tile(app, caption, share_app)),
                ),
        )
        .child(
            view()
                .mx(px(16.0))
                .mb(px(10.0))
                .rounded(px(12.0))
                .bg(color(Token::Surface2))
                .children(actions.iter().enumerate().map(|(index, action)| {
                    view()
                        .px(px(14.0))
                        .py(px(12.0))
                        .when(index + 1 < actions.len(), |row| {
                            row.border_b(px(1.0)).border_color(color(Token::Rule))
                        })
                        .child(
                            text(*action)
                                .font_family(FONT_TEXT)
                                .text_size(px(15.0))
                                .text_color(color(Token::Ink)),
                        )
                })),
        )
        .child(
            view()
                .px(px(16.0))
                .pb(px(6.0))
                .child(illus(app, illustrative)),
        )
        .into_element()
}

/// One app of the system sheet (`.appi2`). Only Corral answers a press; the others are drawn.
fn app_tile(app: AppState, caption: Signal<String>, share_app: &ShareApp) -> AnyElement {
    let corral = share_app.action.is_some();
    let tile = view()
        .w(px(58.0))
        .h(px(58.0))
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .bg(color(if corral {
            Token::Shell
        } else {
            Token::Surface2
        }))
        .when(corral, |t| {
            t.border(px(2.0))
                .border_color(color(Token::Focus))
                .children(glyph(app, Icon::Brand, 30.0, Token::ShellInk))
        });
    let label = text(share_app.label)
        .number_of_lines(1u32)
        .font_family(FONT_TEXT)
        .text_size(px(11.5))
        .text_color(color(if corral { Token::Ink } else { Token::Ink2 }))
        .when(corral, |t| t.font_weight(FontWeight::BOLD));
    let item = view()
        .w(px(62.0))
        .flex_shrink_0()
        .items_center()
        .gap(px(5.0))
        .child(tile)
        .child(label);
    match share_app.action.clone() {
        Some(action) => item
            .test_id(&format!("btn-share-{}", share_app.label.to_lowercase()))
            .role(AccessibilityRole::Button)
            .label(share_app.label)
            .cursor_pointer()
            .on_press(move || dispatch(app, caption, action.clone()))
            .focus_visible(|s| {
                s.outline(px(2.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(2.0))
            })
            .into_element(),
        None => item.accessibility_elements_hidden(true).into_element(),
    }
}

/// `.sh-h` of the extension: Cancel, the title in the middle, Send while composing. Off the phone
/// the modal carries the title, so the row keeps only the buttons.
fn extension_head(app: AppState, caption: Signal<String>, sheet: &ShareSheet) -> AnyElement {
    let phone = app.form == Form::Phone;
    let side = |button: Option<&Button>, test_id: &'static str| {
        view()
            .w(px(88.0))
            .flex_row()
            .children(button.map(|b| step_button(app, caption, b, test_id)))
    };
    view()
        .flex_row()
        .items_center()
        .px(px(if phone { 8.0 } else { 0.0 }))
        .pb(px(8.0))
        .child(side(sheet.cancel.as_ref(), "btn-cancel"))
        .child(if phone {
            text(sheet.title)
                .role(AccessibilityRole::Header)
                .flex_1()
                .min_w(px(0.0))
                .text_center()
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
                .into_element()
        } else {
            // The modal's own title row names the step off the phone.
            view().flex_1().into_element()
        })
        .child(side(sheet.send.as_ref(), "btn-send").justify_end())
        .into_element()
}

fn step_button(
    app: AppState,
    caption: Signal<String>,
    model: &Button,
    test_id: &'static str,
) -> AnyElement {
    let action = model.action.clone();
    let kind = if model.primary {
        ButtonKind::Primary
    } else {
        ButtonKind::Quiet
    };
    button(app, model.label, kind, test_id, move || {
        dispatch(app, caption, action.clone())
    })
}

/// Saved on this phone (`.notice.warn`): the state chip, when it sends, and Open Corral.
fn saved(
    app: AppState,
    caption: Signal<String>,
    state: String,
    note: &'static str,
    open: &Button,
) -> AnyElement {
    view()
        .gap(px(6.0))
        .px(px(14.0))
        .py(px(12.0))
        .rounded(px(10.0))
        .bg(hue_fill(Hue::Held))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .child(chip(app, state, Hue::Held))
                .child(
                    text(note)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.5))
                        .text_color(color(Token::Ink)),
                ),
        )
        .child(
            view()
                .flex_row()
                .child(step_button(app, caption, open, "btn-open-corral")),
        )
        .into_element()
}

/// The shared link (`.attcard`): its title and address.
fn attachment(app: AppState, page: &SharedPage) -> AnyElement {
    let phone = app.form == Form::Phone;
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(10.0))
        .bg(color(Token::Surface2))
        .border_1()
        .border_color(color(Token::Rule2))
        .children(glyph(app, Icon::Review, 18.0, Token::Ink2))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(page.attachment_title)
                        .number_of_lines(1u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 14.0 } else { 13.0 }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
                .child(
                    text(page.attachment_url)
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .text_size(px(12.0))
                        .text_color(color(Token::Ink3)),
                ),
        )
        .into_element()
}

/// The optional caption, focused as the step opens so the keyboard comes up with it.
fn caption_field(app: AppState, caption: Signal<String>, placeholder: &'static str) -> AnyElement {
    let phone = app.form == Form::Phone;
    let entry = element_ref();
    after_answer(move || entry.focus());
    input()
        .element_ref(&entry)
        .test_id("share-caption")
        .label(placeholder)
        .multiline(true)
        .placeholder(placeholder)
        .value(move || caption.get())
        .on_change(move |text: &str| caption.set(text.to_owned()))
        .h(px(if phone { 88.0 } else { 64.0 }))
        .px(px(12.0))
        .py(px(10.0))
        .font_family(FONT_TEXT)
        .text_size(px(if phone { 16.0 } else { 14.0 }))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .rounded(px(10.0))
        .bg(color(Token::Surface))
        .border(px(1.5))
        .border_color(color(Token::Focus))
        .into_element()
}

/// The idea the assistant filed from the captionless share (`.filed`): its chip, kind and the
/// link attached, who filed it, the title and labels it wrote, then Edit, Merge and Discard.
fn filed_card(app: AppState, caption: Signal<String>, card: &SharedFiledCard) -> AnyElement {
    view()
        .gap(px(6.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .child(chip(app, card.chip.label.clone(), card.chip.hue))
                .child(dim(app, card.description))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .children(glyph(app, Icon::Review, 12.0, Token::Ink2))
                        .child(chip(app, card.attachment, Hue::Neutral)),
                ),
        )
        .child(dim(app, card.attribution))
        .child(label_value(
            app,
            FILED_LABELS.title,
            text(card.title)
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
                .into_element(),
        ))
        .child(label_value(
            app,
            FILED_LABELS.labels,
            rich(app, &card.labels),
        ))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .mt(px(4.0))
                .children(card.buttons.iter().enumerate().map(|(index, b)| {
                    let action = b.action.clone();
                    let kind = if b.danger {
                        ButtonKind::Quiet
                    } else {
                        ButtonKind::Ghost
                    };
                    button(
                        app,
                        b.label,
                        kind,
                        &format!("btn-filed-{index}"),
                        move || dispatch(app, caption, action.clone()),
                    )
                })),
        )
        .into_element()
}

/// An icon where the form draws images; a terminal shows none.
fn glyph(app: AppState, glyph: Icon, size: f32, ink: Token) -> Option<AnyElement> {
    (app.form != Form::Terminal).then(|| icon(glyph, size, color(ink)))
}
