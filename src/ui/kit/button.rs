//! Buttons, the segmented control and the in-page tab strip, in the look of each form.
//!
//! Desktop follows `.btn` (30 pt high, 12.5 pt semibold, 6 pt corners), the phone `.pbtn`
//! (at least 44 pt high, 15 pt bold, 10 pt corners). A terminal draws a control as its label row
//! and bands and bolds the focused or pressed one itself, so there a button is its label on a
//! band of the app's (bold for primary), with no ring or fill of the app's for its states.
//! Every control here carries its role, its selected or pressed state for screen readers, a
//! focus ring on desktop and phone, and a test id on the view around its label (the press
//! targets of `nsh_base` take none themselves; a test press there reaches the control).

use std::rc::Rc;

use nsh_base::{button as press_target, tab, tabs, toggle, toggle_group};
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::term::{cells, key_cap};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// The minimum touch target on a phone.
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;

/// What a button weighs (`.btn`, `.btn.ghost`, `.btn.quiet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The one main action: filled in the neutral button ink (`--btn`), never a state colour.
    Primary,
    /// A second action: outlined in rule-2, ink words.
    Ghost,
    /// A minor action: no fill or outline, ink-2 words.
    Quiet,
}

/// The variants of [`button_with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ButtonOptions {
    /// A leading icon in the words' colour (desktop 14 pt, phone 18 pt; none in a terminal).
    pub icon: Option<Icon>,
    /// No presses, faded to 45 % (the terminal's own faint look), announced as dimmed.
    pub disabled: bool,
    /// Takes the room left in its row (`.pbtn.wide`): the decide bar's buttons.
    pub wide: bool,
    /// A destructive action (`.btn.danger`): no fill, the words in the failure colour inside a
    /// half-strength failure outline, whatever the kind. Terminal: failure-coloured words on
    /// the kind's band.
    pub danger: bool,
    /// Only the icon shows, in a square target (desktop 30 pt, phone 44 pt); the label becomes
    /// what screen readers say. Needs `icon`; without one the label shows as usual. A terminal
    /// shows no images, so there the label shows, as NSH shows an icon button's label.
    pub icon_only: bool,
    /// A key cap after the label (`.btn .kbd`), drawn in the words' colour: Cancel `Esc`.
    /// Desktop shows it after the label, a terminal before it as its key line reads; a phone,
    /// with no keyboard to press it on, leaves it out.
    pub key_hint: Option<&'static str>,
}

/// A button with the form's metrics: desktop 30 pt high, 12 pt padding; phone at least 44 pt
/// high, 16 pt padding; terminal one row. `test_id` names the view around its label.
pub fn button(
    app: AppState,
    label: impl Into<String>,
    kind: ButtonKind,
    test_id: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    button_with(
        app,
        label,
        kind,
        ButtonOptions::default(),
        test_id,
        on_press,
    )
}

/// [`button`] with a leading icon, a disabled state or the full width of its row.
pub fn button_with(
    app: AppState,
    label: impl Into<String>,
    kind: ButtonKind,
    options: ButtonOptions,
    test_id: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let label: String = label.into();
    let test_id: String = test_id.into();
    let look = Look::of(form, kind, options.danger);
    let glyph = options
        .icon
        .zip(look.icon)
        .map(|(glyph, size)| icon(glyph, size, look.ink));
    let bare = options.icon_only && glyph.is_some();
    let words = (!bare).then(|| {
        text(label.clone())
            .number_of_lines(1u32)
            .text_color(look.ink)
            .when(look.bold, |t| t.font_weight(FontWeight::BOLD))
            .when(form != Form::Terminal, |t| {
                t.font_family(FONT_TEXT)
                    .text_size(px(look.size))
                    .font_weight(if form == Form::Phone {
                        FontWeight::BOLD
                    } else {
                        FontWeight::SEMIBOLD
                    })
            })
    });
    let (cap_before, cap_after) = match (form, options.key_hint) {
        (Form::Terminal, Some(keys)) => (Some(key_cap(keys)), None),
        (Form::Desktop, Some(keys)) => (None, Some(desktop_cap(keys, look.ink))),
        _ => (None, None),
    };
    let face = view()
        .test_id(&test_id)
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .children(cap_before)
        .children(glyph)
        .children(words)
        .children(cap_after);
    let target = press_target()
        .disabled(options.disabled)
        .on_press(move |_: &()| on_press())
        .when(bare, |b| b.accessibility_label(label))
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .bg(look.fill)
        .cursor_pointer()
        .disabled_style(|s| s.opacity(0.45))
        .when(options.wide, |b| b.flex_1().min_w(px(0.0)))
        .when_some(look.outline, |b, outline| {
            b.border_1().border_color(outline)
        });
    let target = match form {
        Form::Terminal => target.px(look.pad).h(px(16.0)),
        Form::Desktop => target
            .h(px(30.0))
            .when(bare, |b| b.w(px(30.0)))
            .when(!bare, |b| b.px(look.pad))
            .rounded(px(6.0))
            .hover(look.hover)
            .active(look.active)
            .focus_visible(ring),
        Form::Phone => target
            .min_h(px(TARGET))
            .when(bare, |b| b.w(px(TARGET)))
            .when(!bare, |b| b.px(look.pad))
            .rounded(px(10.0))
            .active(look.active)
            .focus_visible(ring),
    };
    target.child(face).into_element()
}

/// A key cap inside a desktop button (`.btn .kbd`): the mono key in the words' colour at 80 %,
/// framed by a line of that colour at 35 %, so it reads on a filled or an outlined button alike.
fn desktop_cap(keys: &'static str, ink: AdaptiveColor) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .h(px(16.0))
        .px(px(4.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(ink.opacity(0.35))
        .opacity(0.8)
        .accessibility_elements_hidden(true)
        .child(
            text(keys)
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .text_size(px(10.5))
                .text_color(ink),
        )
        .into_element()
}

/// The keyboard focus ring: 2 pt of the focus colour, 2 pt out (`:focus-visible`).
fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(RING))
        .outline_color(color(Token::Focus))
        .outline_offset(px(RING))
}

/// The colours and metrics of a button kind on a form.
struct Look {
    fill: AdaptiveColor,
    ink: AdaptiveColor,
    outline: Option<AdaptiveColor>,
    bold: bool,
    size: f32,
    pad: Points,
    /// The leading icon's size; `None` in a terminal, which shows no images.
    icon: Option<f32>,
    hover: fn(StyleRefinement) -> StyleRefinement,
    active: fn(StyleRefinement) -> StyleRefinement,
}

impl Look {
    /// The kind's look, or over it the danger look: no fill, failure words inside a failure
    /// outline at half strength (a terminal keeps the kind's band and draws no outline).
    fn of(form: Form, kind: ButtonKind, danger: bool) -> Look {
        let look = Look::of_kind(form, kind);
        if !danger {
            return look;
        }
        let fail = color(Token::Fail);
        if form == Form::Terminal {
            return Look { ink: fail, ..look };
        }
        Look {
            fill: AdaptiveColor::TRANSPARENT,
            ink: fail,
            outline: Some(fail.opacity(0.5)),
            hover: |s| s.bg(color(Token::FailBg)),
            active: |s| s.bg(color(Token::FailBg)).opacity(0.85),
            ..look
        }
    }

    fn of_kind(form: Form, kind: ButtonKind) -> Look {
        let (size, icon_size, pad) = match form {
            Form::Desktop => (
                12.5,
                Some(14.0),
                px(if kind == ButtonKind::Quiet { 8.0 } else { 12.0 }),
            ),
            Form::Phone => (15.0, Some(18.0), px(16.0)),
            Form::Terminal => (text_size(form), None, cells(1.0)),
        };
        if form == Form::Terminal {
            // A band lighter than the page in both appearances, as a key cap is; ink words,
            // since neutral inks fainter than ink fade there and would read as disabled.
            return Look {
                fill: match kind {
                    ButtonKind::Primary => color(Token::Surface2),
                    ButtonKind::Ghost => color(Token::Surface),
                    ButtonKind::Quiet => AdaptiveColor::TRANSPARENT,
                },
                ink: color(Token::Ink),
                outline: None,
                bold: kind == ButtonKind::Primary,
                size,
                pad,
                icon: None,
                hover: keep,
                active: keep,
            };
        }
        match kind {
            ButtonKind::Primary => Look {
                fill: color(Token::Btn),
                ink: color(Token::BtnInk),
                outline: None,
                bold: false,
                size,
                pad,
                icon: icon_size,
                hover: |s| s.opacity(0.92),
                active: |s| s.opacity(0.8),
            },
            ButtonKind::Ghost => Look {
                fill: AdaptiveColor::TRANSPARENT,
                ink: color(Token::Ink),
                outline: Some(color(Token::Rule2)),
                bold: false,
                size,
                pad,
                icon: icon_size,
                hover: |s| s.bg(color(Token::Hover)),
                active: |s| s.bg(color(Token::Sel)),
            },
            ButtonKind::Quiet => Look {
                fill: AdaptiveColor::TRANSPARENT,
                ink: color(Token::Ink2),
                outline: None,
                bold: false,
                size,
                pad,
                icon: icon_size,
                hover: |s| s.bg(color(Token::Hover)),
                active: |s| s.bg(color(Token::Sel)),
            },
        }
    }
}

fn keep(s: StyleRefinement) -> StyleRefinement {
    s
}

/// A segmented control: one option pressed at a time (`.seg` on desktop, `.pseg` on a phone).
/// `selected` names the pressed option; pressing another calls `on_pick` with its value (the
/// pressed one stays pressed). Desktop: a sunk track with 2 pt padding, options 24 pt high in
/// 12 pt, the pressed one raised on the surface with a rule-2 edge in ink. Phone: the track
/// spans its row, options share it equally, 32 pt high (44 pt targets), 13.5 pt, the pressed one
/// bold. Terminal: the options in a row; NSH bands the pressed one itself. A toolbar for screen
/// readers, each option a toggle button with its pressed state; the arrow keys move between
/// them. Option test ids are `<test_id>-<index>`.
pub fn seg<T: Copy + PartialEq + 'static>(
    app: AppState,
    options: &[(T, &'static str)],
    selected: impl Fn() -> T + 'static,
    on_pick: impl Fn(T) + 'static,
    test_id: &str,
) -> AnyElement {
    let form = app.form;
    let current = computed(selected);
    let pick: Rc<dyn Fn(T)> = Rc::new(on_pick);
    let group = toggle_group().flex_row().items_center();
    let group = match form {
        Form::Terminal => group.gap(px(8.0)),
        Form::Desktop => group
            .self_start()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(7.0))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule)),
        Form::Phone => group
            .self_stretch()
            .gap(px(3.0))
            .p(px(3.0))
            .rounded(px(9.0))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule)),
    };
    group
        .children(options.iter().enumerate().map(|(index, &(value, label))| {
            let pick = Rc::clone(&pick);
            let on = move || current.get() == value;
            seg_option(form, label, format!("{test_id}-{index}"), on, move || {
                pick(value)
            })
        }))
        .into_element()
}

fn seg_option(
    form: Form,
    label: &'static str,
    test_id: String,
    on: impl Fn() -> bool + Copy + 'static,
    pick: impl Fn() + 'static,
) -> AnyElement {
    let (height, size, radius, pad) = match form {
        Form::Desktop => (24.0, 12.0, 5.0, 10.0),
        Form::Phone => (32.0, 13.5, 7.0, 12.0),
        Form::Terminal => (16.0, text_size(form), 0.0, 8.0),
    };
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(move || {
            if on() || form == Form::Terminal {
                color(Token::Ink)
            } else {
                color(Token::Ink3)
            }
        })
        .style_when(on, |s| s.font_weight(FontWeight::BOLD))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT).text_size(px(size))
        });
    toggle()
        .pressed(on)
        // Choosing the pressed one again keeps it: the control always names one option.
        .on_change(move |_: &bool| pick())
        .h(px(height))
        .px(px(pad))
        .rounded(px(radius))
        .items_center()
        .justify_center()
        .cursor_pointer()
        // The 32 pt option reaches the 44 pt target above and below, never into its neighbours.
        .when(form == Form::Phone, |t| {
            let slop = px((TARGET - height) / 2.0);
            t.flex_1().min_w(px(0.0)).hit_slop(EdgeInsets {
                top: slop,
                right: px(0.0),
                bottom: slop,
                left: px(0.0),
            })
        })
        // In a terminal NSH bands a pressed toggle itself; a fill of the app's would be a second.
        .when(form != Form::Terminal, |t| {
            t.border_1()
                .bg(move || {
                    if on() {
                        color(Token::Surface)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .border_color(move || {
                    if on() {
                        color(Token::Rule2)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(ring)
        })
        .child(view().test_id(&test_id).child(words))
        .into_element()
}

/// The tabs of a page (`.tabs`): the Story, Evidence, Session of an inspector; the evidence
/// tabs of a question. `selected` names the current one; pressing a tab calls `on_pick` with its
/// value. Desktop: 13 pt labels in ink-3, the selected one in ink under a 2 pt ink underline that
/// travels between tabs, a hairline under the strip. Phone: 15 pt, 44 pt targets. Terminal: the
/// tabs in a row two Cells apart, NSH bolding the selected one. A tab list for screen readers,
/// each a tab with its selected state; the arrow keys move between them. Tab test ids are
/// `<test_id>-<index>`.
pub fn tab_strip<T: Copy + PartialEq + 'static>(
    app: AppState,
    options: &[(T, &'static str)],
    selected: impl Fn() -> T + 'static,
    on_pick: impl Fn(T) + 'static,
    test_id: &str,
) -> AnyElement {
    let form = app.form;
    let current = computed(selected);
    let pick: Rc<dyn Fn(T)> = Rc::new(on_pick);
    let strip = tabs().flex_row().items_end();
    let strip = match form {
        Form::Terminal => strip.gap(px(16.0)),
        Form::Desktop | Form::Phone => strip
            .gap(px(2.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .indicator_style(|s| {
                s.bottom(px(-1.0))
                    .h(px(2.0))
                    .mx(px(8.0))
                    .rounded(px(2.0))
                    .bg(color(Token::Ink))
            }),
    };
    strip
        .children(options.iter().enumerate().map(|(index, &(value, label))| {
            let pick = Rc::clone(&pick);
            let on = move || current.get() == value;
            strip_tab(form, label, format!("{test_id}-{index}"), on, move || {
                pick(value)
            })
        }))
        .into_element()
}

fn strip_tab(
    form: Form,
    label: &'static str,
    test_id: String,
    on: impl Fn() -> bool + Copy + 'static,
    pick: impl Fn() + 'static,
) -> AnyElement {
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(move || {
            if on() || form == Form::Terminal {
                color(Token::Ink)
            } else {
                color(Token::Ink3)
            }
        })
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone {
                    15.0
                } else {
                    text_size(form)
                }))
                .font_weight(FontWeight::MEDIUM)
        })
        // Weight marks the selection for those who cannot tell the inks apart.
        .style_when(on, |s| s.font_weight(FontWeight::SEMIBOLD));
    let item = tab()
        .selected(on)
        .on_press(move |_: &()| pick())
        .cursor_pointer()
        .items_center()
        .justify_center();
    let item = match form {
        Form::Terminal => item,
        Form::Desktop => item
            .pt(px(7.0))
            .pb(px(8.0))
            .px(px(10.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring),
        Form::Phone => item
            .min_h(px(TARGET))
            .px(px(12.0))
            .rounded(px(6.0))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring),
    };
    item.child(view().test_id(&test_id).child(words))
        .into_element()
}
