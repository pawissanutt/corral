//! What shows over a page for a moment: the toast, and the modal host that carries both the
//! fresh confirmation of a high-risk command and the phone's generic sheet (a thread, a pairing
//! code). Both are hosts: they take when to show (`open`, the toast's text) and what closing
//! does (`on_close`); the shell wires them to the app state.

use std::rc::Rc;
use std::time::Duration;

use nsh_base::{SheetEvent, dialog, sheet};
use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// How long a toast stays, as the mockup's `toast()`.
const TOAST_MS: u64 = 3400;
/// The desktop dialog's width and the room it keeps from the window's top and bottom.
const DIALOG_WIDTH: f32 = 520.0;
const DIALOG_MARGIN: f32 = 48.0;
/// The phone sheet's largest share of the screen (`.psheet`), and what its grab handle and
/// title row take of it.
const SHEET_SHARE: f32 = 0.82;
const SHEET_HEAD: f32 = 72.0;

/// What a modal shows. Made afresh on each opening by the `content` closure of [`modal`].
pub struct Modal {
    /// On the panel; its controls take it as a prefix (`<id>-done`, `-cancel`, `-confirm`,
    /// `-word`).
    pub test_id: String,
    /// The title, also the dialog's accessible name: "Approve candidate 85b45e1e".
    pub title: String,
    /// The line under the title on desktop and in a terminal, and the dialog's accessible
    /// description: "Confirm on this device. The daemon checks …". The phone leaves it out.
    pub sub: Option<String>,
    /// The content: the effect preview, a thread, a pairing code.
    pub body: AnyElement,
    /// The label of a plain close button in the title row ("Done"), for sheets that only show
    /// something.
    pub done: Option<String>,
    /// The confirmation, for a command that needs a fresh one on this device.
    pub confirm: Option<Confirm>,
}

/// The confirming part of a [`Modal`]. Its wording comes from the model, resolved for the form.
pub struct Confirm {
    /// The confirming button: "Confirm approve" on desktop, "Confirm" on the phone.
    pub label: String,
    /// The button that closes without confirming: "Cancel".
    pub cancel: String,
    /// The word a terminal asks to be typed: the command's first word, "approve".
    pub word: String,
    /// The phone's line over the buttons saying what Confirm checks (the model's: the tap, then
    /// corrald's check of this device and the preview).
    pub check: Option<String>,
    /// Runs the command. It does not close the modal: the confirmation's state leaving the
    /// app state does.
    pub on_confirm: Rc<dyn Fn()>,
}

/// The toast: a line of news that leaves by itself after 3.4 s (calling `on_close`), or at
/// once when `message` changes to another or to `None`. Screen readers hear it as it appears.
/// Desktop: bottom centre of its parent, 22 pt up, on the button colour (`.toast`: padding
/// 9 14, radius 8, 12.5 pt), at most 560 pt wide and wrapping to three lines, rising in over
/// 220 ms; put it last in the content container.
/// Phone: under the top safe area, clear of the docked composer, the floating Ask and the
/// keyboard that a bottom toast would cover, at most 340 pt wide and wrapping; it takes no
/// presses, so the page head under it stays usable. Terminal: a row in the flow on a raised
/// band, the message in bold; put it in the shell's column just above the key line, where it
/// takes a row only while it shows.
/// test_id "toast".
pub fn toast(
    app: AppState,
    message: impl Fn() -> Option<String> + 'static,
    on_close: impl Fn() + 'static,
) -> AnyElement {
    let on_close: Rc<dyn Fn()> = Rc::new(on_close);
    let shown = computed(message);
    dynamic(move || {
        let message = shown.get()?;
        let close = on_close.clone();
        // Made under this run: a new message or none ends it with the run, so an older toast's
        // timer never takes a newer one away.
        set_timeout(Duration::from_millis(TOAST_MS), move || close());
        announce_for_accessibility(message.clone());
        Some(untracked(move || toast_face(app, message)))
    })
}

/// The modal host, shown while `open` holds. Escape, the scrim, the phone's swipe and its back
/// call `on_close`. Desktop: a dialog over the window on a dimmed scrim, the panel centred
/// (520 pt, padding 18 20 16, radius 12, the title 15 pt bold, the sub line 12.5 pt quiet, the
/// body scrolling when it runs long), then Cancel and the confirming button at the trailing
/// edge, the keyboard focus on Cancel once it shows: Enter or Space cancels, Tab reaches the
/// confirming button. Phone: a bottom sheet (`.psheet`: radius 14 at the top, a grab handle,
/// the title row 16 pt bold with Done at its end, the body scrolling within 82 % of the
/// screen); a confirmation adds what Confirm checks, then Cancel (focused, for a hardware
/// keyboard) and Confirm side by side, full width. Terminal: a framed region over the content
/// pane, the title and its keys in the first
/// row, the sub line, the body between joined rules; a confirmation asks for its word to be
/// typed ("Type approve to confirm") and confirms on Enter when it matches. Role alert dialog
/// with a confirmation, dialog without; named by the title, described by the sub line.
pub fn modal(
    app: AppState,
    open: impl Fn() -> bool + 'static,
    on_close: impl Fn() + 'static,
    content: impl Fn() -> Modal + 'static,
) -> AnyElement {
    let on_close: Rc<dyn Fn()> = Rc::new(on_close);
    match app.form {
        Form::Phone => {
            let close = on_close.clone();
            sheet()
                .open(move || Some(open()))
                .close_on_swipe(true)
                .on_open_change(move |_: &SheetEvent| close())
                .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
                // Made on each opening and untracked, as the Ask sheet is: a signal the content
                // reads must not rebuild the open sheet.
                .child(dynamic(move || {
                    let on_close = on_close.clone();
                    untracked(|| phone_sheet(app, content(), on_close))
                }))
                .into_element()
        }
        Form::Desktop | Form::Terminal => when(open, move || {
            let on_close = on_close.clone();
            untracked(|| {
                if app.form == Form::Terminal {
                    terminal_dialog(app, content(), on_close)
                } else {
                    desktop_dialog(app, content(), on_close)
                }
            })
        })
        .into_element(),
    }
}

/// The toast's face for the form.
fn toast_face(app: AppState, message: String) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view()
            .test_id("toast")
            .flex_row()
            .flex_none()
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .bg(color(Token::Surface))
            .child(
                text(message)
                    .number_of_lines(1u32)
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .into_element();
    }
    let phone = form == Form::Phone;
    // The phone's drops in from the top edge it sits under; the desktop's rises from below.
    let rise = motion().entering(
        MotionSpec::new(KeyframeTiming::new(220.0).easing(Easing::Bezier(0.16, 1.0, 0.3, 1.0)))
            .values(
                MotionChannel::TranslateY,
                [if phone { -8.0 } else { 8.0 }, 0.0],
            )
            .values(MotionChannel::Opacity, [0.0, 1.0]),
    );
    let face = view()
        .test_id("toast")
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(14.0))
        .py(px(9.0))
        .rounded(px(8.0))
        .bg(color(Token::Btn))
        .shadow_lg()
        .shadow_color(color(Token::Shell))
        .bind_motion(&rise)
        .child(
            text(message)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .line_height(px(18.0))
                .text_color(color(Token::BtnInk))
                .when(!phone, |t| t.number_of_lines(3u32)),
        );
    // Bounded on every form: a long receipt wraps inside the toast, never past the window's
    // edges; three desktop lines hold a refusal, and the page keeps the full status.
    let face = face.max_w(px(if phone { 340.0 } else { 560.0 }));
    // The strip spans its parent to centre the toast; it takes no presses of its own.
    let strip = view()
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .px(px(16.0))
        .items_center();
    let strip = if phone {
        strip
            .top(move || px(8.0) + safe_area().top)
            .pointer_events(PointerEvents::None)
    } else {
        strip
            .bottom(px(22.0))
            .pointer_events(PointerEvents::BoxNone)
    };
    strip.child(face).into_element()
}

/// The desktop dialog: scrim, centred panel, actions at the trailing edge.
fn desktop_dialog(app: AppState, modal: Modal, on_close: Rc<dyn Fn()>) -> AnyElement {
    let Modal {
        test_id,
        title,
        sub,
        body,
        done,
        confirm,
    } = modal;
    let role = role_of(confirm.is_some());
    let title = SharedString::from(title);
    let close = on_close.clone();
    let scrim = view()
        .absolute()
        .inset_0()
        .focusable(false)
        .accessibility_elements_hidden(true)
        .bg(color(Token::Shell).opacity(0.45))
        .on_press(move || close());
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .child(
            text(title.clone())
                .role(AccessibilityRole::Header)
                .flex_1()
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(15.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .children(done.map(|label| close_button(app, label, &test_id, on_close.clone())));
    let cancel = element_ref();
    let actions = confirm.map(|confirm| {
        let close = on_close.clone();
        let go = confirm.on_confirm.clone();
        // Cancel is the safe default; once the dialog is laid out the keyboard starts there.
        after_answer(move || cancel.focus());
        view()
            .flex_row()
            .justify_end()
            .gap(px(8.0))
            .mt(px(16.0))
            .child(button_with(
                app,
                confirm.cancel,
                ButtonKind::Ghost,
                ButtonOptions {
                    focus: Some(cancel),
                    ..ButtonOptions::default()
                },
                format!("{test_id}-cancel"),
                move || close(),
            ))
            .child(button(
                app,
                confirm.label,
                ButtonKind::Primary,
                format!("{test_id}-confirm"),
                move || go(),
            ))
    });
    let panel = view()
        .test_id(&test_id)
        .w(px(DIALOG_WIDTH))
        .max_h(move || px((window().height.0 - 2.0 * DIALOG_MARGIN).max(160.0)))
        .px(px(20.0))
        .pt(px(18.0))
        .pb(px(16.0))
        .bg(color(Token::Surface))
        .rounded(px(12.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .child(head)
        .when_some(sub.clone(), |panel, sub| {
            panel.child(
                text(sub)
                    .mt(px(2.0))
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(color(Token::Ink3)),
            )
        })
        .child(scroll().flex_auto().min_h(px(0.0)).mt(px(10.0)).child(body))
        .children(actions);
    dialog()
        .role(role)
        .label(title)
        .when_some(sub, |d, sub| d.accessibility_description(sub))
        .animation(ModalAnimation::Fade)
        .on_cancel(move || on_close())
        .flex_1()
        .items_center()
        .justify_center()
        .child(scrim)
        .child(panel)
        .into_element()
}

/// The phone sheet's panel: grab handle, title row, scrolling body, the confirmation under it.
fn phone_sheet(app: AppState, modal: Modal, on_close: Rc<dyn Fn()>) -> AnyElement {
    let Modal {
        test_id,
        title,
        body,
        done,
        confirm,
        ..
    } = modal;
    let role = role_of(confirm.is_some());
    let title = SharedString::from(title);
    let head = view()
        .flex_row()
        .items_center()
        .px(px(18.0))
        .pt(px(4.0))
        .pb(px(10.0))
        .child(
            text(title.clone())
                .role(AccessibilityRole::Header)
                .flex_1()
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .children(done.map(|label| close_button(app, label, &test_id, on_close.clone())));
    let cancel = element_ref();
    let confirming = confirm.map(|confirm| {
        let close = on_close.clone();
        let go = confirm.on_confirm.clone();
        after_answer(move || cancel.focus());
        let wide = ButtonOptions {
            wide: true,
            ..ButtonOptions::default()
        };
        view()
            .when_some(confirm.check, |part, check| {
                part.child(
                    text(check)
                        .mt(px(14.0))
                        .mb(px(4.0))
                        .text_center()
                        .font_family(FONT_TEXT)
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .text_color(color(Token::Ink3)),
                )
            })
            .child(
                view()
                    .flex_row()
                    .gap(px(8.0))
                    .mt(px(10.0))
                    .child(button_with(
                        app,
                        confirm.cancel,
                        ButtonKind::Ghost,
                        ButtonOptions {
                            focus: Some(cancel),
                            ..wide
                        },
                        format!("{test_id}-cancel"),
                        move || close(),
                    ))
                    .child(button_with(
                        app,
                        confirm.label,
                        ButtonKind::Primary,
                        wide,
                        format!("{test_id}-confirm"),
                        move || go(),
                    )),
            )
    });
    view()
        .test_id(&test_id)
        .role(role)
        .label(title)
        .pb_keyboard()
        .bg(color(Token::Surface))
        .rounded_t(px(14.0))
        .border_t(px(1.0))
        .border_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
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
        .child(head)
        .child(
            scroll()
                .max_h(move || px((window().height.0 * SHEET_SHARE - SHEET_HEAD).max(120.0)))
                .child(
                    view()
                        .px(px(18.0))
                        .pb(move || {
                            px(30.0)
                                + if keyboard_height() > px(0.0) {
                                    px(0.0)
                                } else {
                                    safe_area().bottom
                                }
                        })
                        .child(body)
                        .children(confirming),
                ),
        )
        .into_element()
}

/// The terminal dialog: the content pane turned into the modal, as the palette stands, so no
/// half-cut words of the page show beside it.
fn terminal_dialog(app: AppState, modal: Modal, on_close: Rc<dyn Fn()>) -> AnyElement {
    let Modal {
        test_id,
        title,
        sub,
        body,
        confirm,
        ..
    } = modal;
    let role = role_of(confirm.is_some());
    let title = SharedString::from(title);
    let close = on_close.clone();
    let backdrop = view()
        .absolute()
        .inset_0()
        .focusable(false)
        .accessibility_elements_hidden(true)
        .on_press(move || close());
    let header = view()
        .flex_row()
        .items_center()
        .h(term::rows(1.0))
        .px(px(8.0))
        .child(
            text(title.clone())
                .role(AccessibilityRole::Header)
                .flex_1()
                .number_of_lines(1u32)
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .when(confirm.is_some(), |row| {
            row.child(term::legend("Enter", "confirm"))
                .child(term::space(2.0))
        })
        .child(term::legend(
            "Esc",
            if confirm.is_some() { "cancel" } else { "close" },
        ));
    let typed = confirm.map(|confirm| {
        let go = confirm.on_confirm.clone();
        let word = confirm.word;
        let expected = word.clone();
        view().child(term::hrule()).child(
            view()
                .flex_row()
                .items_center()
                .px(px(8.0))
                .child(term::quiet("Type "))
                .child(
                    text(word.clone())
                        .flex_shrink_0()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
                .child(term::quiet(" to confirm"))
                .child(term::space(2.0))
                .child(
                    input()
                        .test_id(&format!("{test_id}-word"))
                        .label(format!("Type {word} to confirm"))
                        .placeholder(word)
                        .auto_focus(true)
                        .on_submit(move |typed: &str| {
                            if typed.trim() == expected {
                                go();
                            }
                        })
                        .flex_1()
                        .min_w(px(0.0))
                        .h(term::rows(1.0))
                        .px(px(8.0))
                        .text_size(px(text_size(Form::Terminal)))
                        .text_color(color(Token::Ink))
                        .placeholder_color(move || for_appearance(color(Token::Ink3)))
                        .caret_color(move || for_appearance(color(Token::Ink)))
                        // NSH draws a field without a fill of its own as a raised band.
                        .bg(AdaptiveColor::TRANSPARENT)
                        .border_1()
                        .border_color(color(Token::Rule2)),
                ),
        )
    });
    let panel = view()
        .test_id(&test_id)
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .absolute()
        .top(term::rows(2.0))
        .bottom(term::rows(2.0))
        .left(term::cells(term::sidebar_columns(app) + 1.0))
        .right(term::cells(1.0))
        .child(header)
        .when_some(sub.clone(), |panel, sub| {
            panel.child(
                view()
                    .px(px(8.0))
                    .child(text(sub).text_color(color(Token::Ink3))),
            )
        })
        .child(term::hrule())
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .child(view().px(px(8.0)).child(body)),
        )
        .children(typed);
    dialog()
        .role(role)
        .label(title)
        .when_some(sub, |d, sub| d.accessibility_description(sub))
        .animation(ModalAnimation::Fade)
        .on_cancel(move || on_close())
        .flex_1()
        .child(backdrop)
        .child(panel)
        .into_element()
}

/// A confirmation is an alert dialog: it asks before something happens.
fn role_of(confirming: bool) -> AccessibilityRole {
    if confirming {
        AccessibilityRole::AlertDialog
    } else {
        AccessibilityRole::Dialog
    }
}

/// The title row's plain close button ("Done"), test_id "<id>-done".
fn close_button(app: AppState, label: String, test_id: &str, on_close: Rc<dyn Fn()>) -> AnyElement {
    button(
        app,
        label,
        ButtonKind::Quiet,
        format!("{test_id}-done"),
        move || on_close(),
    )
}
