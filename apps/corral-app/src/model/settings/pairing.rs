//! The fixture preview's Devices section and phone pairing sheets: the illustrative device
//! table, the pairing walkthrough and its QR-like image.

use super::{
    Action, Button, DEVICES, Device, Emphasis, PAYLOAD, STEPS, Sheet, State, Step, button,
};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};

/// Pairing step status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Done,
    On,
    Pending,
}
/// Pairing instruction with its resolved status and displayed ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepView {
    pub step: Step,
    pub status: StepStatus,
    pub ordinal: usize,
}
fn step_views(state: &State) -> Vec<StepView> {
    STEPS
        .iter()
        .enumerate()
        .map(|(i, step)| StepView {
            step: *step,
            status: if i < state.pair as usize {
                StepStatus::Done
            } else if i == state.pair as usize {
                StepStatus::On
            } else {
                StepStatus::Pending
            },
            ordinal: i + 1,
        })
        .collect()
}
/// Pending pairing details and confirmation controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmView {
    pub device: &'static str,
    pub detail: &'static str,
    pub code_label: &'static str,
    pub code: &'static str,
    pub buttons: Vec<Button>,
}
fn confirm_view() -> ConfirmView {
    ConfirmView {
        device: "pawis-ipad",
        detail: "wants to pair · iPadOS · via direct · lan",
        code_label: "Code on that device:",
        code: "482 913",
        buttons: vec![
            button(
                "Codes match · confirm",
                Action::PairConfirm,
                Emphasis::Primary,
            ),
            button("Reject", Action::PairReject, Emphasis::Ghost),
        ],
    }
}
/// Icon kind for the phone device row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Desktop,
    Phone,
    Terminal,
}

/// Device table row with attribution and applicable controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceView {
    pub device: Device,
    /// Whether the last-seen value is current rather than dimmed.
    pub recent: bool,
    pub kind: DeviceKind,
    pub phone_line: String,
    pub attribution: String,
    pub self_chip: Option<&'static str>,
    pub permission_hue: Hue,
    pub turn_on: Option<Button>,
    pub revoke: Option<Button>,
}
/// Devices section, including desktop pairing flow and phone entry controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicesView {
    pub title: &'static str,
    pub columns: [&'static str; 6],
    pub intro: &'static str,
    pub qr: [[bool; 29]; 29],
    pub qr_note: &'static str,
    pub steps: Vec<StepView>,
    pub confirm: Option<ConfirmView>,
    pub paired: Option<Rich>,
    pub payload: &'static [(&'static str, &'static str)],
    pub step_buttons: Vec<Button>,
    pub phone_buttons: Vec<Button>,
    pub paired_label: &'static str,
    pub devices: Vec<DeviceView>,
}
/// Build the Devices section for both forms.
pub fn devices(state: &State, _env: Env) -> DevicesView {
    let mut step_buttons = Vec::new();
    if state.pair > 0 {
        step_buttons.push(button("Back a step", Action::PairStep(-1), Emphasis::Quiet));
    }
    if state.pair < 3 {
        step_buttons.push(button(
            "Next step (demo)",
            Action::PairStep(1),
            Emphasis::Ghost,
        ));
    }
    DevicesView {
        title: "Devices",
        columns: [
            "Device",
            "Platform",
            "Last seen",
            "Reach path",
            "Attribution",
            "May instruct",
        ],
        intro: "One owner, many devices. Each device signs its actions; revoking one ends its sessions at once.",
        qr: qr(),
        qr_note: "QR illustrative, not scannable",
        steps: step_views(state),
        confirm: (2..4).contains(&state.pair).then(confirm_view),
        paired: (state.pair == 4).then(|| {
            vec![
                span("pawis-ipad paired.").bold(),
                span(" Signs as "),
                span("via=app:pawis-ipad").mono(),
                span("; reach direct · lan."),
            ]
        }),
        payload: &PAYLOAD,
        step_buttons,
        phone_buttons: vec![
            button(
                "Show pairing QR",
                Action::OpenSheet(Sheet::Pair),
                Emphasis::Primary,
            ),
            button(
                "Join (new device)",
                Action::OpenSheet(Sheet::Scan),
                Emphasis::Ghost,
            ),
        ],
        paired_label: "Paired",
        devices: DEVICES
            .iter()
            .map(|d| DeviceView {
                device: *d,
                recent: d.seen == "now",
                kind: if d.platform.contains("iOS") {
                    DeviceKind::Phone
                } else if d.platform.contains("terminal") {
                    DeviceKind::Terminal
                } else {
                    DeviceKind::Desktop
                },
                phone_line: format!(
                    "{} · {} · {} · may instruct: {}",
                    d.platform, d.seen, d.reach, d.may_instruct
                ),
                attribution: format!("via=app:{}", d.name),
                self_chip: d.this_device.then_some("this device"),
                permission_hue: if d.may_instruct == "yes" {
                    Hue::Ok
                } else {
                    Hue::Neutral
                },
                turn_on: (d.may_instruct != "yes")
                    .then(|| button("Turn on", Action::TurnOn(d.name), Emphasis::Quiet)),
                revoke: (!d.this_device)
                    .then(|| button("Revoke", Action::Revoke(d.name), Emphasis::Danger)),
            })
            .collect(),
    }
}
/// The illustrative QR-like image, indexed by row then column, with dark cells true.
pub fn qr() -> [[bool; 29]; 29] {
    let mut cells = [[false; 29]; 29];
    let mut seed = 868_i64;
    for y in 0..29 {
        for x in 0..29 {
            let in_finder = (x < 8 && y < 8) || (x > 20 && y < 8) || (x < 8 && y > 20);
            if !in_finder {
                // JavaScript multiplies as f64 before its signed 32-bit bitwise conversion.
                let product = (seed as f64 * 1103515245.0 + 12345.0) as i64;
                seed = product & 0x7fff_ffff;
                cells[y][x] = seed as f64 / 2147483647.0 > 0.52
                    || (y == 6 && x % 2 == 0)
                    || (x == 6 && y % 2 == 0);
            }
        }
    }
    for (ox, oy) in [(0, 0), (22, 0), (0, 22)] {
        for y in 0..7 {
            for x in 0..7 {
                cells[oy + y][ox + x] =
                    x == 0 || x == 6 || y == 0 || y == 6 || (x >= 2 && x <= 4 && y >= 2 && y <= 4);
            }
        }
    }
    cells
}
/// Phone pairing sheet content, including the native-camera placeholder description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetView {
    Pair {
        title: &'static str,
        qr: [[bool; 29]; 29],
        note: &'static str,
        steps: Vec<StepView>,
    },
    Scan {
        title: &'static str,
        prompt: &'static str,
        after: &'static str,
        code: &'static str,
        confirm: Rich,
        note: &'static str,
    },
}
/// Build the currently open phone sheet, if any.
pub fn sheet(state: &State, _env: Env) -> Option<SheetView> {
    match state.sheet {
        Some(Sheet::Pair) => Some(SheetView::Pair {
            title: "Pair a device",
            qr: qr(),
            note: "QR illustrative",
            steps: step_views(state),
        }),
        Some(Sheet::Scan) => Some(SheetView::Scan {
            title: "Join corral",
            prompt: "Point at the QR on a trusted surface",
            after: "After scanning, this phone shows",
            code: "482 913",
            confirm: vec![
                span("Confirm the same code on "),
                span("pawis-desktop").bold(),
                span("."),
            ],
            note: "camera view needs a native component (not in NSH)",
        }),
        None => None,
    }
}
