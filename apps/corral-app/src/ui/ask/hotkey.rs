//! The labelled preview's illustration of the small Ask window (`.hkwin`), not an OS window or
//! shortcut. The real native shortcut is installed by `os_integration` and opens Ask. The
//! window holds Ask's own composer, so what is typed and sent here is Ask's draft and message,
//! through the same path as the drawer. A dimmed backdrop stands in for the desktop; a terminal
//! shows the same content in its modal.

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::assistant::{KeyHint, ThreadRef};
use crate::model::{Form, Tone};
use crate::state::AppState;
use crate::ui::atoms::key_hint;
use crate::ui::composer::{Host, ListAt, field};
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::overlay::{Modal, modal};
use crate::ui::kit::text::dim;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, tone};

/// The window's width on the illustrated desktop (`.win10 .hkwin`).
const WIDTH: f32 = 720.0;
/// Room above the window, where the desktop shows.
const TOP: f32 = 96.0;

const TITLE: &str = "Corral · Ask · demo";
const BRAND: &str = "Corral";
const NAME: &str = "Ask";
const HIDE: &str = "Hide";
const LIFETIME: &str = "Demo window · the real shortcut opens Ask";
const REPLY_NOTE: &str = "Replies continue the quick thread";
const OPEN: &str = "Open in Corral";
const KEYS: &[KeyHint] = &[
    KeyHint {
        keys: "Ctrl Shift Space",
        action: "open from anywhere",
    },
    KeyHint {
        keys: "↵",
        action: "send",
    },
    KeyHint {
        keys: "Esc",
        action: "hide",
    },
];

pub(super) fn hotkey(app: AppState) -> AnyElement {
    if app.form != Form::Desktop {
        return modal(
            app,
            move || app.hotkey.get(),
            move || hide(app),
            move || Modal {
                test_id: "hotkey".into(),
                title: TITLE.into(),
                sub: Some(LIFETIME.into()),
                body: content(app),
                foot: None,
                done: Some(HIDE.into()),
                confirm: None,
            },
        );
    }
    when(move || app.hotkey.get(), move || untracked(|| desktop(app))).into_element()
}

/// Hides the window, closing its model list with it.
fn hide(app: AppState) {
    app.assistant.update(|state| {
        state.close_menu();
    });
    app.hotkey.set(false);
}

/// The desktop: the dimmed backdrop standing in for the desktop, the window horizontally
/// centred near the top. Esc closes the model list first, then the window.
fn desktop(app: AppState) -> AnyElement {
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
    let panel = view()
        .test_id("hotkey")
        .role(AccessibilityRole::Region)
        .label(TITLE)
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
        .child(head(app))
        .child(view().px(px(14.0)).child(content(app)))
        .child(foot());
    overlay()
        .inset_0()
        .items_center()
        .pt(px(TOP))
        .bg(color(Token::Shell).opacity(0.55))
        .child(panel)
        .into_element()
}

/// `.hkhead`: the brand, "Ask", the health in small, the reach, the Hide control.
fn head(app: AppState) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(14.0))
        .pt(px(10.0))
        .child(icon(Icon::Brand, 16.0, color(Token::Ink)))
        .child(
            text(BRAND)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(
            text(NAME)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .text_color(color(Token::Ink3)),
        )
        .child(health(app))
        .child(view().flex_1())
        .child(
            text(move || {
                SharedString::from(
                    app.snapshot()
                        .health
                        .reach
                        .map(|reach| reach.into_owned())
                        .unwrap_or_default(),
                )
            })
            .number_of_lines(1u32)
            .font_family(FONT_MONO)
            .text_size(px(11.5))
            .text_color(color(Token::Ink3)),
        )
        .child(
            view()
                .test_id("btn-hide")
                .role(AccessibilityRole::Button)
                .label(HIDE)
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

/// Ask's composer on the quick thread, its model list opening below the field, then the way to
/// the thread in the main window.
fn content(app: AppState) -> AnyElement {
    view()
        .gap(px(10.0))
        .child(field(
            app,
            Host {
                thread: ThreadRef::quick(),
                list: ListAt::Below,
                docked: false,
            },
        ))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(dim(app, REPLY_NOTE))
                .child(view().flex_1())
                .child(button(
                    app,
                    OPEN,
                    ButtonKind::Quiet,
                    "btn-open-in-corral",
                    move || {
                        hide(app);
                        app.open_ask();
                    },
                )),
        )
        .into_element()
}

/// `.hkfoot`: the keys as caps with their words, and that this window is a demo.
fn foot() -> AnyElement {
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
        .children(KEYS.iter().map(|hint| {
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(key_hint(hint.keys))
                .child(quiet(hint.action))
        }))
        .child(view().flex_1())
        .child(quiet(LIFETIME).flex_shrink(1.0).min_w(px(0.0)))
        .into_element()
}
