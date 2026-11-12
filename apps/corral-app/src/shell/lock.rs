//! The lock-screen push demo the phone shows over everything while `AppState::lock` holds (the
//! mock's `lockScreen()` and `.lock`): the date and the clock, the push notification of the
//! question that needs the owner, and the note on how push text is made. Pressing the
//! notification opens its page; pressing anywhere else puts the lock screen away.
//!
//! The lock screen is the phone's own surface, not the app's, so it looks the same in both
//! appearances: the always-dark shell colours for the wallpaper, and the light values of the
//! surface and ink for the notification's frosted card.

use nsh_core::prelude::*;

use crate::model::Page;
use crate::model::nav::LOCK_SCREEN;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The wallpaper's top padding under the status bar (`.lock` 74 pt less the 54 pt status bar).
const TOP: f32 = 20.0;
/// The notification card (`.lock .notif`): its widest, radius, paddings and ground's opacity.
const CARD_WIDTH: f32 = 358.0;
const CARD_RADIUS: f32 = 22.0;
const CARD_OPACITY: f32 = 0.78;
/// The app's icon in the card's head (`.nh .appi`).
const APP_ICON: f32 = 20.0;
/// Room between the card and the note, and under the note for the home indicator.
const CARD_GAP: f32 = 44.0;
const BOTTOM: f32 = 30.0;

/// The lock screen. `open` gets the notification's page when the card is pressed; the shell
/// clears the lock and navigates there. test_id "lock-screen" on the wallpaper, "lock-push" on
/// the notification.
pub fn lock_screen(app: AppState, open: impl Fn(Page) + 'static) -> AnyElement {
    let lock = LOCK_SCREEN;
    let ink = color(Token::ShellInk);
    view()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .items_center()
        .pt_safe_area_plus(px(TOP))
        .pb_safe_area_plus(px(BOTTOM))
        .px(px(16.0))
        .bg(color(Token::Shell))
        .bg_linear_to_b()
        .gradient_from(color(Token::ShellSel))
        .gradient_to(color(Token::Shell))
        .role(AccessibilityRole::Button)
        .label(format!("{}, {}", lock.date, lock.time))
        .test_id("lock-screen")
        .on_press(move || app.lock.set(false))
        .child(
            text(lock.date)
                .font_family(FONT_TEXT)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink.opacity(0.9)),
        )
        .child(
            text(lock.time)
                .mt(px(2.0))
                .font_family(FONT_TEXT)
                .text_size(px(86.0))
                .line_height(px(86.0))
                .letter_spacing(px(-2.5))
                .font_weight(FontWeight::BOLD)
                .text_color(ink),
        )
        .child(view().flex_1())
        .child(card(open))
        .child(view().h(px(CARD_GAP)))
        .child(
            text(lock.footnote)
                .max_w(px(CARD_WIDTH - 40.0))
                .text_center()
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .line_height(px(17.0))
                .text_color(ink.opacity(0.7)),
        )
        .into_element()
}

/// The notification (`.lock .notif`): the app's icon and name with the time at the trailing
/// edge, then the bold title over the question.
fn card(open: impl Fn(Page) + 'static) -> AnyElement {
    let lock = LOCK_SCREEN;
    let ink = on_light(Token::Ink);
    view()
        .w_full()
        .max_w(px(CARD_WIDTH))
        .px(px(14.0))
        .py(px(12.0))
        .gap(px(4.0))
        .rounded(px(CARD_RADIUS))
        .bg(on_light(Token::Surface).opacity(CARD_OPACITY))
        .shadow_lg()
        .shadow_color(color(Token::Shell))
        .role(AccessibilityRole::Button)
        .label(format!("{}, {}. {}", lock.app, lock.title, lock.body))
        .test_id("lock-push")
        .cursor_pointer()
        .active(|s| s.opacity(0.85))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::ShellInk))
                .outline_offset(px(2.0))
        })
        .on_press(move || open(lock.opens))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    view()
                        .size(px(APP_ICON))
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .bg(color(Token::Shell))
                        .child(icon(Icon::Brand, 14.0, color(Token::ShellInk))),
                )
                .child(
                    text(lock.app)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ink),
                )
                .child(view().flex_1())
                .child(
                    text(lock.when)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .text_color(ink.opacity(0.7)),
                ),
        )
        .child(
            text(lock.title)
                .font_family(FONT_TEXT)
                .text_size(px(15.0))
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .text_color(ink),
        )
        .child(
            text(lock.body)
                .font_family(FONT_TEXT)
                .text_size(px(15.0))
                .line_height(px(20.0))
                .text_color(ink),
        )
        .into_element()
}

/// The token's light value in both appearances: the notification is drawn by the phone, whose
/// banners stay light over the wallpaper.
fn on_light(token: Token) -> AdaptiveColor {
    let light = color(token).light;
    AdaptiveColor { light, dark: light }
}
