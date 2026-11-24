//! A card on the map's canvas: its box, name, tag and figure, the overlay's bar and the
//! processes a package spawns. Under a task's marks (Impact) the figure is what the change did
//! (lines added and removed in their colours, the changed files) or, on an untouched card, its
//! size; a touched card is ringed solid and one holding dependents dashed, with their count as
//! a chip; no overlay bar is drawn, since on a marked card it would read as a measure of the
//! change.

use nsh_core::canvas::CanvasTextAlign;
use nsh_core::prelude::*;

use crate::model::Hue;
use crate::model::system::layout::{self, Font, Rect};
use crate::model::system::marks::{self, DEPENDENT, Mark, TOUCHED};
use crate::model::system::scene::{Card, Kind, Scene};
use crate::model::system::tree::share;
use crate::model::system::words::{activity, figure, language_slot, read_shares, slot_hue};
use crate::model::system::{Overlay, Window};
use crate::model::text::{Rich, span};
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::tokens::Token;

use super::canvas::{bounds, css, ink, measure, now};

pub(super) struct Look {
    pub(super) overlay: Overlay,
    pub(super) window: Window,
    pub(super) languages: Vec<String>,
    pub(super) busiest: u64,
}

/// `words` cut with an ellipsis to fit `room` points.
fn fit(words: &str, room: f32, font: Font) -> String {
    if measure(font, words) <= room {
        return words.to_owned();
    }
    let mut kept: Vec<char> = words.chars().collect();
    while !kept.is_empty() {
        kept.pop();
        let cut: String = kept.iter().collect::<String>() + "…";
        if measure(font, &cut) <= room {
            return cut;
        }
    }
    String::new()
}

fn width(line: &Rich, font: Font) -> f32 {
    line.iter().map(|run| measure(font, &run.text)).sum()
}

/// `line`'s runs left to right from `x`, each in its hue (`plain` where it has none), the run
/// that passes `room` cut with an ellipsis and the rest left out.
fn runs(
    p: Painter,
    line: &Rich,
    (x, y): (f32, f32),
    room: f32,
    font: Font,
    plain: Hsla,
) -> Painter {
    let mut p = p.font(&css(font)).text_align(CanvasTextAlign::Start);
    let end = x + room;
    let mut at = x;
    for run in line {
        let fill = if run.hue == Hue::Neutral {
            plain
        } else {
            now(hue_ink(run.hue))
        };
        let w = measure(font, &run.text);
        if at + w > end {
            return p
                .fill_color(fill)
                .fill_text(&fit(&run.text, end - at, font), at, y);
        }
        p = p.fill_color(fill).fill_text(&run.text, at, y);
        at += w;
    }
    p
}

pub(super) fn paint_card(
    p: Painter,
    card: &Card,
    r: Rect,
    look: &Look,
    scene: &Scene,
    picked: bool,
    // Under a task's marks: this card's, if any.
    mark: Option<Option<Mark>>,
) -> Painter {
    let port = card.kind == Kind::Port;
    let radius = if port { r.h / 2.0 } else { 6.0 };
    let mut p = p;
    if picked {
        p = p
            .stroke_color(ink(Token::Focus))
            .line_width(2.0)
            .begin_path()
            .round_rect(bounds(grown(r, 4.0)), px(radius + 3.0))
            .stroke();
    }
    p = p
        .fill_color(ink(Token::Surface))
        .begin_path()
        .round_rect(bounds(r), px(radius))
        .fill();
    if mark.is_none()
        && look.overlay == Overlay::Activity
        && matches!(card.kind, Kind::Package | Kind::Folder | Kind::Module)
    {
        let commits = activity(card, look.window).map_or(0, |a| a.0);
        if commits > 0 {
            let strength = 0.06 + 0.34 * (commits as f32 / look.busiest as f32).sqrt();
            p = p
                .fill_color(now(hue_ink(Hue::Worker)).opacity(strength))
                .begin_path()
                .round_rect(bounds(r), px(radius))
                .fill();
        }
    }
    let dashed = card.excluded()
        || matches!(
            card.kind,
            Kind::Folder | Kind::More | Kind::Owner | Kind::Scripts
        );
    let dash = [px(4.0), px(3.0)];
    p = p
        .stroke_color(ink(if card.excluded() {
            Token::Ink4
        } else {
            Token::Rule2
        }))
        .line_width(1.0)
        .line_dash(if dashed { &dash } else { &[] })
        .begin_path()
        .round_rect(bounds(r), px(radius))
        .stroke()
        .line_dash(&[]);
    if let Some(Some(mark)) = mark {
        p = rings(p, r, radius, mark);
    }
    if port {
        // The name wins: the dependents give way first, then the whole tag; the button over
        // the card still says it all.
        let lines = match mark {
            Some(mark) => vec![marks::port_line(card, mark), marks::figure_line(card, mark)],
            None => vec![vec![span(figure(card, look.overlay, look.window))]],
        };
        let name_w = measure(Font::Name, &card.name);
        let tag = lines
            .into_iter()
            .find(|line| name_w + width(line, Font::Tag) + 36.0 <= r.w)
            .unwrap_or_default();
        let tag_w = width(&tag, Font::Tag);
        let p = p
            .font(&css(Font::Name))
            .fill_color(ink(Token::Ink))
            .text_align(CanvasTextAlign::Start)
            .fill_text(
                &fit(&card.name, r.w - tag_w - 36.0, Font::Name),
                r.x + 14.0,
                r.y + r.h / 2.0 + 4.5,
            );
        let at = (r.x + r.w - 13.0 - tag_w, r.y + r.h / 2.0 + 4.0);
        // A point to spare, so rounding never cuts the measured tag.
        return runs(p, &tag, at, tag_w + 1.0, Font::Tag, ink(Token::Ink3));
    }
    let name_font = if matches!(card.kind, Kind::More | Kind::Scripts) {
        Font::Sub
    } else {
        Font::Name
    };
    let name_w = measure(name_font, &card.name);
    // The name wins: a tag that would crowd it stays in the inspector.
    let tag = card
        .tag
        .as_deref()
        .filter(|tag| name_w + measure(Font::Tag, tag) + 38.0 <= r.w);
    let tag_w = tag.map_or(0.0, |t| measure(Font::Tag, t) + 12.0);
    p = p
        .font(&css(name_font))
        .fill_color(ink(Token::Ink))
        .text_align(CanvasTextAlign::Start)
        .fill_text(
            &fit(&card.name, r.w - 24.0 - tag_w, name_font),
            r.x + 12.0,
            r.y + 22.0,
        );
    if let Some(tag) = tag {
        p = p
            .font(&css(Font::Tag))
            .fill_color(ink(Token::Ink3))
            .text_align(CanvasTextAlign::End)
            .fill_text(tag, r.x + r.w - 11.0, r.y + 21.0);
    }
    let line = match mark {
        Some(mark) => marks::figure_line(card, mark),
        None => vec![span(figure(card, look.overlay, look.window))],
    };
    p = runs(
        p,
        &line,
        (r.x + 12.0, r.y + 39.0),
        r.w - 24.0,
        Font::Sub,
        ink(Token::Ink2),
    );
    if let Some(mark) = mark {
        if let Some(words) = mark.and_then(Mark::dependent_words) {
            p = dependents_chip(p, &words, r);
        }
    } else {
        if matches!(card.kind, Kind::More | Kind::Definition) || card.kind.barless() {
            return p;
        }
        p = bar(
            p,
            card,
            Rect {
                x: r.x + 12.0,
                y: r.y + 46.0,
                w: r.w - 24.0,
                h: 4.0,
            },
            look,
            scene,
        );
    }
    if !card.spawns.is_empty() {
        p = spawn_chips(p, card, r);
    }
    p
}

fn grown(r: Rect, by: f32) -> Rect {
    Rect {
        x: r.x - by,
        y: r.y - by,
        w: r.w + 2.0 * by,
        h: r.h + 2.0 * by,
    }
}

/// Touched is a solid ring in the change's colour; holding dependents a dashed one in Corral's,
/// outside it when both, so the two differ in shape as well as colour.
fn rings(p: Painter, r: Rect, radius: f32, mark: Mark) -> Painter {
    let mut p = p;
    if mark.touched {
        p = p
            .stroke_color(now(hue_ink(TOUCHED)))
            .line_width(2.2)
            .begin_path()
            .round_rect(bounds(r), px(radius))
            .stroke();
    }
    if mark.dependents > 0 {
        let out = if mark.touched { 3.0 } else { 0.0 };
        p = p
            .stroke_color(now(hue_ink(DEPENDENT)))
            .line_width(1.6)
            .line_dash(&[px(5.0), px(3.0)])
            .begin_path()
            .round_rect(bounds(grown(r, out)), px(radius + out))
            .stroke()
            .line_dash(&[]);
    }
    p
}

/// The dependents a card holds, a chip in the dashed ring's colour on the row the overlay's bar
/// takes elsewhere, at the card's right.
fn dependents_chip(p: Painter, words: &str, r: Rect) -> Painter {
    let w = (measure(Font::Label, words) + 14.0).min(r.w - 24.0);
    let area = Rect {
        x: r.x + r.w - 12.0 - w,
        y: r.y + 42.0,
        w,
        h: 14.0,
    };
    p.fill_color(now(hue_fill(DEPENDENT)))
        .begin_path()
        .round_rect(bounds(area), px(7.0))
        .fill()
        .stroke_color(now(hue_ink(DEPENDENT)))
        .line_width(1.0)
        .line_dash(&[px(2.0), px(2.0)])
        .stroke()
        .line_dash(&[])
        .font(&css(Font::Label))
        .fill_color(now(hue_ink(DEPENDENT)))
        .text_align(CanvasTextAlign::Center)
        .fill_text(
            &fit(words, w - 14.0, Font::Label),
            area.x + w / 2.0,
            area.y + 10.5,
        )
}

/// The overlay's bar under the figure: the language mix, the activity relative to the busiest
/// here, or the files read, not read and failed.
fn bar(p: Painter, card: &Card, area: Rect, look: &Look, scene: &Scene) -> Painter {
    let mut p = p
        .fill_color(ink(Token::Rule))
        .fill_rect(area.x, area.y, area.w, area.h);
    match look.overlay {
        Overlay::Structure => {
            let mut parts: Vec<(Option<usize>, u64)> = Vec::new();
            if let Some(stats) = &card.stats {
                for (language, lines) in &stats.languages {
                    parts.push((language_slot(&look.languages, language), *lines));
                }
            } else if let Some((language, _, _)) = &card.file {
                parts.push((language_slot(&look.languages, language), card.lines));
            }
            parts.sort_by_key(|(slot, _)| slot.unwrap_or(usize::MAX));
            let total: u64 = parts.iter().map(|p| p.1).sum();
            if total == 0 {
                return p;
            }
            let mut x = area.x;
            for (slot, lines) in parts {
                let w = area.w * lines as f32 / total as f32;
                let fill = slot.map_or(ink(Token::Ink4), |s| now(hue_ink(slot_hue(s))));
                p = p.fill_color(fill).fill_rect(x, area.y, w, area.h);
                x += w;
            }
            p
        }
        Overlay::Activity => {
            let s = share(scene, card, Overlay::Activity, look.window);
            if s > 0.0 {
                p = p.fill_color(now(hue_ink(Hue::Worker))).fill_rect(
                    area.x,
                    area.y,
                    (area.w * s).max(2.0),
                    area.h,
                );
            }
            p
        }
        Overlay::Coverage => {
            let Some((read, unsupported, failed)) = read_shares(card) else {
                return p;
            };
            let mut x = area.x;
            for (share, fill) in [
                (read, ink(Token::Ink2)),
                (unsupported, ink(Token::Ink4).opacity(0.6)),
                (failed, now(hue_ink(Hue::Fail))),
            ] {
                let w = area.w * share;
                p = p.fill_color(fill).fill_rect(x, area.y, w, area.h);
                x += w;
            }
            p
        }
    }
}

/// The processes a package spawns, one chip each, then `+n` for those that do not fit.
fn spawn_chips(p: Painter, card: &Card, r: Rect) -> Painter {
    let y = r.y + 56.0;
    let mut p = p
        .font(&css(Font::Sub))
        .fill_color(ink(Token::Ink3))
        .text_align(CanvasTextAlign::Start)
        .fill_text("spawns", r.x + 12.0, y + 12.5);
    let mut x = r.x + 12.0 + measure(Font::Sub, "spawns") + 8.0;
    let end = r.x + r.w - 10.0;
    for (i, (process, sites)) in card.spawns.iter().enumerate() {
        let w = layout::chip_width(&measure, process, *sites);
        let rest = card.spawns.len() - i - 1;
        if i > 0 && x + w + if rest > 0 { 30.0 } else { 0.0 } > end {
            let more = format!("+{}", card.spawns.len() - i);
            let mw = measure(Font::Label, &more) + 14.0;
            return chip(
                p,
                Rect {
                    x,
                    y,
                    w: mw,
                    h: 17.0,
                },
            )
            .fill_color(ink(Token::Ink2))
            .text_align(CanvasTextAlign::Center)
            .fill_text(&more, x + mw / 2.0, y + 12.5);
        }
        p = chip(p, Rect { x, y, w, h: 17.0 })
            .font(&css(Font::Label))
            .fill_color(ink(Token::Ink2))
            .text_align(CanvasTextAlign::Start)
            .fill_text(process, x + 8.0, y + 12.5)
            .font(&css(Font::Tag))
            .fill_color(ink(Token::Ink3))
            .text_align(CanvasTextAlign::End)
            .fill_text(&sites.to_string(), x + w - 8.0, y + 12.5);
        x += w + 4.0;
    }
    p
}

fn chip(p: Painter, r: Rect) -> Painter {
    p.fill_color(ink(Token::BgSunk))
        .begin_path()
        .round_rect(bounds(r), px(8.5))
        .fill()
        .stroke_color(ink(Token::Rule2))
        .line_width(1.0)
        .stroke()
        .font(&css(Font::Label))
}
