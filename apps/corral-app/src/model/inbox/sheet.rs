//! Shared handover descriptors, band ordering and shortcut help for feed-backed questions.

use super::Action;
pub use super::sheet_types::*;
use crate::model::text::{Rich, span};

pub(crate) fn button(label: impl Into<String>, action: Action, kind: ButtonKind) -> Button {
    Button {
        label: label.into(),
        action,
        enabled: true,
        kind,
        shortcut: None,
    }
}

pub(crate) fn band(
    kind: BandKind,
    letter: &'static str,
    title: &'static str,
    detail: Option<String>,
) -> Band {
    Band {
        kind,
        letter,
        title,
        detail,
    }
}

/// Desktop inline order is S, B, A, R, Thread, History. Focus mode moves B, Thread,
/// History to the side and leaves S, A, R in the centre so the choices stay visible.
pub(crate) fn desk_bands(sheet: &mut Sheet, with_side: bool, plan: Option<String>) {
    sheet.bands = vec![band(BandKind::Situation, "S", "Situation", None)];
    if !with_side {
        sheet
            .bands
            .push(band(BandKind::Background, "B", "Background", None));
    }
    sheet.bands.extend([
        band(BandKind::Assessment, "A", "Assessment", None),
        band(BandKind::Recommendation, "R", "Recommendation", None),
    ]);
    if with_side {
        sheet.side_bands = vec![
            band(BandKind::Background, "B", "Background · place", plan),
            band(
                BandKind::Thread,
                "T",
                "Thread with the asker",
                Some(sheet.thread.messages.len().to_string()),
            ),
            band(
                BandKind::History,
                "H",
                "History",
                Some("tried · your decisions".into()),
            ),
        ];
    } else {
        sheet.bands.extend([
            band(BandKind::Thread, "T", "Thread", None),
            band(BandKind::History, "H", "History", None),
        ]);
    }
}

/// Phone order is S, R, A, B, Thread, History; its thread is a collapsed preview opening
/// the shared sheet.
pub(crate) fn phone_bands(sheet: &mut Sheet) {
    let threads = sheet.thread.messages.len();
    sheet.bands = vec![
        band(BandKind::Situation, "S", "Situation", None),
        band(
            BandKind::Recommendation,
            "R",
            "Options",
            Some("consequence of each".into()),
        ),
        band(BandKind::Assessment, "A", "Evidence", None),
        band(BandKind::Background, "B", "Place", None),
        band(
            BandKind::Thread,
            "T",
            "Thread with the asker",
            Some(format!("{threads} · Ask ›")),
        ),
        band(BandKind::History, "H", "History", None),
    ];
}

/// The keys line on a desktop focus page.
pub(crate) fn key_help(choose: &str) -> Rich {
    vec![
        span("Keys "),
        span(choose.to_owned()).mono(),
        span(" choose · "),
        span("Ctrl ↵").mono(),
        span(" answer · "),
        span("Esc").mono(),
        span(" back"),
    ]
}
