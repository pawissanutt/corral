//! Activity › Feed: every event in the one project scope, newest first, under a heading per
//! local day, narrowed by the Feed's own filters (`feed_filter.rs`). A row leads with what
//! happened (a category mark and a readable label; a blocked, failed or forced event also
//! names its alert in a chip and carries an accent, so it does not rely on colour), then the
//! task's title and the event's detail, with the time, task id and who acted quiet beside them;
//! pressing it opens the task's page. Load older fetches the page before the oldest loaded row
//! whatever the filters; Mark seen in the head moves this device's seen cursor, which Home's
//! "since your last visit" counts from. Every word is `work::feed_page`'s. test ids `feed`,
//! `feed-filters`, `feed-no-match`, `event-<seq>`, `btn-mark-seen`, `btn-older`,
//! `btn-feed-reset`.

use nsh_core::prelude::*;

use super::feed_filter;
use crate::model::work::{
    self, Alert, Category, FEED_RESET, FEED_SUB, FEED_TITLE, FeedDay, FeedEvent, FeedFilter,
    FeedPage,
};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::kit::page::{group_head, measure, measured_head, pane};
use crate::ui::kit::text::dim;
use crate::ui::kit::work::pressable;
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, MEASURE_PAGE, Token, color};

const RING: f32 = 2.0;

/// The Feed on every form: the head with Mark seen, the filters, then the days in one scroll.
/// The phone's shell heads the pushed page; there the filters lead its scrolling body. A
/// desktop keeps head, filters and rows to [`MEASURE_PAGE`], centred, as every Activity page
/// does, so the title does not move between them; the scroll spans the window, so its bar
/// stays at the edge.
pub(super) fn screen(app: AppState) -> AnyElement {
    let page = computed(move || {
        let scope = app.project_scope();
        let filter = app.work.with(|state| state.feed.clone());
        app.feed.with(|feed| work::feed_page(feed, &scope, &filter))
    });
    let stale = computed(move || page.with(|page| page.status.dimmed));
    let counted = computed(move || page.with(|page| page.counted.clone()));
    let rows = view()
        .style_when(move || stale.get(), |s| s.saturate(0.35).opacity(0.78))
        .child(dynamic(move || page.with(|page| days(app, page))))
        .into_element();
    let filters = feed_filter::bar(app, counted);
    match app.form {
        Form::Phone => pane(
            app,
            "feed",
            view().child(filters).child(rows).into_element(),
        ),
        Form::Terminal => view()
            .flex_1()
            .min_h(px(0.0))
            .child(head(app, page))
            .child(filters)
            .child(pane(app, "feed", rows))
            .into_element(),
        Form::Desktop => view()
            .flex_1()
            .min_h(px(0.0))
            .child(head(app, page))
            .child(measure(app, MEASURE_PAGE, filters))
            .child(pane(app, "feed", measure(app, MEASURE_PAGE, rows)))
            .into_element(),
    }
}

/// The page's head: its title and Mark seen.
fn head(app: AppState, page: Memo<FeedPage>) -> AnyElement {
    let mark = computed(move || page.with(|page| page.mark_seen.clone()));
    dynamic(move || {
        let actions = mark
            .get()
            .map(|button| pressable(app, &button, false))
            .into_iter()
            .collect();
        untracked(|| {
            measured_head(
                app,
                MEASURE_PAGE,
                FEED_TITLE,
                Some(FEED_SUB.to_owned()),
                actions,
            )
        })
    })
    .into_element()
}

/// The days, or the notice in their place (or, when the filters keep nothing loaded, the
/// no-match line with Clear filters), then Load older while corrald has more.
fn days(app: AppState, page: &FeedPage) -> AnyElement {
    let form = app.form;
    let side = if form == Form::Terminal {
        cells(1.0)
    } else {
        px(20.0)
    };
    let pad = px(if form == Form::Terminal { 0.0 } else { 12.0 });
    let notice = page.notice.clone().map(|words| {
        view()
            .px(side)
            .py(pad)
            .child(dim(app, words))
            .into_element()
    });
    let no_match = page.no_match.clone().map(|words| {
        view()
            .test_id("feed-no-match")
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(if form == Form::Terminal { 8.0 } else { 12.0 }))
            .px(side)
            .py(pad)
            .child(dim(app, words))
            .child(button(
                app,
                FEED_RESET,
                ButtonKind::Ghost,
                "btn-feed-reset-empty",
                move || app.work.update(|state| state.feed = FeedFilter::default()),
            ))
            .into_element()
    });
    view()
        .children(notice)
        .children(no_match)
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

/// The hue an event is marked in: its alert's, else its category's.
fn hue(event: &FeedEvent) -> Hue {
    if event.alert.is_some() {
        return Hue::Fail;
    }
    match event.category {
        Category::Task | Category::Note => Hue::Neutral,
        Category::Run => Hue::Worker,
        Category::Review => Hue::Corral,
        Category::Problem => Hue::Fail,
    }
}

/// The icon a graphical row leads with: the alert's shape, else the category's.
fn glyph(event: &FeedEvent) -> Icon {
    match (event.alert, event.category) {
        (Some(Alert::Failed), _) => Icon::Failed,
        (Some(Alert::Blocked), _) => Icon::Held,
        (Some(Alert::Forced), _) => Icon::NeedsYou,
        (None, Category::Task) => Icon::Plans,
        (None, Category::Run) => Icon::Moving,
        (None, Category::Review) => Icon::Review,
        (None, Category::Note) => Icon::Files,
        (None, Category::Problem) => Icon::Held,
    }
}

/// The Cell a terminal row leads with: `!` for an alert, else the category's mark.
fn cell_glyph(event: &FeedEvent) -> &'static str {
    if event.alert.is_some() {
        return "!";
    }
    match event.category {
        Category::Task => "○",
        Category::Run => "▸",
        Category::Review => "◇",
        Category::Note => "≡",
        Category::Problem => "!",
    }
}

/// Quiet words: the time, the id, who.
fn quiet(form: Form, words: impl Into<SharedString>) -> Text {
    let words = text(words.into())
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(if form == Form::Terminal {
            Token::Ink3
        } else {
            Token::Ink4
        }));
    if form == Form::Terminal {
        words
    } else {
        words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(if form == Form::Phone { 12.0 } else { 11.0 }))
    }
}

/// `T-12 · agent`, or who alone for an event about no task.
fn task_and_who(event: &FeedEvent) -> String {
    if event.id.is_empty() {
        event.who.to_owned()
    } else {
        format!("{} · {}", event.id, event.who)
    }
}

/// A fixed column in a terminal row, `width` Cells.
fn cell_column(width: f32, child: impl IntoElement) -> AnyElement {
    view()
        .w(cells(width))
        .flex_shrink_0()
        .flex_row()
        .min_w(px(0.0))
        .child(child)
        .into_element()
}

/// The category's mark in a tinted disc (desktop 24 pt, phone 28 pt).
fn mark(event: &FeedEvent, size: f32) -> AnyElement {
    let hue = hue(event);
    view()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .rounded(px(size / 2.0))
        .items_center()
        .justify_center()
        .bg(if hue == Hue::Neutral {
            color(Token::BgSunk)
        } else {
            hue_fill(hue)
        })
        .child(icon(
            glyph(event),
            size * 0.55,
            if hue == Hue::Neutral {
                color(Token::Ink3)
            } else {
                hue_ink(hue)
            },
        ))
        .into_element()
}

/// The label, and the alert's chip when there is one.
fn headline(app: AppState, event: &FeedEvent, size: f32) -> AnyElement {
    let ink = if event.alert.is_some() {
        hue_ink(Hue::Fail)
    } else {
        color(Token::Ink)
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .flex_shrink_0()
        .child(
            text(event.label.clone())
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink),
        )
        // A headline that already names the alert (`Blocked`, `Gate failed`) needs no chip.
        .children(
            event
                .alert
                .filter(|alert| {
                    !event
                        .label
                        .to_lowercase()
                        .contains(&alert.label().to_lowercase())
                })
                .map(|alert| chip(app, alert.label(), Hue::Fail)),
        )
        .into_element()
}

/// One event. Desktop: a 3 pt accent for an alert, the mark, the headline beside the task's
/// title over the detail, and the time over the id and who at the trailing edge, a hairline
/// above. Phone: the mark, then headline, title, detail and the quiet time, id and who, each
/// wrapping. Terminal: time, mark, label (upper case for an alert), id, title and detail, who.
fn row(app: AppState, event: &FeedEvent) -> AnyElement {
    let form = app.form;
    let base = view()
        .flex_row()
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
    let accent = |width: f32| {
        view()
            .w(px(width))
            .self_stretch()
            .flex_shrink_0()
            .rounded(px(width / 2.0))
            .bg(if event.alert.is_some() {
                hue_ink(Hue::Fail)
            } else {
                AdaptiveColor::TRANSPARENT
            })
    };
    match form {
        Form::Desktop => {
            let title = event.title.clone().map(|title| {
                text(title)
                    .flex_1()
                    .min_w(px(0.0))
                    .number_of_lines(1u32)
                    .truncate()
                    .font_family(FONT_TEXT)
                    .text_size(px(13.0))
                    .text_color(color(Token::Ink2))
            });
            let detail = (!event.text.is_empty()).then(|| {
                text(event.text.clone())
                    .number_of_lines(1u32)
                    .truncate()
                    .font_family(FONT_TEXT)
                    .text_size(px(12.0))
                    .text_color(color(Token::Ink3))
            });
            base.items_center()
                .min_h(px(44.0))
                .pr(px(20.0))
                .pl(px(14.0))
                .py(px(6.0))
                .gap(px(10.0))
                .border_t_1()
                .border_color(color(Token::Rule))
                .hover(|s| s.bg(color(Token::Hover)))
                .child(accent(3.0))
                .child(mark(event, 24.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(10.0))
                                .min_w(px(0.0))
                                .child(headline(app, event, 13.0))
                                .children(title),
                        )
                        .children(detail),
                )
                .child(
                    view()
                        .w(px(120.0))
                        .flex_shrink_0()
                        .items_end()
                        .gap(px(2.0))
                        .child(quiet(form, event.at.clone()))
                        .child(quiet(form, task_and_who(event))),
                )
                .into_element()
        }
        Form::Phone => {
            let title = event.title.clone().map(|title| {
                text(title)
                    .font_family(FONT_TEXT)
                    .text_size(px(14.0))
                    .line_height(px(19.0))
                    .text_color(color(Token::Ink))
            });
            let detail = (!event.text.is_empty()).then(|| {
                text(event.text.clone())
                    .font_family(FONT_TEXT)
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(color(Token::Ink2))
            });
            base.items_start()
                .min_h(px(44.0))
                .pl(px(8.0))
                .pr(px(14.0))
                .py(px(8.0))
                .gap(px(10.0))
                .border_t_1()
                .border_color(color(Token::Rule))
                .active(|s| s.bg(color(Token::Hover)))
                .child(accent(3.0))
                .child(mark(event, 28.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(3.0))
                        .child(headline(app, event, 14.5))
                        .children(title)
                        .children(detail)
                        .child(quiet(
                            form,
                            format!("{} · {}", event.at, task_and_who(event)),
                        )),
                )
                .into_element()
        }
        Form::Terminal => {
            let alert = event.alert.is_some();
            let label = if alert {
                event.label.to_uppercase()
            } else {
                event.label.clone()
            };
            let ink = if alert {
                hue_ink(Hue::Fail)
            } else {
                color(Token::Ink)
            };
            let rest = [event.title.as_deref().unwrap_or_default(), &event.text]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            base.items_center()
                .gap(px(8.0))
                .px(cells(1.0))
                .child(cell_column(5.0, quiet(form, event.at.clone())))
                .child(cell_column(
                    1.0,
                    text(cell_glyph(event))
                        .text_color(ink)
                        .when(alert, |t| t.font_weight(FontWeight::BOLD)),
                ))
                .child(cell_column(
                    17.0,
                    text(label)
                        .number_of_lines(1u32)
                        .truncate()
                        .text_color(ink)
                        .font_weight(if alert {
                            FontWeight::BOLD
                        } else {
                            FontWeight::NORMAL
                        }),
                ))
                .child(cell_column(7.0, quiet(form, event.id.clone())))
                .child(
                    text(rest)
                        .flex_1()
                        .min_w(px(0.0))
                        .number_of_lines(1u32)
                        .truncate()
                        .text_color(color(Token::Ink)),
                )
                .child(cell_column(
                    9.0,
                    view()
                        .flex_1()
                        .flex_row()
                        .justify_end()
                        .child(quiet(form, event.who)),
                ))
                .into_element()
        }
    }
}
