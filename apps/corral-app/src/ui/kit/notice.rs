//! What a page says about itself rather than its content: notices in their tones, the error
//! box, the empty state, the loading skeleton and the connection banner.
//!
//! Tones are hues: `Hue::Neutral` is the plain notice (the mockup's `.notice`, `.conn-banner
//! .info`), `Hue::Held` warn, `Hue::Ok` ok, `Hue::Fail` bad. A terminal leads each with a
//! shaped mark in the tone, so the tone reads without its colour.

use nsh_core::prelude::*;

use crate::model::{Form, Hue, Rich};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::kit::text::{rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// One sweep of the skeleton's pulse, as the mockup's `.skel` shimmer.
const PULSE_MS: f32 = 1400.0;

/// A notice (`.notice`): the sentence on its tone's ground with an optional trailing action.
/// Desktop: padding 10 14, radius 8, 10 pt between text and action. Phone: the same at
/// padding 12 14, radius 10. Terminal: the tone's mark ([`tone_mark`]) in the tone and bold,
/// the sentence, the action at the end of the row. test_id "notice".
pub fn notice(app: AppState, hue: Hue, content: &Rich, action: Option<AnyElement>) -> AnyElement {
    toned(app, hue, None, content, action)
}

/// [`notice`] led by an icon in the tone's ink (the full ink when neutral), 16 pt on desktop and
/// 18 pt on the phone, 10 pt before the sentence, as the mockup's machines note. A terminal
/// shows no images, so it leads with the tone's mark as [`notice`] does. test_id "notice".
pub fn notice_icon(
    app: AppState,
    hue: Hue,
    glyph: Icon,
    content: &Rich,
    action: Option<AnyElement>,
) -> AnyElement {
    toned(app, hue, Some(glyph), content, action)
}

/// The body of [`notice`] and [`notice_icon`].
fn toned(
    app: AppState,
    hue: Hue,
    glyph: Option<Icon>,
    content: &Rich,
    action: Option<AnyElement>,
) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return marked_row(app, hue, content, action.into_iter().collect())
            .test_id("notice")
            .into_element();
    }
    let phone = form == Form::Phone;
    let ink = match hue {
        Hue::Neutral => color(Token::Ink),
        _ => hue_ink(hue),
    };
    view()
        .test_id("notice")
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(14.0))
        .py(px(if phone { 12.0 } else { 10.0 }))
        .rounded(px(if phone { 10.0 } else { 8.0 }))
        .bg(hue_fill(hue))
        .children(glyph.map(|glyph| icon(glyph, if phone { 18.0 } else { 16.0 }, ink)))
        .child(sentence(app, content))
        .children(action)
        .into_element()
}

/// The error box (`.err`): what failed and what happens now, on the failure ground, with an
/// optional action ("Retry now"). Desktop and phone: margin 14 18, padding 12 14, radius 8; bold
/// runs of `content` should carry `Hue::Fail` as the mockup's `.err b`. Terminal: `✗` in the
/// failure tone, the sentence and the action. role alert, test_id "error".
pub fn error(app: AppState, content: &Rich, action: Option<AnyElement>) -> AnyElement {
    if app.form == Form::Terminal {
        return marked_row(app, Hue::Fail, content, action.into_iter().collect())
            .test_id("error")
            .role(AccessibilityRole::Alert)
            .into_element();
    }
    view()
        .test_id("error")
        .role(AccessibilityRole::Alert)
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .mx(px(18.0))
        .my(px(14.0))
        .px(px(14.0))
        .py(px(12.0))
        .rounded(px(8.0))
        .bg(hue_fill(Hue::Fail))
        .child(sentence(app, content))
        .children(action)
        .into_element()
}

/// The empty state (`.empty`): a heading, a quiet paragraph, an optional action under them.
/// Desktop and phone: padding 40 28, at most 420 pt wide, left-aligned; heading 16 pt bold,
/// 6 pt above the paragraph, the action 14 pt under it. Terminal: a blank row, the heading in
/// bold, the paragraph, the action. The heading has the header role; test_id "empty".
pub fn empty(
    app: AppState,
    heading: impl Into<SharedString>,
    paragraph: impl Into<SharedString>,
    action: Option<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let heading: SharedString = heading.into();
    let paragraph: SharedString = paragraph.into();
    if form == Form::Terminal {
        return view()
            .test_id("empty")
            .px(term::cells(1.0))
            .child(term::blank())
            .child(
                text(heading)
                    .role(AccessibilityRole::Header)
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .child(text(paragraph).text_color(color(Token::Ink)))
            .when_some(action, |state, action| {
                state
                    .child(term::blank())
                    .child(view().flex_row().child(action))
            })
            .into_element();
    }
    let size = text_size(form);
    view()
        .test_id("empty")
        .px(px(28.0))
        .py(px(40.0))
        .max_w(px(420.0))
        .child(
            text(heading)
                .role(AccessibilityRole::Header)
                .mb(px(6.0))
                .font_family(FONT_TEXT)
                .text_size(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(
            text(paragraph)
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .line_height(px(size * 1.5))
                .text_color(color(Token::Ink3)),
        )
        .when_some(action, |state, action| {
            state.child(view().flex_row().mt(px(14.0)).child(action))
        })
        .into_element()
}

/// The loading skeleton (`.skel`): one bar per `(width, height)`, the width a fraction of the
/// row (0.4 is 40 %), the height in desktop points (10 is a text line). Desktop and phone:
/// radius 4 bars on `surface-2`, 8 pt apart, pulsing slowly (not under reduced motion).
/// Terminal: bands a whole number of rows high (one per 16 points), no pulse. Hidden from
/// screen readers, which hear the container's label "Loading"; test_id "skeleton".
pub fn skeleton(app: AppState, bars: &[(f32, f32)]) -> AnyElement {
    let form = app.form;
    let bars = bars.iter().map(|&(width, height)| {
        let bar = view()
            .w(relative(width.clamp(0.0, 1.0)))
            .bg(color(Token::Surface2))
            .accessibility_elements_hidden(true);
        if form == Form::Terminal {
            return bar
                .h(term::rows((height / 16.0).round().max(1.0)))
                .into_element();
        }
        let pulse = motion().entering(
            MotionSpec::new(KeyframeTiming::new(PULSE_MS).infinite())
                .values(MotionChannel::Opacity, [1.0, 0.55, 1.0]),
        );
        bar.h(px(height))
            .rounded(px(4.0))
            .bind_motion(&pulse)
            .into_element()
    });
    view()
        .test_id("skeleton")
        .label("Loading")
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(bars.collect::<Vec<_>>())
        .into_element()
}

/// The connection banner (`.conn-banner`): a strip across the top of the page saying how the
/// connection stands, with its actions. Desktop: padding 7 18, the tone's ground, a rule under
/// it, the actions 8 pt apart at the trailing edge. Phone: padding 10 18 and no actions, as the
/// mockup's phone banner (Settings › Connection is a push away). Terminal: the tone's mark, the
/// sentence, the actions, a joined rule under the row. test_id "conn-banner".
pub fn banner(app: AppState, hue: Hue, content: &Rich, actions: Vec<AnyElement>) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .test_id("conn-banner")
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .px(px(18.0))
            .py(px(7.0))
            .bg(hue_fill(hue))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(sentence(app, content))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(8.0))
                    .children(actions),
            )
            .into_element(),
        Form::Phone => view()
            .test_id("conn-banner")
            .flex_none()
            .px(px(18.0))
            .py(px(10.0))
            .bg(hue_fill(hue))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(sentence(app, content))
            .into_element(),
        Form::Terminal => view()
            .flex_none()
            .child(marked_row(app, hue, content, actions).test_id("conn-banner"))
            .child(term::hrule())
            .into_element(),
    }
}

/// The mark a terminal leads a tone with, shaped so it reads without colour: `▲` warn, `✗`
/// bad, `✓` ok, `●` needs you, `◐` verifying or producing, `•` plain.
pub fn tone_mark(hue: Hue) -> &'static str {
    match hue {
        Hue::Neutral => "•",
        Hue::You => "●",
        Hue::Corral | Hue::Worker => "◐",
        Hue::Held => "▲",
        Hue::Fail => "✗",
        Hue::Ok => "✓",
    }
}

/// The sentence, taking the room its row leaves: at the secondary size on desktop and phone
/// (12 and 13.5 pt, as `.notice` and the phone banner), in full ink in a terminal, where the
/// secondary ink fades.
fn sentence(app: AppState, content: &Rich) -> AnyElement {
    let words = if app.form == Form::Terminal {
        rich(app, content)
    } else {
        rich_small(app, content)
    };
    view().flex_1().min_w(px(0.0)).child(words).into_element()
}

/// A terminal row: the tone's mark in the tone and bold, the sentence, the actions two Cells
/// apart. Colour alone would not carry the tone on a terminal without colours.
fn marked_row(app: AppState, hue: Hue, content: &Rich, actions: Vec<AnyElement>) -> View {
    let ink = match hue {
        Hue::Neutral => color(Token::Ink),
        _ => hue_ink(hue),
    };
    let mut row = view()
        .flex_row()
        .items_start()
        .px(term::cells(1.0))
        .child(
            text(tone_mark(hue))
                .flex_shrink_0()
                .font_weight(FontWeight::BOLD)
                .text_color(ink),
        )
        .child(term::space(1.0))
        .child(sentence(app, content));
    for action in actions {
        row = row.child(term::space(2.0)).child(action);
    }
    row
}
