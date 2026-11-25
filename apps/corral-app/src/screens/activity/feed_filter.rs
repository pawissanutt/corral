//! Activity › Feed's filter bar: the search field (`field-feed`), the category options
//! (`seg-feed-category-<all|task|run|review|note|problem>`), who acted
//! (`seg-feed-actor-<all|owner|agent|automation>`) and, while any is set, how many loaded rows
//! pass and Clear filters (`btn-feed-reset`). They write `work::State.feed` only; the scope,
//! Mark seen and Load older stay apart.
//!
//! Desktop: one wrapping toolbar row under the head. Phone: the field over wrapping rows of
//! options. Terminal: the field and options on as few rows as the width allows.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::work::{Category, FEED_RESET, FEED_SEARCH, FeedFilter, Who};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::filters::{Size, option, search_field, track};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

fn set(app: AppState, change: impl FnOnce(&mut FeedFilter)) {
    app.work.update(|state| change(&mut state.feed));
}

/// One toolbar of options: `All` (or `Anyone`) then each value, the chosen one pressed. It is
/// built once and reads `chosen` reactively, so choosing keeps the keyboard on the option.
fn options<T: Copy + PartialEq + 'static>(
    app: AppState,
    name: &'static str,
    group: &'static str,
    every: &'static str,
    values: &[(T, &'static str, &'static str)],
    chosen: impl Fn() -> Option<T> + Copy + 'static,
    pick: impl Fn(Option<T>) + Copy + 'static,
) -> AnyElement {
    let all = option(
        app,
        Size::Bar,
        every.to_owned(),
        None,
        move || chosen().is_none(),
        format!("seg-feed-{group}-all"),
        move || pick(None),
    );
    let rest = values.iter().map(|&(value, label, slug)| {
        option(
            app,
            Size::Bar,
            label.to_owned(),
            None,
            move || chosen() == Some(value),
            format!("seg-feed-{group}-{slug}"),
            move || pick(Some(value)),
        )
    });
    track(
        app,
        Size::Bar,
        name,
        std::iter::once(all).chain(rest).collect(),
    )
}

/// The filter bar, `counted` the line saying how many loaded rows pass.
pub(super) fn bar(app: AppState, counted: Memo<Option<String>>) -> AnyElement {
    let form = app.form;
    let field = search_field(
        app,
        "field-feed",
        FEED_SEARCH,
        move || app.work.with(|state| state.feed.search.clone()),
        move |typed| set(app, |filter| typed.clone_into(&mut filter.search)),
    );
    let category = computed(move || app.work.with(|state| state.feed.category));
    let who = computed(move || app.work.with(|state| state.feed.who));
    let categories: Vec<_> = Category::ALL
        .into_iter()
        .map(|value| (value, value.label(), value.slug()))
        .collect();
    let actors: Vec<_> = Who::ALL
        .into_iter()
        .map(|value| (value, value.label(), value.slug()))
        .collect();
    // On the phone each group takes the row's width, so its options wrap inside it.
    let choices = view()
        .when(form != Form::Phone, |row| {
            row.flex_row().flex_wrap().items_center()
        })
        .when(form == Form::Terminal, |row| row.gap_x(term::cells(2.0)))
        .when(form == Form::Desktop, |row| row.gap(px(8.0)))
        .when(form == Form::Phone, |column| column.gap(px(12.0)))
        .child(options(
            app,
            "Event category",
            "category",
            "All",
            &categories,
            move || category.get(),
            move |value| set(app, |filter| filter.category = value),
        ))
        .child(options(
            app,
            "Who acted",
            "actor",
            "Anyone",
            &actors,
            move || who.get(),
            move |value| set(app, |filter| filter.who = value),
        ));
    let reset = dynamic(move || {
        let counted = counted.get();
        untracked(move || {
            let Some(words) = counted else {
                return view().into_element();
            };
            view()
                .flex_row()
                .items_center()
                .gap(px(if form == Form::Terminal { 8.0 } else { 10.0 }))
                .child(dim(app, words))
                .child(button(
                    app,
                    FEED_RESET,
                    ButtonKind::Ghost,
                    "btn-feed-reset",
                    move || set(app, |filter| *filter = FeedFilter::default()),
                ))
                .into_element()
        })
    });
    let bar = view().flex_none().test_id("feed-filters");
    match form {
        Form::Desktop => bar
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(10.0))
            .px(px(20.0))
            .py(px(8.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(field)
            .child(choices)
            .child(reset)
            .into_element(),
        Form::Phone => bar
            .gap(px(10.0))
            .px(px(14.0))
            .pt(px(4.0))
            .pb(px(10.0))
            .child(field)
            .child(choices)
            .child(reset)
            .into_element(),
        Form::Terminal => bar
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x(term::cells(2.0))
            .min_h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(field)
            .child(choices)
            .child(reset)
            .into_element(),
    }
}
