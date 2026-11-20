//! Activity › Feed: every event in the one project scope, newest first, under a heading per
//! local day. A row (`.ev`) is the time, the engine's kind (a forced operation's in the failure
//! colour), the task's id, what the event says and who did it; pressing it opens the task's
//! page. Load older fetches the page before the oldest row; Mark seen in the head moves this
//! device's seen cursor, which Home's "since your last visit" counts from. Every word is
//! `work::feed_page`'s. test ids `feed`, `event-<seq>`, `btn-mark-seen`, `btn-older`.

use nsh_core::prelude::*;

use crate::model::work::{self, FEED_SUB, FEED_TITLE, FeedDay, FeedEvent, FeedPage};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::{group_head, head, pane};
use crate::ui::kit::text::dim;
use crate::ui::kit::work::pressable;
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::subnav;

const RING: f32 = 2.0;

/// The Feed on every form: the head with Mark seen, the strip, then the days in one scroll.
/// The phone's shell heads the pushed page; there this is its scrolling body.
pub(super) fn screen(app: AppState) -> AnyElement {
    let page = computed(move || {
        let scope = app.project_scope();
        app.feed.with(|feed| work::feed_page(feed, &scope))
    });
    let stale = computed(move || page.with(|page| page.status.dimmed));
    let body = view()
        .style_when(move || stale.get(), |s| s.saturate(0.35).opacity(0.78))
        .child(dynamic(move || page.with(|page| days(app, page))))
        .into_element();
    if app.form == Form::Phone {
        return pane(app, "feed", body);
    }
    let mark = computed(move || page.with(|page| page.mark_seen.clone()));
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(dynamic(move || {
            let actions = mark
                .get()
                .map(|button| pressable(app, &button, false))
                .into_iter()
                .collect();
            untracked(|| head(app, FEED_TITLE, Some(FEED_SUB.to_owned()), actions))
        }))
        .children(subnav(app))
        .child(pane(app, "feed", body))
        .into_element()
}

/// The days, or the notice in their place, then Load older while corrald has more.
fn days(app: AppState, page: &FeedPage) -> AnyElement {
    let form = app.form;
    let side = if form == Form::Terminal {
        cells(1.0)
    } else {
        px(20.0)
    };
    let notice = page.notice.clone().map(|words| {
        view()
            .px(side)
            .py(px(if form == Form::Terminal { 0.0 } else { 12.0 }))
            .child(dim(app, words))
            .into_element()
    });
    view()
        .children(notice)
        .children(
            page.days
                .iter()
                .map(|day| day_block(app, day))
                .collect::<Vec<_>>(),
        )
        .children(page.older.as_ref().map(|older| {
            view()
                .flex_row()
                .px(side)
                .py(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
                .child(pressable(app, older, false))
                .into_element()
        }))
        .into_element()
}

fn day_block(app: AppState, day: &FeedDay) -> AnyElement {
    view()
        .role(AccessibilityRole::List)
        .child(group_head(
            app,
            day.label.clone(),
            Some(day.events.len().to_string()),
            Hue::Neutral,
            None,
            None,
        ))
        .children(
            day.events
                .iter()
                .map(|event| row(app, event))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// Quiet mono words in a fixed column: the time, the id, who.
fn quiet_mono(form: Form, words: impl Into<SharedString>) -> Text {
    let words = text(words.into())
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(Token::Ink3));
    if form == Form::Terminal {
        words
    } else {
        words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(if form == Form::Phone { 12.5 } else { 11.5 }))
    }
}

/// A column `points` wide on desktop and phone, `terminal` Cells in a terminal.
fn column(form: Form, points: f32, terminal: f32, child: impl IntoElement) -> AnyElement {
    view()
        .w(if form == Form::Terminal {
            cells(terminal)
        } else {
            px(points)
        })
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .child(child)
        .into_element()
}

/// One event (`.ev`). Desktop: 30 pt, time 96, kind 110, id 70, the text cut to fit, who 80 at
/// the trailing edge, a hairline above. Terminal: the same in Cells (5, 14, 7, the rest, 9).
/// Phone (`.phone .ev`): the time beside the kind and id in bold before the text, which wraps.
fn row(app: AppState, event: &FeedEvent) -> AnyElement {
    let form = app.form;
    let kind_ink = if event.forced {
        hue_ink(Hue::Fail)
    } else {
        color(Token::Ink)
    };
    let kind = text(event.kind)
        .number_of_lines(1u32)
        .truncate()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(kind_ink);
    let words = text(event.text.clone())
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(if form == Form::Terminal {
            Token::Ink
        } else {
            Token::Ink2
        }));
    let base = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .test_id(&event.test_id)
        .label(event.spoken());
    let base = match event.open.clone() {
        Some(live) => base
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .on_press(move || app.open_live(live.clone()))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            }),
        None => base,
    };
    match form {
        Form::Desktop => base
            .min_h(px(30.0))
            .px(px(20.0))
            .gap(px(12.0))
            .border_t_1()
            .border_color(color(Token::Rule))
            .hover(|s| s.bg(color(Token::Hover)))
            .child(column(form, 96.0, 0.0, quiet_mono(form, event.at.clone())))
            .child(column(
                form,
                110.0,
                0.0,
                kind.font_family(FONT_TEXT).text_size(px(12.5)),
            ))
            .child(column(form, 70.0, 0.0, quiet_mono(form, event.id.clone())))
            .child(
                words
                    .number_of_lines(1u32)
                    .truncate()
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5)),
            )
            .child(column(
                form,
                80.0,
                0.0,
                view()
                    .flex_1()
                    .flex_row()
                    .justify_end()
                    .child(quiet_mono(form, event.who)),
            ))
            .into_element(),
        Form::Terminal => base
            .gap(px(8.0))
            .px(cells(1.0))
            .child(column(form, 0.0, 5.0, quiet_mono(form, event.at.clone())))
            .child(column(form, 0.0, 14.0, kind))
            .child(column(form, 0.0, 7.0, quiet_mono(form, event.id.clone())))
            .child(words.number_of_lines(1u32).truncate())
            .child(column(
                form,
                0.0,
                9.0,
                view()
                    .flex_1()
                    .flex_row()
                    .justify_end()
                    .child(quiet_mono(form, event.who)),
            ))
            .into_element(),
        Form::Phone => base
            .items_start()
            .min_h(px(44.0))
            .px(px(14.0))
            .py(px(6.0))
            .gap(px(8.0))
            .border_t_1()
            .border_color(color(Token::Rule))
            .active(|s| s.bg(color(Token::Hover)))
            .child(column(form, 52.0, 0.0, quiet_mono(form, event.at.clone())))
            .child(
                text("")
                    .flex_1()
                    .min_w(px(0.0))
                    .font_family(FONT_TEXT)
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .child(
                        text(format!("{} {}  ", event.kind, event.id))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(kind_ink),
                    )
                    .child(text(event.text.clone()).text_color(color(Token::Ink2))),
            )
            .into_element(),
    }
}
