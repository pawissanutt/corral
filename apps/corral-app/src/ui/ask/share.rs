//! The labelled preview's illustration of a phone share extension (`.psheet.osshare`,
//! `.asksheet`). It neither registers an OS share target nor receives content from another
//! application: NSH 0.5.0's packaged hosts expose inbound URLs, not an inbound share payload or
//! extension API. Sending hands the drawn link and the caption to Ask as its draft, so nothing
//! leaves the device until the owner sends it there, through Ask's own path.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::illus;
use crate::ui::kit::text::dim;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The browser page drawn as the thing being shared.
const PAGE_TITLE: &str = "DiffView";
const PAGE_URL: &str = "nsh.dev/elements/diff-view";
const LINK_TITLE: &str = "DiffView · Native Shell";
const LINK_URL: &str = "https://nsh.dev/elements/diff-view";
const EXCERPT: &str = "The DiffView component: the changes between two texts or of a patch, side by side or unified, with the words changed within lines highlighted, unchanged lines collapsed into separators that expand…";
const PAGE_NOTE: &str = "Another app (browser), not Corral.";

/// The system sheet's apps (only Corral answers a press) and actions.
const APPS: [&str; 5] = ["AirDrop", "Messages", "Mail", "Corral", "Notes"];
const CORRAL: &str = "Corral";
const ACTIONS: [&str; 3] = ["Copy", "Add to Reading List", "Add Bookmark"];
const ILLUSTRATIVE: &str = "Share sheet demo · drawn for illustration";

const SHEET_TITLE: &str = "Share";
const COMPOSE_TITLE: &str = "Share to Corral";
const CANCEL: &str = "Cancel";
const SEND: &str = "Send";
const PLACEHOLDER: &str = "Add a caption (optional)";
const COMPOSE_NOTE: &str =
    "Opens Ask with the link and your caption; nothing is sent until you send it there.";

/// Where the extension is: the system's app chooser, or Corral's caption step.
#[derive(Clone, Copy, PartialEq)]
enum Step {
    Sheet,
    Compose,
}

pub(super) fn share(app: AppState) -> AnyElement {
    let step = signal(Step::Sheet);
    // The optional caption: the extension's own field, gone with the extension.
    let caption = signal(String::new());
    when(
        move || app.share.get(),
        move || untracked(|| phone(app, step, caption)),
    )
    .into_element()
}

/// Leaves the extension at whatever step: the demo closes and starts over next time.
fn close(app: AppState, step: Signal<Step>, caption: Signal<String>) {
    app.share.set(false);
    step.set(Step::Sheet);
    caption.set(String::new());
}

/// Hands the caption and the link to Ask as its draft, then leaves the extension.
fn send(app: AppState, step: Signal<Step>, caption: Signal<String>) {
    let words = caption.get_untracked();
    let words = words.trim();
    let draft = if words.is_empty() {
        LINK_URL.to_owned()
    } else {
        format!("{words}\n{LINK_URL}")
    };
    close(app, step, caption);
    app.open_ask_with(&draft);
}

/// The phone: the shared page fills the screen, a scrim over it that dismisses the extension,
/// the sheet at the bottom riding on the keyboard while the caption has it. Back closes it too.
fn phone(app: AppState, step: Signal<Step>, caption: Signal<String>) -> AnyElement {
    overlay()
        .inset_0()
        .on_cancel(move || close(app, step, caption))
        .child(shared_page())
        .child(
            view()
                .absolute()
                .inset_0()
                .bg(color(Token::Shell).opacity(0.5))
                .focusable(false)
                .accessibility_elements_hidden(true)
                .on_press(move || close(app, step, caption)),
        )
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .pb_keyboard()
                .test_id("share")
                .role(AccessibilityRole::Region)
                .label(ILLUSTRATIVE)
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
                        .child(dynamic(move || {
                            let now = step.get();
                            Some(untracked(|| match now {
                                Step::Sheet => system_sheet(app, step, caption),
                                Step::Compose => compose(app, step, caption),
                            }))
                        })),
                ),
        )
        .into_element()
}

/// The browser page being shared (`.sharepage`): its address, title, excerpt and the note that
/// this is another app.
fn shared_page() -> AnyElement {
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
                .child(words(PAGE_URL, 14.0, Token::Ink2)),
        )
        .child(
            text(PAGE_TITLE)
                .font_family(FONT_TEXT)
                .text_size(px(26.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(words(EXCERPT, 15.0, Token::Ink2))
        .child(words(PAGE_NOTE, 15.0, Token::Ink3))
        .into_element()
}

/// `.sh-h`: Cancel, the title in the middle, and Send while composing.
fn extension_head(
    app: AppState,
    title: &'static str,
    cancel: impl Fn() + 'static,
    send: Option<Box<dyn Fn()>>,
) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .px(px(8.0))
        .pb(px(8.0))
        .child(view().w(px(88.0)).flex_row().child(button(
            app,
            CANCEL,
            ButtonKind::Quiet,
            "btn-cancel",
            cancel,
        )))
        .child(
            text(title)
                .role(AccessibilityRole::Header)
                .flex_1()
                .min_w(px(0.0))
                .text_center()
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(
            view().w(px(88.0)).flex_row().justify_end().children(
                send.map(|send| button(app, SEND, ButtonKind::Primary, "btn-send", send)),
            ),
        )
        .into_element()
}

/// The system sheet (`.osshare`): Cancel and its title, the shared item, the apps (Corral opens
/// its extension), the system actions, the illustration tag.
fn system_sheet(app: AppState, step: Signal<Step>, caption: Signal<String>) -> AnyElement {
    view()
        .child(extension_head(
            app,
            SHEET_TITLE,
            move || close(app, step, caption),
            None,
        ))
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
                        .child(icon(Icon::Review, 20.0, color(Token::Ink2))),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            text(LINK_TITLE)
                                .number_of_lines(1u32)
                                .font_family(FONT_TEXT)
                                .text_size(px(15.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(color(Token::Ink)),
                        )
                        .child(
                            text(PAGE_URL)
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
                .children(APPS.iter().map(|&label| app_tile(label, step))),
        )
        .child(
            view()
                .mx(px(16.0))
                .mb(px(10.0))
                .rounded(px(12.0))
                .bg(color(Token::Surface2))
                .children(ACTIONS.iter().enumerate().map(|(index, &action)| {
                    view()
                        .px(px(14.0))
                        .py(px(12.0))
                        .when(index + 1 < ACTIONS.len(), |row| {
                            row.border_b(px(1.0)).border_color(color(Token::Rule))
                        })
                        .child(
                            text(action)
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
                .child(illus(app, ILLUSTRATIVE)),
        )
        .into_element()
}

/// One app of the system sheet (`.appi2`). Only Corral answers a press; the others are drawn.
fn app_tile(label: &'static str, step: Signal<Step>) -> AnyElement {
    let corral = label == CORRAL;
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
                .child(icon(Icon::Brand, 30.0, color(Token::ShellInk)))
        });
    let name = text(label)
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
        .child(name);
    if !corral {
        return item.accessibility_elements_hidden(true).into_element();
    }
    item.test_id("btn-share-corral")
        .role(AccessibilityRole::Button)
        .label(label)
        .cursor_pointer()
        .on_press(move || step.set(Step::Compose))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(2.0))
        })
        .into_element()
}

/// Corral's step: Cancel, the title and Send, the link, the optional caption and what Send does.
fn compose(app: AppState, step: Signal<Step>, caption: Signal<String>) -> AnyElement {
    view()
        .child(extension_head(
            app,
            COMPOSE_TITLE,
            move || close(app, step, caption),
            Some(Box::new(move || send(app, step, caption))),
        ))
        .child(
            view()
                .gap(px(10.0))
                .px(px(16.0))
                .pb(px(10.0))
                .child(attachment())
                .child(caption_field(caption))
                .child(dim(app, COMPOSE_NOTE)),
        )
        .into_element()
}

/// The shared link (`.attcard`): its title and address.
fn attachment() -> AnyElement {
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
        .child(icon(Icon::Review, 18.0, color(Token::Ink2)))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(LINK_TITLE)
                        .number_of_lines(1u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
                .child(
                    text(LINK_URL)
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .text_size(px(12.0))
                        .text_color(color(Token::Ink3)),
                ),
        )
        .into_element()
}

/// The optional caption, focused as the step opens so the keyboard comes up with it.
fn caption_field(caption: Signal<String>) -> AnyElement {
    let entry = element_ref();
    after_answer(move || entry.focus());
    input()
        .element_ref(&entry)
        .test_id("share-caption")
        .label(PLACEHOLDER)
        .multiline(true)
        .placeholder(PLACEHOLDER)
        .value(move || caption.get())
        .on_change(move |text: &str| caption.set(text.to_owned()))
        .h(px(88.0))
        .px(px(12.0))
        .py(px(10.0))
        .font_family(FONT_TEXT)
        .text_size(px(16.0))
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
