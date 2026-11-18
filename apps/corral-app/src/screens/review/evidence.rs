//! The Summary, Artifacts and Gate log tabs: the worker run's summary (`.plan-doc`), the gate
//! log's terminal capture (`.term`) with its provenance line, and the task's artifacts.

use nsh_core::prelude::*;

use crate::model::review::{ArtifactContent, ArtifactView, ArtifactsView, LogView, ReportView};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::{dim, h3_flush, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color, text_size};

use super::{Ctx, command, rich_at};

/// `.plan-doc p { max-width: 72ch }` at the body size.
const DOC_MAX: f32 = 560.0;
/// The screenshot capture's widest (`max-width: 640px`).
const SHOT_MAX: f32 = 640.0;
/// `.term`'s line height.
const TERM_LEADING: f32 = 1.55;
/// The capture's cursor block (`.term .cur`).
const CURSOR: (f32, f32) = (7.0, 13.0);
/// An image's frame before it has loaded and told its own proportions.
const IMAGE_RATIO: f32 = 16.0 / 9.0;

/// The worker run's summary: heading, paragraphs and the source line.
pub(super) fn report(app: AppState, report: &ReportView) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let leading = (text_size(form) * TERM_LEADING).round();
    let paragraph = |words: &str| {
        plain(app, words.to_owned())
            .when(!terminal, |t| t.line_height(px(leading)))
            .into_element()
    };
    let gap = || {
        if terminal {
            term::blank()
        } else {
            view().h(px(10.0)).into_element()
        }
    };
    let mut parts = vec![h3_flush(app, report.heading.clone(), None)];
    if terminal {
        parts.push(term::blank());
    }
    for (at, words) in report.paragraphs.iter().enumerate() {
        if at > 0 {
            parts.push(gap());
        }
        parts.push(paragraph(words.as_str()));
    }
    parts.push(gap());
    parts.push(rich_at(app, &report.source, 12.0, Token::Ink3, false).into_element());
    view()
        .when(!terminal, |doc| doc.max_w(px(DOC_MAX)))
        .children(parts)
        .into_element()
}

/// The gate log, its passing lines in the success colour and failing ones in the failure
/// colour, then the served tail's extent.
pub(super) fn log(app: AppState, lines: &[LogView], note: &str) -> AnyElement {
    let ink = console_ink(app);
    let mut spans = Vec::with_capacity(lines.len() * 2);
    for (at, line) in lines.iter().enumerate() {
        if at > 0 {
            spans.push(text("\n"));
        }
        spans.push(text(line.text.clone()).text_color(match line.hue {
            Hue::Neutral => ink,
            hue => hue_ink(hue),
        }));
    }
    view()
        .child(console(app, spans, 11.5, false, None))
        .child(below(app))
        .child(dim(app, note.to_owned()))
        .into_element()
}

/// The candidate's artifacts: each record's title, type and catalog key with Show or Hide, and
/// once shown its contents; then where those contents come from.
pub(super) fn artifacts(cx: &Ctx, list: &ArtifactsView) -> AnyElement {
    let app = cx.app;
    let terminal = app.form == Form::Terminal;
    view()
        .gap(px(if terminal { 0.0 } else { 12.0 }))
        .children(list.empty.map(|line| dim(app, line).into_element()))
        .children(list.rows.iter().map(|row| artifact(cx, row)))
        .when(terminal, |rows| rows.child(term::blank()))
        .child(dim(app, list.note))
        .into_element()
}

fn artifact(cx: &Ctx, row: &ArtifactView) -> AnyElement {
    let app = cx.app;
    let terminal = app.form == Form::Terminal;
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(if terminal { 0.0 } else { 8.0 }))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    plain(app, row.title.clone())
                        .truncate()
                        .font_weight(FontWeight::BOLD),
                )
                .child(
                    view()
                        .flex_row()
                        .gap(px(6.0))
                        .child(dim(app, row.kind.clone()).flex_shrink_0())
                        .child(
                            mono(app, row.source.clone())
                                .truncate()
                                .text_color(color(Token::Ink3)),
                        ),
                ),
        )
        .child(command(cx, &row.toggle));
    let body = match &row.content {
        ArtifactContent::Closed => None,
        ArtifactContent::Loading => Some(dim(app, "Loading from corrald…").into_element()),
        ArtifactContent::Error(line) => Some(
            dim(app, line.clone())
                .text_color(hue_ink(Hue::Fail))
                .into_element(),
        ),
        ArtifactContent::Unsupported(line) => Some(dim(app, *line).into_element()),
        ArtifactContent::Text {
            text: words,
            truncated,
        } => Some(
            view()
                .child(console(app, vec![text(words.clone())], 11.0, false, None))
                .children(truncated.then(|| cut_note(app)))
                .into_element(),
        ),
        ArtifactContent::Image { uri, truncated } => Some(
            view()
                .child(picture(row.title.clone(), uri.clone()))
                .children(truncated.then(|| cut_note(app)))
                .into_element(),
        ),
    };
    view()
        .gap(px(if terminal { 0.0 } else { 6.0 }))
        .child(head)
        .children(body)
        .into_element()
}

fn cut_note(app: AppState) -> AnyElement {
    view()
        .mt(px(4.0))
        .child(dim(
            app,
            "corrald stops this artifact at its size bound; the rest is not served to the app.",
        ))
        .into_element()
}

/// An image artifact at its own proportions, at most the capture's width, on a hairline frame.
fn picture(title: String, uri: String) -> AnyElement {
    let ratio = signal(IMAGE_RATIO);
    view()
        .max_w(px(SHOT_MAX))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::BgSunk))
        .overflow_hidden()
        .child(
            image(uri)
                .w_full()
                .aspect_ratio(move || ratio.get())
                .object_fit(ResizeMode::Contain)
                .role(AccessibilityRole::Image)
                .label(title)
                .on_load(move |size: &Size<DevicePixels>| {
                    if size.width.0 > 0 && size.height.0 > 0 {
                        ratio.set(size.width.0 as f32 / size.height.0 as f32);
                    }
                }),
        )
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
