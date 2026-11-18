//! The lock-screen push demo the phone shows over everything while `AppState::lock` holds (the
//! mock's `lockScreen()` and `.lock`): the device's real date and clock, the push of the first
//! Blocked task in the feed, and the note on how push text is made. Pressing the notification
//! opens its task; pressing anywhere else puts the lock screen away. Once nothing is Blocked no
//! push shows, as on the phone.
//!
//! The lock screen is the phone's own surface, not the app's, so it looks the same in both
//! appearances: the always-dark shell colours for the wallpaper, and the light values of the
//! surface and ink for the notification's frosted card.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::Live;
use crate::model::daemon::Feed;
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

/// The notification's app identity, as the phone prints it.
const APP_NAME: &str = "CORRAL";
/// What the wallpaper is, said once above the date and to screen readers.
const DEMO_LABEL: &str = "Lock-screen push demo";
const DISMISS_LABEL: &str = "Lock-screen push demo, tap to dismiss";
const FOOTNOTE: &str =
    "Push text is generated on this phone from an encrypted payload; the relay only sends a wake.";
const NOTHING: &str = "Nothing needs you. A push shows here when a task is blocked.";

/// The push of the first Blocked task: its title line, why it is blocked, when it changed and
/// the page it opens.
#[derive(Debug, Clone, PartialEq)]
struct Push {
    title: String,
    body: String,
    when: String,
    opens: Live,
}

/// The first Blocked task in id order, or none once nothing is blocked.
fn push(feed: &Feed) -> Option<Push> {
    feed.board.tasks().iter().find_map(|task| {
        let blocked = task.blocked()?;
        let zone = feed.time_zone.clone().unwrap_or(jiff::tz::TimeZone::UTC);
        Some(Push {
            title: format!("Needs you · {}", task.id),
            body: blocked.reason.clone(),
            when: task
                .task()
                .updated
                .to_zoned(zone)
                .strftime("%H:%M")
                .to_string(),
            opens: Live::Task(task.id.clone()),
        })
    })
}

/// The feed's local day as a lock screen prints it, "Sunday 15 November".
fn day(feed: &Feed) -> String {
    feed.date
        .parse::<jiff::civil::Date>()
        .map(|date| date.strftime("%A %-d %B").to_string())
        .unwrap_or_default()
}

/// The lock screen. `open` gets the notification's question when the card is pressed; the shell
/// clears the lock and navigates there. test_id "lock-screen" on the wallpaper, "lock-push" on
/// the notification.
pub fn lock_screen(app: AppState, open: impl Fn(Live) + 'static) -> AnyElement {
    let open: Rc<dyn Fn(Live)> = Rc::new(open);
    let ink = color(Token::ShellInk);
    let shown = computed(move || app.feed.with(push));
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
        .label(DISMISS_LABEL)
        .test_id("lock-screen")
        .on_press(move || app.lock.set(false))
        .child(
            text(DEMO_LABEL)
                .mb(px(6.0))
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .text_color(ink.opacity(0.6)),
        )
        .child(
            text(move || SharedString::from(app.feed.with(day)))
                .font_family(FONT_TEXT)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink.opacity(0.9)),
        )
        .child(
            text(move || SharedString::from(app.feed.with(|feed| feed.clock.clone())))
                .mt(px(2.0))
                .font_family(FONT_TEXT)
                .text_size(px(86.0))
                .line_height(px(86.0))
                .letter_spacing(px(-2.5))
                .font_weight(FontWeight::BOLD)
                .text_color(ink),
        )
        .child(view().flex_1())
        .child(dynamic(move || {
            let shown = shown.get();
            let open = open.clone();
            Some(untracked(move || match shown {
                Some(push) => card(push, open),
                None => text(NOTHING)
                    .max_w(px(CARD_WIDTH - 40.0))
                    .text_center()
                    .font_family(FONT_TEXT)
                    .text_size(px(15.0))
                    .line_height(px(20.0))
                    .text_color(ink.opacity(0.8))
                    .into_element(),
            }))
        }))
        .child(view().h(px(CARD_GAP)))
        .child(
            text(FOOTNOTE)
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
fn card(push: Push, open: Rc<dyn Fn(Live)>) -> AnyElement {
    let ink = on_light(Token::Ink);
    let opens = push.opens.clone();
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
        .label(format!("{APP_NAME}, {}. {}", push.title, push.body))
        .test_id("lock-push")
        .cursor_pointer()
        .active(|s| s.opacity(0.85))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::ShellInk))
                .outline_offset(px(2.0))
        })
        .on_press(move || open(opens.clone()))
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
                    text(APP_NAME)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ink),
                )
                .child(view().flex_1())
                .child(
                    text(push.when)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .text_color(ink.opacity(0.7)),
                ),
        )
        .child(
            text(push.title)
                .font_family(FONT_TEXT)
                .text_size(px(15.0))
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .text_color(ink),
        )
        .child(
            text(push.body)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// PW-12: once nothing is blocked, no push shows on the lock screen.
    #[test]
    fn no_blocked_task_leaves_no_push() {
        use crate::model::daemon::{Task, bare_task, task_row};
        use corral_types::{Blocked, Status};
        let at = jiff::Timestamp::UNIX_EPOCH;
        let mut blocked = bare_task("T-8", "needs a reply", Status::Blocked, at);
        blocked.blocked = Some(Blocked {
            reason: "worker asks".into(),
            detail: None,
            next: Vec::new(),
        });
        let mut feed = Feed::default();
        feed.board_received(1, vec![Task::new(task_row(blocked))], &[], true);
        assert_eq!(
            push(&feed).map(|push| push.opens),
            Some(Live::Task("T-8".into()))
        );
        let ready = bare_task("T-8", "needs a reply", Status::Running, at);
        feed.board_received(2, vec![Task::new(task_row(ready))], &[], true);
        assert_eq!(push(&feed), None);
    }

    /// PW-18: the lock screen prints the feed's real day, not a mockup date.
    #[test]
    fn the_day_comes_from_the_feed() {
        let feed = Feed {
            date: "2026-11-15".into(),
            ..Feed::default()
        };
        assert_eq!(day(&feed), "Sunday 15 November");
    }
}
