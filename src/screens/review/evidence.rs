//! The Report, Screenshot and Gate log tabs: the rendered report (`.plan-doc`), and the terminal
//! captures (`.term`) with their provenance lines.

use nsh_core::prelude::*;

use crate::model::review::{LogLine, ReportView, ScreenshotView};
use crate::model::{Form, Hue, Span};
use crate::state::AppState;
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::text::{h3, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color, text_size};

use super::{flush_h3, rich_at};

/// `.plan-doc p { max-width: 72ch }` at the body size.
const DOC_MAX: f32 = 560.0;
/// The screenshot capture's widest (`max-width: 640px`).
const SHOT_MAX: f32 = 640.0;
/// `.term`'s line height.
const TERM_LEADING: f32 = 1.55;
/// The capture's cursor block (`.term .cur`).
const CURSOR: (f32, f32) = (7.0, 13.0);

/// The rendered implementation report: heading, paragraphs, the acceptance list with its marks,
/// and the source line.
pub(super) fn report(app: AppState, report: &ReportView) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let doc = report.document;
    let leading = (text_size(form) * TERM_LEADING).round();
    let paragraph = |words: &'static str| {
        plain(app, words)
            .when(!terminal, |t| t.line_height(px(leading)))
            .into_element()
    };
    let items = view()
        .gap(px(if terminal { 0.0 } else { 4.0 }))
        .map(|list| {
            if terminal {
                list.pl(term::cells(2.0))
            } else {
                list.pl(px(18.0))
            }
        })
        .children(doc.acceptance.iter().map(|item| {
            view()
                .flex_row()
                .items_start()
                .gap(px(8.0))
                .child(
                    view()
                        .flex_shrink_0()
                        .when(!terminal, |m| m.pt(px(3.0)))
                        .child(mark(app, item.mark, Hue::Neutral)),
                )
                .when(terminal, |row| row.child(term::space(1.0)))
                .child(paragraph(item.text))
                .into_element()
        }));
    let mut parts = vec![flush_h3(app, doc.heading)];
    if terminal {
        parts.push(term::blank());
    }
    parts.extend(doc.paragraphs.iter().map(|&words| paragraph(words)));
    if terminal {
        parts.push(term::blank());
    }
    parts.push(h3(app, doc.acceptance_heading, None));
    if terminal {
        parts.push(term::blank());
    }
    parts.push(items.into_element());
    parts.push(if terminal {
        term::blank()
    } else {
        view().h(px(10.0)).into_element()
    });
    parts.push(rich_at(app, &report.source, 12.0, Token::Ink3, false).into_element());
    view()
        .when(!terminal, |doc| doc.max_w(px(DOC_MAX)))
        .children(parts)
        .into_element()
}

/// The terminal capture with its cursor, then the artifact caption.
pub(super) fn screenshot(app: AppState, shot: &ScreenshotView) -> AnyElement {
    let spans = shot
        .terminal
        .iter()
        .map(|run| capture_span(app, run))
        .collect();
    view()
        .child(console(app, spans, 11.0, shot.cursor, Some(SHOT_MAX)))
        .child(below(app))
        .child(rich_at(app, &shot.caption, 12.0, Token::Ink3, false))
        .into_element()
}

/// The gate log, its passing lines in the success colour, then its illustrative label.
pub(super) fn log(app: AppState, lines: &[LogLine], note: &'static str) -> AnyElement {
    let ink = console_ink(app);
    let mut spans = Vec::with_capacity(lines.len() * 2);
    for (at, line) in lines.iter().enumerate() {
        if at > 0 {
            spans.push(text("\n"));
        }
        spans.push(text(line.text).text_color(match line.hue {
            Hue::Neutral => ink,
            hue => hue_ink(hue),
        }));
    }
    view()
        .child(console(app, spans, 11.5, false, None))
        .child(below(app))
        .child(illus(app, note))
        .into_element()
}

/// The room between a capture and its caption: 6 pt, a row in a terminal.
fn below(app: AppState) -> AnyElement {
    if app.form == Form::Terminal {
        term::blank()
    } else {
        view().h(px(6.0)).into_element()
    }
}

/// `.term`'s words: the shell ink, which stays light on the always-dark capture; a terminal's
/// own ink there.
fn console_ink(app: AppState) -> AdaptiveColor {
    color(if app.form == Form::Terminal {
        Token::Ink
    } else {
        Token::ShellInk
    })
}

/// One run of the capture: bold runs (`.w8`) bold in the capture's secondary ink, a hue in its
/// colour.
fn capture_span(app: AppState, run: &Span) -> Text {
    let terminal = app.form == Form::Terminal;
    let mut piece = text(run.text.clone());
    if run.bold {
        piece = piece.font_weight(FontWeight::BOLD);
        if !terminal {
            piece = piece.text_color(color(Token::ShellInk2));
        }
    }
    if run.hue != Hue::Neutral {
        piece = piece.text_color(hue_ink(run.hue));
    }
    piece
}

/// A terminal capture (`.term`): preformatted mono lines on the shell's dark ground, radius 6,
/// padding 10 12, the cursor block after the last line when there is one. A terminal frames the
/// lines instead and shows the cursor as a block glyph.
fn console(
    app: AppState,
    spans: Vec<Text>,
    size: f32,
    cursor: bool,
    widest: Option<f32>,
) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let ink = console_ink(app);
    let body = text("").text_color(ink).children(spans);
    if terminal {
        let body = body.when(cursor, |t| t.child(text("█").text_color(ink)));
        return term::framed(view().px(term::cells(1.0)).child(body)).into_element();
    }
    let body = body
        .font_family(FONT_MONO)
        .text_size(px(size))
        .line_height(px((size * TERM_LEADING).round()))
        .when(cursor, |t| {
            t.child(
                view()
                    .w(px(CURSOR.0))
                    .h(px(CURSOR.1))
                    .bg(ink)
                    .accessibility_elements_hidden(true),
            )
        });
    view()
        .when_some(widest, |capture, widest| capture.max_w(px(widest)))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(6.0))
        .bg(color(Token::Shell))
        .overflow_hidden()
        .child(body)
        .into_element()
}
