//! Plan headers on the lanes canvas (desktop and phone): a hairline over each group, the fold
//! caret, the plan's id and title in the label column, and on the plot a thin summary bar from
//! the group's earliest to latest activity with its landed share filled, then `n/m landed`.
//! Each header is a button (`plan-<key>`) that folds or unfolds its lanes.

use nsh_core::canvas::CanvasTextAlign;
use nsh_core::prelude::*;

use super::chart::{ID_ADVANCE, LABEL_GAP, NOTE_ADVANCE, RING, Rect, TITLE_ADVANCE, clip};
use crate::model::timeline::{self, Layout, PlacedHeader};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::chart::{ink, mono_font, now, text_font, tip};
use crate::ui::kit::marks::hue_ink;
use crate::ui::tokens::{Token, color};

/// The summary bar's height.
const SUMMARY: f32 = 4.0;
/// Room for the caret before the plan's id.
const CARET: f32 = 14.0;

/// The fold caret: pointing down while the plan's lanes show.
pub fn caret(folded: bool) -> &'static str {
    if folded { "▸" } else { "▾" }
}

fn baseline(layout: &Layout, phone: bool, y: f32) -> f32 {
    if phone {
        y + 14.0
    } else {
        y + layout.header_height / 2.0 + 4.0
    }
}

/// The summary bar's top: centred in the row on desktop, under the words on the phone.
fn bar_top(layout: &Layout, phone: bool, y: f32) -> f32 {
    if phone {
        y + 24.0
    } else {
        y + (layout.header_height - SUMMARY) / 2.0
    }
}

fn tally_width(header: &PlacedHeader) -> f32 {
    header.tally.chars().count() as f32 * NOTE_ADVANCE
}

pub fn paint(mut p: Painter, layout: &Layout, phone: bool) -> Painter {
    let quiet = ink(Token::Ink3);
    for header in &layout.headers {
        let base = baseline(layout, phone, header.y);
        p = p
            .stroke_color(ink(Token::Rule))
            .line_width(1.0)
            .begin_path()
            .move_to(0.0, header.y + 0.5)
            .line_to(layout.width, header.y + 0.5)
            .stroke()
            .text_align(CanvasTextAlign::Start)
            .font(&text_font(600, 12.0))
            .fill_color(quiet)
            .fill_text(caret(header.folded), 0.0, base);
        let right = if phone {
            layout.width - tally_width(header) - LABEL_GAP
        } else {
            layout.label_width - LABEL_GAP
        };
        let mut x = CARET;
        if let Some(id) = &header.id {
            p = p
                .font(&mono_font(11.0))
                .fill_color(quiet)
                .fill_text(id, x, base);
            x += id.chars().count() as f32 * ID_ADVANCE + 6.0;
        }
        let title = clip(&header.title, right - x, TITLE_ADVANCE);
        p = p
            .font(&text_font(600, 12.0))
            .fill_color(ink(Token::Ink))
            .fill_text(&title, x, base);
        if let Some((x1, x2)) = header.bar {
            let top = bar_top(layout, phone, header.y);
            p = p
                .fill_color(ink(Token::Rule))
                .begin_path()
                .round_rect(Rect::new(x1, top, x2 - x1, SUMMARY).bounds(), px(2.0))
                .fill();
            if header.share > 0.0 {
                let filled = ((x2 - x1) * header.share).max(2.0);
                p = p
                    .fill_color(now(hue_ink(Hue::Ok)))
                    .begin_path()
                    .round_rect(Rect::new(x1, top, filled, SUMMARY).bounds(), px(2.0))
                    .fill();
            }
        }
        p = p.font(&text_font(400, 11.0)).fill_color(quiet);
        p = if phone {
            p.text_align(CanvasTextAlign::End)
                .fill_text(&header.tally, layout.width - 2.0, base)
        } else {
            // After the bar where it fits, else before it, else at the trailing edge.
            let (x1, x2) = header
                .bar
                .unwrap_or((layout.label_width, layout.label_width));
            let width = tally_width(header);
            if x2 + 6.0 + width <= layout.width {
                p.text_align(CanvasTextAlign::Start)
                    .fill_text(&header.tally, x2 + 6.0, base)
            } else if x1 - 6.0 - width >= layout.label_width {
                p.text_align(CanvasTextAlign::End)
                    .fill_text(&header.tally, x1 - 6.0, base)
            } else {
                p.text_align(CanvasTextAlign::End).fill_text(
                    &header.tally,
                    layout.width - 2.0,
                    base,
                )
            }
        };
    }
    p
}

/// Each header's button over its row (`plan-<key>`): pressing it folds or unfolds the plan.
pub fn buttons(app: AppState, layout: &Layout) -> Vec<AnyElement> {
    let form = app.form;
    layout
        .headers
        .iter()
        .map(|header| {
            let key = header.key.clone();
            let spoken = timeline::header_spoken(header);
            view()
                .absolute()
                .left(px(0.0))
                .top(px(header.y))
                .w(px(layout.width))
                .h(px(layout.header_height))
                .test_id(&format!("plan-{}", header.key))
                .cursor_pointer()
                .rounded(px(4.0))
                .role(AccessibilityRole::Button)
                .label(spoken.clone())
                .on_press(move || {
                    let key = key.clone();
                    app.act(app.timeline, move |state, _| state.fold(&key));
                })
                .focus_visible(|s| {
                    s.outline(px(RING))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(-RING))
                })
                .when(form == Form::Desktop, |v| v.tooltip(tip(spoken.into())))
                .into_element()
        })
        .collect()
}
