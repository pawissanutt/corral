//! The Ask composer and the surface it opens in: a drawer beside the content on desktop and in a
//! terminal, a bottom sheet on the phone. Nothing can be sent from this build, and the composer
//! says so beside its Send button instead of letting a message go nowhere.

use nsh_base::{SheetEvent, button, sheet, toggle, toggle_group};
use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::{AskTarget, Form, Overlay};
use crate::state::AppState;
use crate::ui::atoms::key_hint;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

const PLACEHOLDER: &str = "Ask a question, drop an idea, or give an instruction…";
/// The same words on two lines: a terminal draws a placeholder's lines as given and wraps none,
/// so on one line it would stop at the field's edge mid-sentence.
const PLACEHOLDER_LINES: &str = "Ask a question, drop an idea,\nor give an instruction…";
const REASON: &str = "Not connected to corrald. Nothing can be sent from this build.";

/// One field, the Assistant | PM switch, Send (always disabled in this build) and the reason line
/// "Not connected to corrald. Nothing can be sent from this build." Reads and writes app.ask_draft and
/// app.ask_target; focuses the field whenever app.ask_focus changes.
pub fn composer(app: AppState) -> AnyElement {
    let form = app.form;
    let field = element_ref();
    // Each opening of Ask bumps the counter. The field is made here, before it is mounted, so a
    // focus sent from this run would reach no element. `after_answer` waits for the commit after
    // the one that mounts it, which is also after the one where a dialog or sheet closing in the
    // same turn (the palette running Ask) gives the focus back to where it came from: this focus
    // lands last. The caret goes after any draft the opening brought.
    effect(move || {
        app.ask_focus.with(|_| ());
        after_answer(move || {
            let end = app
                .ask_draft
                .with_untracked(|draft| draft.encode_utf16().count());
            let end = u32::try_from(end).unwrap_or(u32::MAX);
            field.focus();
            field.set_selection(end, end);
        });
    });
    let shell = view().gap(px(match form {
        Form::Desktop => 8.0,
        Form::Phone => 12.0,
        Form::Terminal => 0.0,
    }));
    let shell = match form {
        Form::Desktop => shell
            .px(px(14.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule)),
        Form::Phone => shell.px(px(18.0)).pt(px(4.0)),
        // Line to line: the switch's and the field's bands start at the frame's lines, as the
        // tabs' do, and their text a Cell in, where the title's and the reason's start. Inside
        // the frame gaps take no rows, so the blank rows are boxes of their own.
        Form::Terminal => shell,
    };
    let terminal = form == Form::Terminal;
    shell
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(target_switch(app))
                .child(view().flex_1())
                .child(when(form == Form::Desktop, key_hints)),
        )
        .when(terminal, |s| s.child(term::blank()))
        .child(field_input(app, field))
        .when(terminal, |s| s.child(term::blank()))
        .child(
            view()
                .flex_row()
                // Send on the reason's last row, where the composer ends.
                .map(|row| {
                    if terminal {
                        row.items_end().px(px(8.0))
                    } else {
                        row.items_center()
                    }
                })
                .gap(px(if terminal { 16.0 } else { 12.0 }))
                .child(
                    text(REASON)
                        .test_id("ask-reason")
                        .flex_1()
                        .min_w(px(0.0))
                        .font_family(FONT_TEXT)
                        .text_size(px(small_size(form)))
                        .line_height(px(small_size(form) * 1.4))
                        .text_color(color(Token::Ink3)),
                )
                .child(send_button(form)),
        )
        .when(terminal, |s| s.child(term::blank()))
        .into_element()
}

/// The surface Ask opens in, shown while Overlay::Ask is open: on Form::Desktop a drawer on the right
/// edge of its parent (absolute, about 500 pt wide); on Form::Terminal half the content beside the
/// page on a wide grid, all of it on a narrow one; both with a title row "Ask" and a close button;
/// on Form::Phone a Sheet from the bottom. Closing it (button, backdrop, Esc, back) calls
/// app.close(Overlay::Ask).
pub fn ask_surface(app: AppState) -> AnyElement {
    match app.form {
        Form::Phone => sheet()
            .open(move || Some(app.is_open(Overlay::Ask)))
            .close_on_swipe(true)
            .on_open_change(move |_: &SheetEvent| app.close(Overlay::Ask))
            .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
            // Made on each opening, so the field and its focus request start fresh and nothing of
            // the composer lives while the sheet is closed. Built untracked: a signal the build
            // happens to read must not rebuild the open sheet and lose the draft's focus.
            .child(dynamic(move || untracked(|| phone_panel(app))))
            .into_element(),
        Form::Desktop | Form::Terminal => when(
            move || app.is_open(Overlay::Ask),
            move || untracked(|| drawer(app)),
        )
        .into_element(),
    }
}

/// The drawer over the right of the content; the content beside it stays visible and usable.
fn drawer(app: AppState) -> AnyElement {
    if app.form == Form::Terminal {
        return terminal_drawer(app);
    }
    let body = view()
        .flex_1()
        .child(title_row(app))
        .child(view().flex_1())
        .child(composer(app));
    // One short slide from the right edge, eased out; reduced motion skips it.
    let entrance = motion().entering(
        MotionSpec::new(KeyframeTiming::new(200.0).easing(Easing::Bezier(0.16, 1.0, 0.3, 1.0)))
            .values(MotionChannel::TranslateX, [24.0, 0.0])
            .values(MotionChannel::Opacity, [0.6, 1.0]),
    );
    view()
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .role(AccessibilityRole::Region)
        .label("Ask")
        .border_l(px(1.0))
        .border_l_color(color(Token::Rule2))
        .w(px(500.0))
        .bg(color(Token::Surface))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .bind_motion(&entrance)
        .child(body)
        .into_element()
}

/// Ask in a terminal: a pane of the frame around the content, its title row, the empty thread
/// and the composer ruled apart by lines that join the frame. On a wide grid it takes half the
/// content beside the page, split from it by a line of its own, so nothing of the page is cut
/// under it; on a narrow grid, whose page's words would not fit beside it, it takes the whole
/// frame while the shell sets the page aside.
fn terminal_drawer(app: AppState) -> AnyElement {
    let body = view()
        .flex_1()
        .min_w(px(0.0))
        .child(title_row(app))
        .child(term::hrule())
        .child(view().flex_1())
        .child(term::hrule())
        .child(composer(app));
    let pane = view()
        .role(AccessibilityRole::Region)
        .label("Ask")
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

/// The sheet's panel: a grab handle, the title row and the composer, riding on the software
/// keyboard and clear of the home indicator while the keyboard is down.
fn phone_panel(app: AppState) -> AnyElement {
    view()
        .pb_keyboard()
        .bg(color(Token::Surface))
        .rounded_t(px(14.0))
        .border_t(px(1.0))
        .border_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .role(AccessibilityRole::Region)
        .label("Ask")
        .child(
            view()
                .pb(move || {
                    px(14.0)
                        + if keyboard_height() > px(0.0) {
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
                        .accessibility_elements_hidden(true),
                )
                .child(title_row(app))
                .child(composer(app)),
        )
        .into_element()
}

/// "Ask" and the close button.
fn title_row(app: AppState) -> AnyElement {
    let form = app.form;
    let row = view().flex_row().items_center().gap(px(8.0));
    let row = match form {
        Form::Desktop => row
            .h(px(44.0))
            .pl(px(16.0))
            .pr(px(10.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule)),
        Form::Phone => row.pl(px(18.0)).pr(px(12.0)).pb(px(6.0)),
        Form::Terminal => row.px(px(8.0)).h(px(16.0)),
    };
    let title_size = match form {
        Form::Desktop => 14.0,
        Form::Phone | Form::Terminal => text_size(form),
    };
    row.child(
        text("Ask")
            .role(AccessibilityRole::Header)
            .flex_1()
            .font_family(FONT_TEXT)
            .text_size(px(title_size))
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink)),
    )
    .child(close_button(app))
    .into_element()
}

fn close_button(app: AppState) -> AnyElement {
    let form = app.form;
    let size = match form {
        Form::Desktop => 12.0,
        Form::Phone | Form::Terminal => text_size(form),
    };
    let close = view()
        .test_id("ask-close")
        .role(AccessibilityRole::Button)
        .label("Close Ask")
        .on_press(move || app.close(Overlay::Ask))
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .border_1()
        .border_color(AdaptiveColor::TRANSPARENT)
        .focus_visible(|s| s.border_color(color(Token::Focus)));
    let label = text("Close")
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        // Full ink in a terminal, where the fainter one fades and would read as disabled.
        .text_color(color(if form == Form::Terminal {
            Token::Ink
        } else {
            Token::Ink2
        }));
    match form {
        Form::Desktop => close
            .h(px(28.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .active(|s| s.bg(color(Token::Sel)))
            .child(label)
            .child(key_hint("Esc"))
            .into_element(),
        Form::Phone => close
            .min_h(px(44.0))
            .px(px(8.0))
            .rounded(px(10.0))
            .active(|s| s.bg(color(Token::Hover)))
            .child(label)
            .into_element(),
        // As the key line reads: the key's cap, then what it does.
        Form::Terminal => close
            .child(term::key_cap("Esc"))
            .child(label)
            .into_element(),
    }
}

/// Assistant | PM: one toggle pressed at a time, the group one stop of the keyboard focus with
/// the arrow keys moving between the two. In a terminal the pressed one is a band of its own
/// (NSH's look for a toggle that is on), so the group draws no fill or frame around it.
fn target_switch(app: AppState) -> AnyElement {
    let pad = if app.form == Form::Terminal { 0.0 } else { 2.0 };
    let group = toggle_group()
        .flex_row()
        .items_center()
        .gap(px(pad))
        .p(px(pad));
    let group = if app.form == Form::Terminal {
        group
    } else {
        group
            .rounded(px(if app.form == Form::Phone { 9.0 } else { 7.0 }))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule))
    };
    group
        .child(target_item(
            app,
            AskTarget::Assistant,
            "ask-target-assistant",
        ))
        .child(target_item(app, AskTarget::Pm, "ask-target-pm"))
        .into_element()
}

fn target_item(app: AppState, target: AskTarget, test_id: &'static str) -> AnyElement {
    let form = app.form;
    let on = move || app.ask_target.get() == target;
    let (height, size, radius) = match form {
        Form::Desktop => (24.0, 12.0, 5.0),
        Form::Phone => (36.0, 14.0, 7.0),
        Form::Terminal => (16.0, text_size(form), 0.0),
    };
    toggle()
        .pressed(on)
        // Choosing the pressed one again keeps it: the switch always names one target.
        .on_change(move |_: &bool| app.ask_target.set(target))
        .h(px(height))
        .px(px(if form == Form::Phone { 16.0 } else { 10.0 }))
        .rounded(px(radius))
        .items_center()
        .justify_center()
        .border_1()
        // In a terminal NSH bands a pressed toggle itself; a fill of the app's would be a second.
        .bg(move || {
            if on() && form != Form::Terminal {
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
        .focus_visible(|s| s.border_color(color(Token::Focus)))
        .child(
            // The pressable toggle is this view's parent, so a press found by the test id lands on
            // the toggle itself.
            view().test_id(test_id).child(
                text(target.label())
                    .font_family(FONT_TEXT)
                    .text_size(px(size))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(move || {
                        if on() {
                            color(Token::Ink)
                        } else {
                            color(Token::Ink3)
                        }
                    }),
            ),
        )
        .into_element()
}

/// The multiline field. Return inserts a line break and nothing submits it: this build sends
/// nothing, so no key may look like it did.
fn field_input(app: AppState, field: ElementRef) -> AnyElement {
    let form = app.form;
    let size = text_size(form);
    let entry = input()
        .element_ref(&field)
        .test_id("ask-input")
        .label("Ask")
        .multiline(true)
        .placeholder(if form == Form::Terminal {
            PLACEHOLDER_LINES
        } else {
            PLACEHOLDER
        })
        .value(move || app.ask_draft.get())
        .on_change(move |text: &str| app.ask_draft.set(text.to_owned()))
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink3)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(match form {
            Form::Phone => color(Token::Bg),
            Form::Desktop => color(Token::Surface),
            // NSH draws a field without a fill of its own as a raised band, and underlines it
            // where the terminal has no bands; a fill would turn the standard grey there, with
            // the placeholder dim on it.
            Form::Terminal => AdaptiveColor::TRANSPARENT,
        })
        .border_1()
        .border_color(color(Token::Rule2))
        .focus(|s| s.border_color(color(Token::Focus)));
    match form {
        Form::Desktop => entry
            .h(px(64.0))
            .px(px(12.0))
            .py(px(9.0))
            .line_height(px(19.0))
            .rounded(px(8.0))
            .into_element(),
        Form::Phone => entry
            .h(px(96.0))
            .px(px(12.0))
            .py(px(10.0))
            .line_height(px(23.0))
            .rounded(px(10.0))
            .into_element(),
        Form::Terminal => entry.h(px(48.0)).px(px(8.0)).into_element(),
    }
}

/// Send, disabled for presses, keys and screen readers alike (they hear it dimmed); the reason
/// line beside it says why.
fn send_button(form: Form) -> AnyElement {
    let (height, radius) = match form {
        Form::Desktop => (30.0, 6.0),
        Form::Phone => (44.0, 10.0),
        Form::Terminal => (16.0, 0.0),
    };
    button()
        .disabled(true)
        .h(px(height))
        .px(px(if form == Form::Phone { 18.0 } else { 14.0 }))
        .rounded(px(radius))
        .items_center()
        .justify_center()
        .bg(color(Token::Surface2))
        .border_1()
        .border_color(color(Token::Rule2))
        .child(
            view().test_id("ask-send").child(
                text("Send")
                    .font_family(FONT_TEXT)
                    .text_size(px(text_size(form)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(Token::Ink3)),
            ),
        )
        .into_element()
}

/// The keys the composer answers to on desktop.
fn key_hints() -> AnyElement {
    let hint = |words: &'static str| {
        text(words)
            .font_family(FONT_TEXT)
            .text_size(px(11.0))
            .text_color(color(Token::Ink3))
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(key_hint("Enter"))
        .child(hint("new line"))
        .child(view().w(px(6.0)))
        .child(key_hint("Esc"))
        .child(hint("close"))
        .into_element()
}

fn small_size(form: Form) -> f32 {
    match form {
        Form::Desktop => 12.0,
        Form::Phone => 14.0,
        Form::Terminal => text_size(form),
    }
}
