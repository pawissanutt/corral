//! Where Ask opens: the drawer docked on the right over the current screen on desktop, a pane of
//! the frame in a terminal, a sheet riding on the keyboard on a phone. Desktop and terminal show
//! the quick thread above the composer; the phone sheet holds the composer alone and links to the
//! thread. Without a connection each shows why nothing can be sent and the way to connect.

use nsh_core::prelude::*;

use crate::model::assistant::{self, Action, Button, Chip, QUICK, ThreadRef};
use crate::model::{DataState, Form, Id};
use crate::screens::assistant::{items, open_scope};
use crate::state::AppState;
use crate::ui::ask::{act, unconnected};
use crate::ui::atoms::control_height;
use crate::ui::composer::composer;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::chip;
use crate::ui::kit::page_phone::short_window;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// The drawer's width (`.drawer`).
const DRAWER_WIDTH: f32 = 500.0;
/// The sheet's largest share of the screen (`.asksheet`).
const SHEET_SHARE: f32 = 0.92;

/// The head of Ask, as the model resolves it for the form.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Head {
    label: &'static str,
    title: String,
    state: Option<Chip>,
    subtitle: Option<String>,
    buttons: Vec<Button>,
    close_label: &'static str,
}

/// The model's drawer head, tracked: it follows the feed's quick thread and the connection, never
/// the draft.
fn head(app: AppState) -> Memo<Head> {
    computed(move || {
        let env = app.env();
        let drawer = match app.data.get() {
            DataState::Disconnected => assistant::unconnected_drawer(),
            DataState::Daemon | DataState::Fixture => {
                app.feed.with(|feed| assistant::live_drawer(feed, env))
            }
        };
        Head {
            label: drawer.label,
            title: drawer.title,
            state: drawer.state,
            subtitle: drawer.subtitle,
            buttons: drawer.buttons,
            close_label: drawer.close_label,
        }
    })
}

/// The quick thread, subscribed and its host warmed while Ask is open, so the first answer does
/// not wait for it.
fn quick(app: AppState) -> ThreadRef {
    if untracked(|| app.data.get()) != DataState::Disconnected {
        open_scope(app, Id::from(QUICK), true);
    }
    ThreadRef::quick()
}

/// The test id of a head button.
fn button_id(action: &Action) -> &'static str {
    match action {
        Action::CloseAsk => "ask-close",
        _ => "btn-open-thread",
    }
}

/// The desktop drawer: absolute on the right of its parent, 500 pt wide, the page beside it still
/// visible and usable; the head, the quick thread scrolling to its newest message, the composer.
pub(crate) fn desktop(app: AppState) -> AnyElement {
    let head = head(app);
    let label = head.with_untracked(|h| h.label);
    // One short slide from the right edge, eased out; reduced motion skips it.
    let entrance = motion().entering(
        MotionSpec::new(KeyframeTiming::new(200.0).easing(Easing::Bezier(0.16, 1.0, 0.3, 1.0)))
            .values(MotionChannel::TranslateX, [24.0, 0.0])
            .values(MotionChannel::Opacity, [0.5, 1.0]),
    );
    view()
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .w(px(DRAWER_WIDTH))
        .max_w(relative(1.0))
        .role(AccessibilityRole::Region)
        .label(label)
        .bg(color(Token::Bg))
        .border_l(px(1.0))
        .border_l_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .bind_motion(&entrance)
        .child(dynamic(move || {
            let head = head.get();
            untracked(|| desktop_head(app, &head))
        }))
        .child(thread(app, false))
        .into_element()
}

/// The quick thread scrolling to its newest message over the composer, ruled apart in a
/// terminal (`rule`); without a connection, why nothing can be sent and the way to connect.
fn thread(app: AppState, rule: bool) -> AnyElement {
    when(
        move || unconnected::unconnected(app),
        move || untracked(|| unconnected::body(app)),
    )
    .otherwise(move || {
        untracked(|| {
            view()
                .flex_1()
                .min_h(px(0.0))
                .child(items(app, quick(app)))
                .when(rule, |v| v.child(term::hrule()))
                .child(composer(app))
                .into_element()
        })
    })
    .into_element()
}

/// `.dhead`: the title, the state chip, the cross-device scope (cut first), Open in Agents and
/// the close control, 44 pt high over a rule.
fn desktop_head(app: AppState, head: &Head) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(8.0))
        .h(px(44.0))
        .pl(px(16.0))
        .pr(px(10.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(
            text(head.title.clone())
                .role(AccessibilityRole::Header)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .children(
            head.state
                .as_ref()
                .map(|state| chip(app, state.label.clone(), state.hue)),
        )
        .children(head.subtitle.clone().map(|subtitle| {
            text(subtitle)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .text_color(color(Token::Ink3))
        }))
        .child(view().flex_1())
        .children(head.buttons.iter().map(|b| {
            if b.action == Action::CloseAsk {
                close_icon(app, b.action.clone(), head.close_label)
            } else {
                let action = b.action.clone();
                button(
                    app,
                    b.label,
                    ButtonKind::Quiet,
                    button_id(&b.action),
                    move || act(app, action.clone()),
                )
            }
        }))
        .into_element()
}

/// The drawer's close control: the close icon alone, named "Close (Esc)" for screen readers.
fn close_icon(app: AppState, action: Action, label: &'static str) -> AnyElement {
    view()
        .test_id("ask-close")
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(move || act(app, action.clone()))
        .size(px(control_height(30.0)))
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .active(|s| s.bg(color(Token::Sel)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(2.0))
        })
        .child(icon(Icon::Close, 14.0, color(Token::Ink2)))
        .into_element()
}

/// Ask in a terminal: a pane of the frame around the content, its title row, the quick thread and
/// the composer ruled apart by lines that join the frame. On a wide grid it takes half the content
/// beside the page, split from it by a line of its own, so nothing of the page is cut under it; on
/// a narrow grid it takes the whole frame while the shell sets the page aside.
pub(crate) fn pane(app: AppState) -> AnyElement {
    let head = head(app);
    let label = head.with_untracked(|h| h.label);
    let body = view()
        .flex_1()
        .min_w(px(0.0))
        .child(terminal_head(app, head))
        .child(term::hrule())
        .child(thread(app, true));
    let pane = view()
        .role(AccessibilityRole::Region)
        .label(label)
        .flex_row();
    if term::wide() {
        pane.w(relative(0.5))
            .flex_shrink_0()
            .child(term::vrule())
            .child(body)
            .into_element()
    } else {
        pane.flex_1().min_w(px(0.0)).child(body).into_element()
    }
}

/// Cells between two parts of the terminal head.
const HEAD_GAP: usize = 2;

/// The terminal title row: the title in bold, the state, Open in Agents, then Esc and Close as
/// the key line reads. The title and Esc Close always show whole; the state chip, then Open in
/// Agents, show only when they fit whole in the room the row measures.
fn terminal_head(app: AppState, head: Memo<Head>) -> AnyElement {
    let room = signal(None::<f32>);
    view()
        .flex_row()
        .items_center()
        .h(term::rows(1.0))
        .px(term::cells(1.0))
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            let head = head.get();
            let column = term::cells(1.0).0.max(1.0);
            // The row's own width less its padding of a Cell each side.
            let room = room.get().map_or(0, |width| {
                ((width / column + 0.01).floor() as usize).saturating_sub(2)
            });
            untracked(|| terminal_parts(app, &head, room))
        }))
        .into_element()
}

/// The head's parts that fit `room` Cells, in order.
fn terminal_parts(app: AppState, head: &Head, room: usize) -> Vec<AnyElement> {
    let close = head.buttons.iter().find(|b| b.action == Action::CloseAsk);
    let open = head.buttons.iter().find(|b| b.action != Action::CloseAsk);
    // A key cap is its key and a Cell either side; a chip or button its words and a Cell either
    // side.
    let close_width = close.map_or(0, |b| ESC.chars().count() + 2 + 1 + b.label.chars().count());
    let mut used = head.title.chars().count() + HEAD_GAP + close_width;
    let mut fits = |width: usize| {
        let fit = used + HEAD_GAP + width <= room;
        if fit {
            used += HEAD_GAP + width;
        }
        fit
    };
    let state = head
        .state
        .as_ref()
        .filter(|state| fits(state.label.chars().count() + 2));
    let open = open.filter(|b| fits(b.label.chars().count() + 2));
    let mut parts = vec![
        text(head.title.clone())
            .role(AccessibilityRole::Header)
            .number_of_lines(1u32)
            .flex_shrink_0()
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink))
            .into_element(),
    ];
    if let Some(state) = state {
        parts.push(term::space(HEAD_GAP as f32));
        parts.push(chip(app, state.label.clone(), state.hue));
    }
    parts.push(
        view()
            .flex_1()
            .min_w(term::cells(HEAD_GAP as f32))
            .into_element(),
    );
    if let Some(b) = open {
        let action = b.action.clone();
        parts.push(button(
            app,
            b.label,
            ButtonKind::Ghost,
            button_id(&b.action),
            move || act(app, action.clone()),
        ));
        parts.push(term::space(HEAD_GAP as f32));
    }
    if let Some(b) = close {
        let action = b.action.clone();
        parts.push(
            view()
                .test_id("ask-close")
                .role(AccessibilityRole::Button)
                .label(head.close_label)
                .on_press(move || act(app, action.clone()))
                .flex_row()
                .flex_shrink_0()
                .child(term::key_cap(ESC))
                .child(term::space(1.0))
                .child(
                    text(b.label)
                        .number_of_lines(1u32)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(Token::Ink)),
                )
                .into_element(),
        );
    }
    parts
}

/// The key that closes Ask, as the terminal's key line names it.
const ESC: &str = "Esc";

/// The phone sheet's panel (`.asksheet`): a grab handle, "Ask" with the quick-thread link and
/// Close, the composer; riding on the software keyboard, which its field raises as it opens, and
/// clear of the home indicator while the keyboard is down. Held sideways, the little height
/// goes to the field: the grab handle gives way (the sheet still swipes down) and the sides
/// keep clear of the notch.
pub(crate) fn phone_panel(app: AppState) -> AnyElement {
    let head = head(app);
    let label = head.with_untracked(|h| h.label);
    view()
        .pb_keyboard()
        .max_h(move || px(window().height.0 * SHEET_SHARE))
        .bg(color(Token::Surface))
        .rounded_t(px(14.0))
        .border_t(px(1.0))
        .border_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .role(AccessibilityRole::Region)
        .label(label)
        .child(
            view()
                .pl_safe_area()
                .pr_safe_area()
                .style_when(short_window, |s| s.pt(px(6.0)))
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
                        .mb(px(4.0))
                        .rounded(px(3.0))
                        .bg(color(Token::Rule2))
                        .accessibility_elements_hidden(true)
                        .style_when(short_window, |s| s.hidden()),
                )
                .child(dynamic(move || {
                    let head = head.get();
                    untracked(|| phone_head(app, &head))
                }))
                .child(
                    when(
                        move || unconnected::unconnected(app),
                        move || untracked(|| unconnected::body(app)),
                    )
                    .otherwise(move || {
                        untracked(|| {
                            // The sheet shows no transcript, but its sends land in the quick
                            // thread it subscribes.
                            let _ = quick(app);
                            composer(app)
                        })
                    }),
                ),
        )
        .into_element()
}

/// `.sh-h`: "Ask", then the quick-thread link and Close at the trailing edge.
fn phone_head(app: AppState, head: &Head) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .pl(px(18.0))
        .pr(px(6.0))
        .pb(px(2.0))
        .child(
            text(head.title.clone())
                .role(AccessibilityRole::Header)
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(text_size(Form::Phone)))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .children(head.buttons.iter().map(|b| {
            let action = b.action.clone();
            button(
                app,
                b.label,
                ButtonKind::Quiet,
                button_id(&b.action),
                move || act(app, action.clone()),
            )
        }))
        .into_element()
}
