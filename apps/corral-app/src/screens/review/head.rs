//! The selected candidate's head, its page notice and its SHA-bound verdicts.

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::review::{DetailView, Header, Notice, VerdictView};
use crate::model::{Form, Hue, Span, span};
use crate::state::AppState;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{qhead, section};
use crate::ui::kit::text::{dim, h3, mono};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::{Ctx, command, rich_at};

/// A struck verdict of an older candidate sits a step back (`opacity: .8`).
const OLD_VERDICT_OPACITY: f32 = 0.8;

/// A page notice (loading, stale, failed, superseded) with its refresh control.
pub(super) fn notice_row(cx: &Ctx, line: &Notice) -> AnyElement {
    let app = cx.app;
    let action = line.action.as_ref().map(|button| command(cx, button));
    let place = match app.form {
        Form::Desktop => view().px(px(24.0)).pb(px(8.0)),
        Form::Phone => view().px(px(14.0)).pb(px(8.0)),
        Form::Terminal => view().px(term::cells(1.0)),
    };
    place
        .child(notice(
            app,
            line.hue,
            &vec![span(line.text.clone())],
            action,
        ))
        .into_element()
}

/// The selected candidate's head (`.qhead`): the metadata, its tags, the title.
pub(super) fn header(app: AppState, header: &Header) -> AnyElement {
    let mut meta: Vec<AnyElement> = header
        .metadata
        .iter()
        .map(|run| meta_run(app, run))
        .collect();
    meta.extend(header.chips.iter().map(|tag| match tag.hue {
        Hue::Neutral => illus(app, tag.label.clone()),
        hue => chip(app, tag.label.clone(), hue),
    }));
    qhead(app, meta, header.title.clone(), None, Vec::new())
}

/// One run of the head's metadata: mono ids in mono, the rest as quiet text; the meta row
/// spaces the runs itself.
fn meta_run(app: AppState, run: &Span) -> AnyElement {
    let words = run.text.trim().to_owned();
    let ink = match run.hue {
        Hue::Neutral => color(Token::Ink3),
        hue => hue_ink(hue),
    };
    if run.mono {
        mono(app, words).text_color(ink).into_element()
    } else {
        dim(app, words).text_color(ink).into_element()
    }
}

/// The verdicts, each bound to the SHA it was given on.
pub(super) fn verdicts(app: AppState, detail: &DetailView) -> AnyElement {
    let form = app.form;
    let gap = match form {
        Form::Desktop => 5.0,
        Form::Phone => 8.0,
        Form::Terminal => 0.0,
    };
    let rows = view()
        .gap(px(gap))
        .children(
            detail
                .verdicts
                .iter()
                .map(|verdict| verdict_row(app, verdict)),
        )
        .children(
            detail
                .verdicts_empty
                .map(|line| dim(app, line).into_element()),
        )
        .into_element();
    let heading = detail.verdict_heading;
    match form {
        Form::Desktop => section(
            app,
            None,
            None,
            Hue::Neutral,
            view()
                .child(flush_h3(app, heading))
                .child(rows)
                .into_element(),
        ),
        Form::Phone => section(
            app,
            Some(heading.to_owned()),
            detail.verdict_binding.map(str::to_owned),
            Hue::Neutral,
            rows,
        ),
        Form::Terminal => section(app, Some(heading.to_owned()), None, Hue::Neutral, rows),
    }
}

/// `text::h3` keeps 18 pt above its caption (14 on a phone) for a caption inside a page; where
/// the mockup's caption opens its section (`.h3` at `margin-top: 0`) that room goes again.
pub(super) fn flush_h3(app: AppState, label: impl Into<SharedString>) -> AnyElement {
    let lift = match app.form {
        Form::Desktop => 18.0,
        Form::Phone => 14.0,
        Form::Terminal => 0.0,
    };
    view()
        .mt(px(-lift))
        .child(h3(app, label, None))
        .into_element()
}

/// One verdict. Desktop: the mockup's columns (mark 16, who 56, SHA 78, time 44, result).
/// Phone: the mark, then who · SHA time with the tag, the result under it. Terminal: one line.
fn verdict_row(app: AppState, verdict: &VerdictView) -> AnyElement {
    let form = app.form;
    let old = verdict.mark == Mark::Stale;
    let tag = verdict
        .tag
        .as_ref()
        .map(|tag| chip(app, tag.label.clone(), tag.hue));
    match form {
        Form::Desktop => view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .when(old, |row| row.opacity(OLD_VERDICT_OPACITY))
            .child(fixed(16.0, mark(app, verdict.mark, Hue::Neutral)))
            .child(fixed(
                56.0,
                text(verdict.who.clone())
                    .truncate()
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            ))
            .child(fixed(
                78.0,
                rich_at(app, &verdict.sha, 11.5, Token::Ink, true).truncate(),
            ))
            .child(fixed(
                44.0,
                mono(app, verdict.time.clone()).text_color(color(Token::Ink3)),
            ))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(rich_at(app, &verdict.text, 12.5, Token::Ink, false))
                    .children(tag),
            )
            .into_element(),
        Form::Phone => view()
            .flex_row()
            .items_start()
            .gap(px(8.0))
            .when(old, |row| row.opacity(OLD_VERDICT_OPACITY))
            .child(view().w(px(16.0)).flex_shrink_0().pt(px(3.0)).child(mark(
                app,
                verdict.mark,
                Hue::Neutral,
            )))
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
                            .gap(px(6.0))
                            .child(rich_at(app, &verdict.line, 13.0, Token::Ink, false))
                            .children(tag),
                    )
                    .child(rich_at(app, &verdict.text, 13.0, Token::Ink, false)),
            )
            .into_element(),
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .child(mark(app, verdict.mark, Hue::Neutral))
            .child(term::space(1.0))
            .child(
                view().w(term::cells(7.0)).flex_shrink_0().child(
                    text(verdict.who.clone())
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                ),
            )
            .child(
                view()
                    .w(term::cells(10.0))
                    .flex_shrink_0()
                    .child(rich_at(app, &verdict.sha, 0.0, Token::Ink, true).truncate()),
            )
            .child(
                view()
                    .w(term::cells(12.0))
                    .flex_shrink_0()
                    .child(term::quiet(verdict.time.clone())),
            )
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(rich_at(app, &verdict.text, 0.0, Token::Ink, false).truncate()),
            )
            .when(tag.is_some(), |row| row.child(term::space(1.0)))
            .children(tag)
            .into_element(),
    }
}

/// A fixed column of a desktop row.
fn fixed(width: f32, content: impl IntoElement) -> AnyElement {
    view()
        .w(px(width))
        .flex_shrink_0()
        .child(content)
        .into_element()
}
