//! Settings › Devices: the pairing flow (QR, the four steps, the pending device's matching
//! code, the key exchange payload) and the paired devices with their reach, attribution and
//! permission. The phone opens the QR and the join view as sheets.

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use super::parts::{
    act, action_button, button_row, cell, frame, intro, model_button, mono_cell, read, space,
    test_id, trailing,
};
use crate::model::settings::{
    self, Action, ConfirmView, DeviceKind, DeviceView, DevicesView, SHEET_DONE, Sheet, SheetView,
    StepStatus, StepView,
};
use crate::model::{Form, Hue, Rich};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::ButtonOptions;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::kit::notice::{notice, tone_mark};
use crate::ui::kit::overlay::{Modal, modal};
use crate::ui::kit::page::{Width, group_head, panel};
use crate::ui::kit::table::{StepState, devtable, qtable, steps4};
use crate::ui::kit::text::rich;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The QR tile's side and its quiet zone (`.qr`: 210 pt, padding 12).
const QR_SIDE: f32 = 210.0;
const QR_PAD: f32 = 12.0;
/// A terminal cannot draw the QR: this line stands in its place.
const TERMINAL_QR: &str = "QR: open Devices in the desktop or phone app to show it";

/// Devices on the desktop and in a terminal: the title and its line, the pairing panel (the QR
/// beside the steps; a terminal names where to find the QR instead), then the paired devices.
pub fn page(app: AppState) -> AnyElement {
    let v = untracked(|| read(app, settings::devices));
    let steps = view().flex_1().min_w(px(0.0)).child(dynamic(move || {
        let v = read(app, settings::devices);
        untracked(move || pairing_steps(app, &v))
    }));
    let pairing = if app.form == Form::Terminal {
        view()
            .child(illus(app, TERMINAL_QR))
            .child(term::blank())
            .child(steps)
            .into_element()
    } else {
        view()
            .flex_row()
            .items_start()
            .gap(px(24.0))
            .child(
                view()
                    .w(px(230.0))
                    .flex_shrink_0()
                    .child(qr(v.qr, v.qr_note))
                    .child(view().mt(px(6.0)).child(illus(app, v.qr_note))),
            )
            .child(steps)
            .into_element()
    };
    let devices = if app.form == Form::Terminal {
        terminal_devices(app, &v)
    } else {
        desktop_devices(app, &v)
    };
    frame(
        app,
        v.title,
        4.0,
        vec![
            intro(app, v.intro),
            space(app, 14.0),
            panel(app, None, pairing),
            space(app, 22.0),
            devices,
        ],
    )
}

/// Devices on the phone: the two pairing buttons, the pending device's code while one waits,
/// the paired devices under their count, and the QR and join sheets.
pub fn phone(app: AppState) -> AnyElement {
    let v = untracked(|| read(app, settings::devices));
    let entry = v
        .phone_buttons
        .iter()
        .map(|button| {
            let options = ButtonOptions {
                icon: matches!(button.action, Action::OpenSheet(Sheet::Pair)).then_some(Icon::Qr),
                wide: true,
                ..ButtonOptions::default()
            };
            model_button(app, button, options, test_id(&button.action))
        })
        .collect::<Vec<_>>();
    let pending = dynamic(move || {
        let confirm = read(app, |state, env| settings::devices(state, env).confirm);
        untracked(move || {
            confirm.map(|confirm| {
                view()
                    .px(px(14.0))
                    .pt(px(8.0))
                    .child(confirm_box(app, &confirm))
                    .into_element()
            })
        })
    });
    view()
        .child(
            view()
                .flex_row()
                .gap(px(8.0))
                .px(px(18.0))
                .pt(px(14.0))
                .pb(px(6.0))
                .children(entry),
        )
        .child(pending)
        .child(group_head(
            app,
            v.paired_label,
            Some(v.devices.len().to_string()),
            Hue::Neutral,
            None,
            None,
        ))
        .children(
            v.devices
                .iter()
                .map(|device| phone_device(app, device))
                .collect::<Vec<_>>(),
        )
        .child(sheet_host(app, Sheet::Pair))
        .child(sheet_host(app, Sheet::Scan))
        .into_element()
}

/// The steps, then the pending device or the pairing result, the payload and the step buttons.
fn pairing_steps(app: AppState, v: &DevicesView) -> Vec<AnyElement> {
    let mut parts = vec![steps(app, &v.steps)];
    if let Some(confirm) = &v.confirm {
        parts.push(space(app, 14.0));
        parts.push(confirm_box(app, confirm));
    }
    if let Some(paired) = &v.paired {
        parts.push(space(app, 14.0));
        parts.push(notice(app, Hue::Ok, paired, None));
    }
    parts.push(space(app, 12.0));
    parts.push(payload(app, v.payload));
    if !v.step_buttons.is_empty() {
        parts.push(space(app, 12.0));
        parts.push(button_row(
            app,
            v.step_buttons
                .iter()
                .map(|button| action_button(app, button))
                .collect(),
        ));
    }
    parts
}

/// The numbered steps (`.steps4`), each in the state the model resolved.
fn steps(app: AppState, steps: &[StepView]) -> AnyElement {
    steps4(
        app,
        steps
            .iter()
            .map(|step| {
                let state = match step.status {
                    StepStatus::Done => StepState::Done,
                    StepStatus::On => StepState::On,
                    StepStatus::Pending => StepState::Next,
                };
                (
                    state,
                    step.step.title.to_owned(),
                    step.step.detail.to_owned(),
                )
            })
            .collect(),
    )
}

/// The key exchange payload: the key in quiet ink, the value in mono.
fn payload(app: AppState, rows: &'static [(&'static str, &'static str)]) -> AnyElement {
    qtable(
        app,
        &[Width::Fixed(110.0), Width::Fill],
        rows.iter()
            .map(|&(key, value)| {
                vec![
                    cell(app, key, Token::Ink3, false).into_element(),
                    mono_cell(app, value, Token::Ink).into_element(),
                ]
            })
            .collect(),
    )
}

/// The pending device on the warning ground (`.notice.warn`): who asks and over which path,
/// the six-digit code to compare (`.code6`), and confirm or reject. Desktop: the buttons at the
/// trailing edge; phone: under the code, at the trailing edge; terminal: led by the warning
/// mark, the buttons on their own row.
fn confirm_box(app: AppState, confirm: &ConfirmView) -> AnyElement {
    let form = app.form;
    let buttons = button_row(
        app,
        confirm
            .buttons
            .iter()
            .map(|button| action_button(app, button))
            .collect(),
    );
    let (size, code_size) = match form {
        Form::Phone => (13.5, 24.0),
        Form::Desktop | Form::Terminal => (12.5, 22.0),
    };
    let asks = text("")
        .text_color(color(Token::Ink))
        .child(text(confirm.device).font_weight(FontWeight::BOLD))
        .child(" ")
        .child(text(confirm.detail));
    let label = text(confirm.code_label).text_color(color(Token::Ink3));
    let code = text(confirm.code)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    if form == Form::Terminal {
        return view()
            .flex_row()
            .items_start()
            .child(
                text(tone_mark(Hue::Held))
                    .flex_shrink_0()
                    .font_weight(FontWeight::BOLD)
                    .text_color(hue_ink(Hue::Held)),
            )
            .child(term::space(1.0))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(asks)
                    .child(
                        view()
                            .flex_row()
                            .child(label)
                            .child(term::space(1.0))
                            .child(code),
                    )
                    .child(buttons),
            )
            .into_element();
    }
    let asks = asks
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px((size * 1.5).round()));
    let code_line = view()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .gap(px(6.0))
        .child(label.font_family(FONT_TEXT).text_size(px(size)))
        .child(
            code.font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(code_size))
                .font_weight(FontWeight::SEMIBOLD)
                .letter_spacing(px(code_size * 0.12)),
        );
    let text_part = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(asks)
        .child(code_line);
    let ground = view()
        .rounded(px(if form == Form::Phone { 10.0 } else { 8.0 }))
        .bg(hue_fill(Hue::Held))
        .px(px(14.0))
        .py(px(if form == Form::Phone { 12.0 } else { 10.0 }));
    if form == Form::Phone {
        return ground
            .child(text_part)
            .child(view().flex_row().justify_end().mt(px(10.0)).child(buttons))
            .into_element();
    }
    ground
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(text_part)
        .child(view().flex_shrink_0().child(buttons))
        .into_element()
}

/// The paired devices as the mockup's `.devtable`: name with the this-device chip, platform,
/// last seen (quiet unless recent), reach in mono, attribution in quiet mono, the permission
/// chip with Turn on, and Revoke at the trailing edge.
fn desktop_devices(app: AppState, v: &DevicesView) -> AnyElement {
    let mut header: Vec<&str> = v.columns.to_vec();
    header.push("");
    let widths = [
        Width::Fill,
        Width::Fill,
        Width::Fixed(84.0),
        Width::Fixed(124.0),
        Width::Fill,
        Width::Fixed(150.0),
        Width::Fixed(96.0),
    ];
    let rows = v
        .devices
        .iter()
        .map(|device| {
            vec![
                name(app, device),
                cell(app, device.device.platform, Token::Ink, false).into_element(),
                cell(
                    app,
                    device.device.seen,
                    if device.recent {
                        Token::Ink
                    } else {
                        Token::Ink3
                    },
                    false,
                )
                .into_element(),
                mono_cell(app, device.device.reach, Token::Ink).into_element(),
                mono_cell(app, device.attribution.clone(), Token::Ink3).into_element(),
                permission(app, device),
                trailing(revoke(app, device)),
            ]
        })
        .collect();
    devtable(app, &widths, &header, rows)
}

/// The device's name in bold and, on this device, its chip beside it.
fn name(app: AppState, device: &DeviceView) -> AnyElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.0))
        .child(cell(app, device.device.name, Token::Ink, true))
        .children(device.self_chip.map(|label| chip(app, label, Hue::Neutral)))
        .into_element()
}

/// What the device may do, as a chip, and Turn on where it may not instruct yet.
fn permission(app: AppState, device: &DeviceView) -> AnyElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.0))
        .child(chip(app, device.device.may_instruct, device.permission_hue))
        .children(
            device
                .turn_on
                .as_ref()
                .map(|button| action_button(app, button)),
        )
        .into_element()
}

/// Revoke, or nothing for this device.
fn revoke(app: AppState, device: &DeviceView) -> AnyElement {
    match &device.revoke {
        Some(button) => action_button(app, button),
        None => view().into_element(),
    }
}

/// The paired devices in a terminal, which has no room for seven columns: per device, its name,
/// chip and attribution with Turn on and Revoke at the end of the row, then the model's one-line
/// summary quiet under it.
fn terminal_devices(app: AppState, v: &DevicesView) -> AnyElement {
    view()
        .children(
            v.devices
                .iter()
                .map(|device| {
                    let controls = device
                        .turn_on
                        .iter()
                        .chain(device.revoke.iter())
                        .map(|button| action_button(app, button))
                        .collect::<Vec<_>>();
                    view()
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .child(
                                    text(device.device.name)
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(color(Token::Ink)),
                                )
                                .children(device.self_chip.map(|label| {
                                    view().flex_row().child(term::space(1.0)).child(chip(
                                        app,
                                        label,
                                        Hue::Neutral,
                                    ))
                                }))
                                .child(term::space(2.0))
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .child(term::quiet(device.attribution.clone())),
                                )
                                .child(button_row(app, controls)),
                        )
                        .child(
                            text(device.phone_line.clone())
                                .number_of_lines(1u32)
                                .truncate()
                                .text_color(color(Token::Ink3)),
                        )
                        .child(term::blank())
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// A paired device on the phone (`.prow`): its kind's icon, the name with the this-device chip,
/// the one-line summary under it, and Revoke.
fn phone_device(app: AppState, device: &DeviceView) -> AnyElement {
    let glyph = match device.kind {
        DeviceKind::Phone => Icon::Phone,
        DeviceKind::Terminal => Icon::Term,
        DeviceKind::Desktop => Icon::Agents,
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .min_h(px(60.0))
        .px(px(18.0))
        .py(px(10.0))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .child(
            view()
                .w(px(28.0))
                .flex_shrink_0()
                .child(icon(glyph, 18.0, color(Token::Ink2))),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            text(device.device.name)
                                .font_family(FONT_TEXT)
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color(Token::Ink)),
                        )
                        .children(device.self_chip.map(|label| chip(app, label, Hue::Neutral))),
                )
                .child(
                    text(device.phone_line.clone())
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .text_color(color(Token::Ink3)),
                ),
        )
        .children(
            device
                .revoke
                .as_ref()
                .map(|button| action_button(app, button)),
        )
        .into_element()
}

/// The illustrative QR (`.qr`): dark modules on a pale tile in both appearances, as a code
/// stays readable to a camera. The tile and the modules are the shell's ink and ground, the
/// one pair that keeps its light-over-dark order in both themes. One element for screen
/// readers, named by its note.
fn qr(cells: [[bool; 29]; 29], note: &'static str) -> AnyElement {
    let inner = QR_SIDE - 2.0 * QR_PAD;
    let module = inner / 29.0;
    // Runs of dark modules in a row as one rectangle each, on whole points, so no seams show.
    let drawing = canvas(move || {
        let mut painter = Painter::new().fill_style(for_appearance(color(Token::Shell)));
        for (y, row) in cells.iter().enumerate() {
            let top = (y as f32 * module).round();
            let bottom = ((y + 1) as f32 * module).round();
            let mut x = 0;
            while x < row.len() {
                if !row[x] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < row.len() && row[x] {
                    x += 1;
                }
                let left = (start as f32 * module).round();
                let right = (x as f32 * module).round();
                painter = painter.fill_rect(px(left), px(top), px(right - left), px(bottom - top));
            }
        }
        painter
    })
    .size(px(inner));
    view()
        .size(px(QR_SIDE))
        .flex_shrink_0()
        .p(px(QR_PAD))
        .rounded(px(8.0))
        .bg(color(Token::ShellInk))
        .role(AccessibilityRole::Image)
        .label(note)
        .child(drawing)
        .into_element()
}

/// One of the phone's pairing sheets, shown while the model's sheet is `which`; closing it
/// (Done, the scrim, a swipe) runs the model's close.
fn sheet_host(app: AppState, which: Sheet) -> AnyElement {
    modal(
        app,
        move || app.settings.with(|state| state.sheet == Some(which)),
        move || act(app, Action::CloseSheet),
        move || sheet_content(app, which),
    )
}

/// The sheet's title and body from the model's sheet view for `which`.
fn sheet_content(app: AppState, which: Sheet) -> Modal {
    // Built for `which` whatever is open now: the host makes its content before it opens.
    let shown = read(app, |state, env| {
        let mut state = state.clone();
        state.sheet = Some(which);
        settings::sheet(&state, env)
    });
    let (title, body) = match shown {
        Some(SheetView::Pair {
            title,
            qr: cells,
            note,
            steps: items,
        }) => (
            title,
            view()
                .child(
                    view()
                        .items_center()
                        .gap(px(10.0))
                        .child(qr(cells, note))
                        .child(illus(app, note)),
                )
                .child(view().mt(px(16.0)).child(steps(app, &items)))
                .into_element(),
        ),
        Some(SheetView::Scan {
            title,
            prompt,
            after,
            code,
            confirm,
            note,
        }) => (title, scan(app, prompt, after, code, &confirm, note)),
        None => ("", view().into_element()),
    };
    let slug = match which {
        Sheet::Pair => "pair",
        Sheet::Scan => "scan",
    };
    Modal {
        test_id: format!("sheet-{slug}"),
        title: title.to_owned(),
        sub: None,
        body,
        done: Some(SHEET_DONE.to_owned()),
        confirm: None,
    }
}

/// Joining from the phone: where the camera view goes (a dark square with the viewfinder and
/// the prompt, as NSH has no camera component), then the code this phone shows and where to
/// confirm it.
fn scan(
    app: AppState,
    prompt: &'static str,
    after: &'static str,
    code: &'static str,
    confirm: &Rich,
    note: &'static str,
) -> AnyElement {
    let camera = view()
        .relative()
        .w_full()
        .aspect_ratio(1.0)
        .rounded(px(16.0))
        .overflow_hidden()
        .bg(color(Token::Shell))
        .items_center()
        .justify_center()
        .child(
            view()
                .w(relative(0.62))
                .aspect_ratio(1.0)
                .rounded(px(14.0))
                .border(px(3.0))
                .border_color(color(Token::ShellInk))
                .accessibility_elements_hidden(true),
        )
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(12.0))
                .items_center()
                .child(
                    text(prompt)
                        .font_family(FONT_TEXT)
                        .text_size(px(13.0))
                        .text_color(color(Token::ShellInk)),
                ),
        );
    view()
        .child(camera)
        .child(
            view()
                .mt(px(16.0))
                .items_center()
                .child(
                    text(after)
                        .text_center()
                        .font_family(FONT_TEXT)
                        .text_size(px(14.0))
                        .text_color(color(Token::Ink3)),
                )
                .child(
                    text(code)
                        .my(px(6.0))
                        .font_family(FONT_MONO)
                        .tabular_nums()
                        .text_size(px(28.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .letter_spacing(px(28.0 * 0.12))
                        .text_color(color(Token::Ink)),
                )
                .child(rich(app, confirm))
                .child(view().mt(px(8.0)).child(illus(app, note))),
        )
        .into_element()
}
