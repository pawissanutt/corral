//! The small pieces every shell shares: key caps, counts and navigation rows. The health line is
//! `ui::health_line`.
//!
//! The shell surfaces (desktop title bar and sidebar, phone health strip) stay dark in both
//! appearances while pages follow the appearance. Colours drawn on the shell therefore take the
//! dark value of a tone in both appearances ([`on_shell`]); the light values are tuned for light
//! pages and fall under AA contrast on the dark shell. Counts and key caps carry their own fill,
//! because the same atom sits on the shell in the sidebar and on a page in the content.

use nsh_core::AccessibilityCurrent;
use nsh_core::element::Idiom;
use nsh_core::prelude::*;

use crate::model::{Destination, Form};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::hint::tip;
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_MONO, Token, color, row_height, text_size};

const FOCUS_RING: f32 = 2.0;

/// The touch target's floor (Apple's and the mock's 44 pt).
pub const TOUCH_TARGET: f32 = 44.0;

/// The desktop shell on a touch tablet (lib.rs gives a large tablet the desktop shell): there is
/// no pointer to aim finely, so its controls take the touch target's height.
pub fn touch_shell() -> bool {
    untracked(idiom) == Idiom::Pad
}

/// A control's height: `height` with a pointer, at least the touch target on a tablet.
pub fn control_height(height: f32) -> f32 {
    if touch_shell() {
        height.max(TOUCH_TARGET)
    } else {
        height
    }
}

/// A key cap in the mono font: key_hint("/"), key_hint("Ctrl K").
pub fn key_hint(keys: &'static str) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .h(px(16.0))
        .px(px(4.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(color(Token::BgSunk))
        .child(
            text(keys)
                .font_family(FONT_MONO)
                .text_size(px(10.5))
                .text_color(color(Token::Ink3))
                .number_of_lines(1u32),
        )
        .into_element()
}

/// The destination's count as a bare figure, its words as its name and its hint; nothing when it
/// has none. A needs-you count in the needs-you colour. test_id "count-<slug>".
pub fn count(app: AppState, destination: Destination) -> AnyElement {
    let form = app.form;
    let test_id = format!("count-{}", destination.slug());
    dynamic(move || {
        let snapshot = app.snapshot();
        snapshot
            .count(destination)
            .zip(snapshot.count_words(destination))
            .map(|(n, words)| count_chip(form, n, snapshot.needs_you(destination), words, &test_id))
    })
}

/// What a destination's row says to assistive technology: its name, then its count as the
/// count reads ("Inbox, 5 wait on you"), as the figure beside the name is part of what it shows.
pub fn destination_spoken(app: AppState, destination: Destination) -> impl Fn() -> SharedString {
    move || match app.snapshot().count_words(destination) {
        Some(words) => format!("{}, {words}", destination.label()).into(),
        None => destination.label().into(),
    }
}

/// A pressable row for lists of destinations and sections: label, optional trailing element,
/// selected look, role button, the given test id. Height `row_height(app.form)`. The selected
/// row is the current page for assistive technology.
pub fn nav_row(
    app: AppState,
    label: impl Into<SharedString>,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    trailing: Option<AnyElement>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let label: SharedString = label.into();
    let spoken = {
        let label = label.clone();
        move || label.clone()
    };
    row(
        app, None, label, spoken, test_id, selected, trailing, on_press,
    )
}

/// nav_row with an optional leading icon in the label's colour, named by `spoken` for assistive
/// technology ([`destination_spoken`] for a destination with a count).
#[allow(clippy::too_many_arguments)]
pub fn nav_row_icon(
    app: AppState,
    icon: Option<Icon>,
    label: impl Into<SharedString>,
    spoken: impl Fn() -> SharedString + 'static,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    trailing: Option<AnyElement>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    row(
        app,
        icon,
        label.into(),
        spoken,
        test_id,
        selected,
        trailing,
        on_press,
    )
}

/// The body of [`nav_row`] and [`nav_row_icon`]. The terminal shows no images, so a glyph is
/// left out there; its rows lead with a bar on the selected one instead, which reads at every
/// colour depth.
#[allow(clippy::too_many_arguments)]
fn row(
    app: AppState,
    glyph: Option<Icon>,
    label: SharedString,
    spoken: impl Fn() -> SharedString + 'static,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    trailing: Option<AnyElement>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let look = RowLook::of(form);
    let selected = computed(selected);
    let ink = move || {
        if selected.get() {
            look.ink_selected
        } else {
            look.ink
        }
    };
    let leading = match form {
        // A column of its own, so the labels line up whether a row is selected or not.
        Form::Terminal => Some(
            view()
                .w(cells(1.0))
                .flex_shrink_0()
                .accessibility_elements_hidden(true)
                .child(
                    text(move || if selected.get() { "▌" } else { " " })
                        .text_color(color(Token::Ink)),
                )
                .into_element(),
        ),
        Form::Desktop | Form::Phone => glyph
            .zip(look.icon)
            .map(|(glyph, size)| dynamic(move || icon(glyph, size, ink()))),
    };
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(look.gap))
        .h(px(control_height(row_height(form))))
        .px(px(look.pad))
        .rounded(px(look.radius))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(spoken)
        // `current` is what a screen reader announces for the page shown (`aria-current`);
        // `selected` stays for the terminal, which bands the selected row from it.
        .accessibility_state(move || AccessibilityState {
            selected: selected.get(),
            current: selected.get().then_some(AccessibilityCurrent::Page),
            ..AccessibilityState::default()
        })
        .test_id(&test_id)
        .style_when(selected, |s| s.bg(look.selected_bg))
        .hover(|s| s.bg(look.hover))
        .active(|s| s.bg(look.selected_bg))
        .when_some(look.focus, |row, focus| {
            row.focus_visible(|s| {
                s.outline(px(FOCUS_RING))
                    .outline_color(focus)
                    .outline_offset(px(-FOCUS_RING))
            })
        })
        .on_press(move || on_press())
        .children(leading)
        .child(
            text(label)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(text_size(form)))
                .text_color(ink)
                // Weight marks the selection for those who cannot tell the fills apart.
                .style_when(selected, |s| s.font_weight(FontWeight::SEMIBOLD)),
        )
        .children(trailing)
        .into_element()
}

/// The colours and metrics of a navigation row on the surface its form lists rows on: the shell
/// sidebar on desktop, the page on phone, a framed sidebar in the terminal, whose labels are
/// full ink: a fainter ink fades there and would read as disabled.
#[derive(Clone, Copy)]
struct RowLook {
    ink: AdaptiveColor,
    ink_selected: AdaptiveColor,
    selected_bg: AdaptiveColor,
    hover: AdaptiveColor,
    /// `None` in the terminal, which bands and bolds the focused control itself.
    focus: Option<AdaptiveColor>,
    /// The leading icon's size; `None` in the terminal, which shows no images.
    icon: Option<f32>,
    pad: f32,
    gap: f32,
    radius: f32,
}

impl RowLook {
    fn of(form: Form) -> RowLook {
        match form {
            Form::Desktop => RowLook {
                ink: color(Token::ShellInk2),
                ink_selected: color(Token::ShellInk),
                selected_bg: color(Token::ShellSel),
                hover: color(Token::ShellSel).opacity(0.6),
                focus: Some(on_shell(Token::Focus)),
                icon: Some(16.0),
                pad: 8.0,
                gap: 10.0,
                radius: 6.0,
            },
            Form::Phone => RowLook {
                ink: color(Token::Ink),
                ink_selected: color(Token::Ink),
                selected_bg: color(Token::Sel),
                hover: color(Token::Hover),
                focus: Some(color(Token::Focus)),
                icon: Some(22.0),
                pad: 16.0,
                gap: 12.0,
                radius: 10.0,
            },
            // NSH draws a selected, hovered or pressed row's band itself: an app fill would be
            // a second one, and on a terminal without bands the standard grey the reversed
            // selection then draws its text in.
            Form::Terminal => RowLook {
                ink: color(Token::Ink),
                ink_selected: color(Token::Ink),
                selected_bg: AdaptiveColor::TRANSPARENT,
                hover: AdaptiveColor::TRANSPARENT,
                focus: None,
                icon: None,
                pad: 0.0,
                gap: 8.0,
                radius: 0.0,
            },
        }
    }
}

/// The token's dark value in both appearances, for marks drawn on the always-dark shell.
pub(crate) fn on_shell(token: Token) -> AdaptiveColor {
    let dark = color(token).dark;
    AdaptiveColor { light: dark, dark }
}

/// A count, named by its `words`, which are also its hint on desktop and in a terminal. On the
/// phone a pill of its own fill: the Inbox count (what needs you) a solid needs-you pill in bold,
/// the others quiet. The desktop sidebar's counts are bare figures on the shell, as the mock's;
/// a terminal's are figures in a column: a column of small fills reads as one band there.
fn count_chip(form: Form, n: u32, needs_you: bool, words: String, test_id: &str) -> AnyElement {
    if form == Form::Terminal {
        let figure = text(n.to_string())
            .test_id(test_id)
            .label(words.clone())
            .flex_shrink_0()
            .text_color(color(if needs_you { Token::You } else { Token::Ink }))
            .when(needs_you, |t| t.font_weight(FontWeight::BOLD));
        return tip(form, figure, words.into()).into_element();
    }
    if form == Form::Desktop {
        // The sidebar's `.nav .cnt`: a figure on the shell, the needs-you one bold in its colour.
        let figure = text(n.to_string())
            .test_id(test_id)
            .label(words.clone())
            .flex_shrink_0()
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(11.0))
            .text_color(if needs_you {
                on_shell(Token::You)
            } else {
                color(Token::ShellInk2)
            })
            .when(needs_you, |t| t.font_weight(FontWeight::BOLD));
        return tip(form, figure, words.into()).into_element();
    }
    let (fill, ink, weight) = if needs_you {
        (Token::You, Token::BtnInk, FontWeight::BOLD)
    } else {
        (Token::Surface2, Token::Ink2, FontWeight::MEDIUM)
    };
    let (size, height, pad) = (13.0, 22.0, 7.0);
    view()
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .h(px(height))
        .min_w(px(height))
        .px(px(pad))
        .rounded(px(height / 2.0))
        .bg(color(fill))
        .child(
            text(n.to_string())
                .test_id(test_id)
                .label(words)
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(size))
                .font_weight(weight)
                .text_color(color(ink)),
        )
        .into_element()
}
