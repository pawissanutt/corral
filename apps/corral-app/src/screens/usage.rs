//! Usage: cost and quota per provider, profile and session, visibility only. Desktop leads with
//! the page's one coverage note, then sets a summary card per provider in a ruled grid
//! (`.ugrid`, up to three a row as the width allows); under it the profile table (grouped by
//! provider) and cost per day stand side by side where the width allows, else stacked, then the
//! sessions. The phone stacks the provider blocks (with their profiles), the cost bars and the
//! sessions; a terminal draws a gauge line per provider, the table in columns and the costs as
//! Cell bars. Every word and figure is `model::usage`'s, built from the selected feed's recorded
//! usage (`shown`) in a daemon session and the explicit sample preview alike; loading, stale,
//! failed and partial states are said once, compactly, above the content. The period switch
//! runs its reducer.

mod cellbar;
mod chart;
mod costs;
mod profiles;
mod providers;
mod quotas;
mod sessions;
mod shown;

use nsh_core::element::cell_size;
use nsh_core::prelude::*;

use crate::model::text::span;
use crate::model::usage::{
    PROFILE_TITLE, Page, Period, ProviderView, SESSION_TITLE, SUBTITLE, TITLE,
};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button, seg};
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{body, head, pane, section};
use crate::ui::kit::text::{dim, h3};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use shown::{Shown, Status};

/// The held-ink words that qualify a figure (partial coverage, a retained copy); Timeline says
/// its coverage the same way.
pub(crate) use costs::qualifier;

/// A provider card needs about this much width (its amounts line and chart); the grid holds as
/// many a row as fit, up to [`CARDS_MAX`].
const CARD_MIN: f32 = 270.0;
const CARDS_MAX: usize = 3;
/// Below this content width the profile and session tables have no room for their columns and
/// stack as records (a narrow window, or a collapsed sidebar on a small screen).
const TABLES_MIN: f32 = 760.0;
/// From this content width the profile table and cost per day stand side by side.
const BESIDE_MIN: f32 = 1080.0;
/// A desktop section's sides, as the page head's.
const SECTION_SIDES: f32 = 18.0;
/// A terminal below this many Cells lists the sessions as records too.
const TERM_TABLES_MIN: usize = 64;
/// One Cell where the surface reports none yet.
const COLUMN: f32 = 8.0;

/// How the desktop sections fit the content width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// Tables as stacked records.
    Narrow,
    /// Tables in columns, one section under another.
    Stacked,
    /// The profile table beside cost per day.
    Beside,
}

fn fit(width: Option<f32>) -> Fit {
    match width {
        Some(width) if width < TABLES_MIN => Fit::Narrow,
        Some(width) if width < BESIDE_MIN => Fit::Stacked,
        _ => Fit::Beside,
    }
}

/// The Usage destination on every form. Desktop and terminal fill the content area and scroll
/// below the page head; the phone returns its scrolling body (the shell draws the pushed
/// header with "Usage" and the back to More). test ids: "page-title" (desktop and terminal
/// head), "usage" (the scroll), "seg-period-<n>", "usage-<provider id>", "usage-profiles",
/// "usage-sessions", "usage-costs", "usage-quotas-<provider id>", "btn-retry-usage".
pub fn screen(app: AppState) -> AnyElement {
    shown::keep_loaded(app);
    let data = computed(move || shown::shown(app));
    match app.form {
        Form::Desktop => desktop(app, data),
        Form::Phone => phone(app, data),
        Form::Terminal => terminal(app, data),
    }
}

/// Today, 7 days, 30 days (`.seg`, `.pseg`): the selected period from `app.usage`.
fn period_seg(app: AppState) -> AnyElement {
    let options = Period::ALL.map(|period| (period, period.label()));
    seg(
        app,
        &options,
        move || app.usage.with(|state| state.period),
        move |period| app.act(app.usage, |state, _| state.select_period(period)),
        "seg-period",
    )
}

/// Above the content: the rows' loading or stale state, then corrald's read failure said once
/// as a compact notice (the model folds the rows' and the series' causes into one line) with its
/// Retry, then how complete the totals are, said once for the whole page in the held tone, as
/// partial figures are degraded ones; nothing when current and whole.
fn status(app: AppState, current: &Shown) -> Vec<AnyElement> {
    let pad = |element: AnyElement| match app.form {
        Form::Terminal => element,
        Form::Desktop | Form::Phone => view()
            .px(px(18.0))
            .pt(px(12.0))
            .child(element)
            .into_element(),
    };
    let state = match current.status {
        Status::Ready => None,
        Status::Loading => Some(notice(
            app,
            Hue::Neutral,
            &vec![span("Loading recorded usage from corrald…")],
            None,
        )),
        Status::Stale => Some(notice(
            app,
            Hue::Held,
            &vec![span(
                "Showing the last usage received; it may be out of date until corrald answers again.",
            )],
            None,
        )),
    };
    state
        .into_iter()
        .chain(current.page.notices.iter().map(|line| {
            let retry = button(
                app,
                "Retry",
                ButtonKind::Ghost,
                "btn-retry-usage",
                move || shown::retry(app),
            );
            notice(app, Hue::Fail, &vec![span(line.as_str())], Some(retry))
        }))
        .chain(
            current
                .page
                .coverage
                .iter()
                .map(|line| notice(app, Hue::Held, &vec![span(line.as_str())], None)),
        )
        .map(pad)
        .collect()
}

/// The page body, built again when the shown page changes.
fn live_body(data: Memo<Shown>, build: impl Fn(&Shown) -> AnyElement + 'static) -> AnyElement {
    dynamic(move || {
        let shown = data.get();
        untracked(|| build(&shown))
    })
    .into_element()
}

/// A desktop section: its caption over the content, at the page head's 18 pt sides.
fn block(app: AppState, title: impl Into<SharedString>, content: AnyElement) -> AnyElement {
    view()
        .px(px(SECTION_SIDES))
        .pb(px(10.0))
        .child(h3(app, title, None))
        .child(content)
        .into_element()
}

/// The provider cards, `columns` a row, rules between them and under each row (`.ugrid`). A
/// short last row keeps its cards at the full rows' width.
fn grid(app: AppState, providers: &[ProviderView], columns: usize) -> AnyElement {
    let columns = columns.clamp(1, CARDS_MAX).min(providers.len().max(1));
    let rows = providers
        .chunks(columns)
        .map(|chunk| {
            let gaps = (chunk.len()..columns).map(|_| {
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .border_l(px(1.0))
                    .border_l_color(color(Token::Rule))
                    .into_element()
            });
            view()
                .flex_row()
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .children(
                    chunk
                        .iter()
                        .enumerate()
                        .map(|(at, item)| providers::desktop_cell(app, item, at == 0))
                        .collect::<Vec<_>>(),
                )
                .children(gaps.collect::<Vec<_>>())
                .into_element()
        })
        .collect::<Vec<_>>();
    view().children(rows).into_element()
}

fn desktop(app: AppState, data: Memo<Shown>) -> AnyElement {
    let width = signal(None::<f32>);
    let columns = computed(move || {
        width
            .get()
            .map_or(CARDS_MAX, |w| ((w / CARD_MIN) as usize).clamp(1, CARDS_MAX))
    });
    let fits = computed(move || fit(width.get()));
    let content = view()
        .on_layout(move |frame: &Bounds<Points>| {
            let measured = Some(frame.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        .child(dynamic(move || {
            let (columns, fits) = (columns.get(), fits.get());
            let shown = data.get();
            untracked(|| desktop_body(app, &shown, columns, fits))
        }))
        .into_element();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(
            app,
            TITLE,
            Some(SUBTITLE.to_owned()),
            vec![period_seg(app)],
        ))
        .child(pane(app, "usage", content))
        .into_element()
}

/// The profile table and cost per day: side by side where they [`Fit::Beside`], the table at its
/// own width and the bars taking the rest past a rule; else one under the other, the table no
/// wider than beside, so its figures stay near their names.
fn profiles_and_costs(app: AppState, page: &Page, fits: Fit) -> AnyElement {
    let table_width = profiles::desktop_width(page);
    let profiles = (!page.groups.is_empty()).then(|| {
        let table = if fits == Fit::Narrow {
            profiles::compact(app, page)
        } else {
            profiles::desktop(app, page)
        };
        block(
            app,
            PROFILE_TITLE,
            view().max_w(px(table_width)).child(table).into_element(),
        )
    });
    let costs = block(
        app,
        page.costs.title.clone(),
        costs::bars(app, &page.costs),
    );
    match profiles {
        Some(profiles) if fits == Fit::Beside => view()
            .flex_row()
            .child(
                view()
                    .w(px(table_width + 2.0 * SECTION_SIDES))
                    .flex_shrink_0()
                    .child(profiles),
            )
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .border_l(px(1.0))
                    .border_l_color(color(Token::Rule))
                    .child(costs),
            )
            .into_element(),
        profiles => view().children(profiles).child(costs).into_element(),
    }
}

/// The desktop page: the notices, `columns` provider cards a row, then the sections as they
/// [`fit`] the content width.
fn desktop_body(app: AppState, shown: &Shown, columns: usize, fits: Fit) -> AnyElement {
    let page = &shown.page;
    let notices = status(app, shown);
    // Under notices the grid opens with its own rule; under the page head, the head's rule.
    let lead = !notices.is_empty() || page.providers_empty.is_some();
    view()
        .children(notices)
        .children(page.providers_empty.map(|line| {
            view()
                .px(px(SECTION_SIDES))
                .pt(px(16.0))
                .child(dim(app, line))
                .into_element()
        }))
        .when(!page.providers.is_empty(), |body| {
            body.child(
                view()
                    .when(lead, |grid| {
                        grid.mt(px(12.0))
                            .border_t(px(1.0))
                            .border_color(color(Token::Rule))
                    })
                    .child(grid(app, &page.providers, columns)),
            )
        })
        .child(profiles_and_costs(app, page, fits))
        .child(block(
            app,
            SESSION_TITLE,
            sessions::table(
                app,
                &page.sessions,
                page.sessions_note.as_deref(),
                fits == Fit::Narrow,
            ),
        ))
        .child(
            view()
                .px(px(SECTION_SIDES))
                .pt(px(8.0))
                .pb(px(18.0))
                .child(note(app, page.disclaimer)),
        )
        .into_element()
}

/// The phone body: the period switch (`.pseg`, 4 16 8 around it), a block per provider with its
/// profiles, the cost bars and the sessions as sections, and the note. It is the page's scroll
/// and keeps the clearance under the floating Ask button.
fn phone(app: AppState, data: Memo<Shown>) -> AnyElement {
    let content = view()
        .child(
            view()
                .px(px(16.0))
                .pt(px(4.0))
                .pb(px(8.0))
                .child(period_seg(app)),
        )
        .child(live_body(data, move |shown| {
            let page = &shown.page;
            let blocks = page
                .providers
                .iter()
                .map(|item| providers::phone_block(app, item))
                .collect::<Vec<_>>();
            view()
                .children(status(app, shown))
                .children(page.providers_empty.map(|line| {
                    view()
                        .px(px(18.0))
                        .py(px(12.0))
                        .child(dim(app, line))
                        .into_element()
                }))
                .children(blocks)
                .child(section(
                    app,
                    Some(page.costs.title.clone()),
                    None,
                    Hue::Neutral,
                    costs::bars(app, &page.costs),
                ))
                .child(section(
                    app,
                    Some(SESSION_TITLE.to_owned()),
                    None,
                    Hue::Neutral,
                    sessions::table(app, &page.sessions, page.sessions_note.as_deref(), true),
                ))
                .child(
                    view()
                        .px(px(18.0))
                        .pt(px(10.0))
                        .pb(px(24.0))
                        .child(note(app, page.disclaimer)),
                )
                .into_element()
        }))
        .into_element();
    pane(app, "usage", content)
}

fn terminal(app: AppState, data: Memo<Shown>) -> AnyElement {
    let width = signal(None::<f32>);
    let content = view()
        .on_layout(move |frame: &Bounds<Points>| {
            let measured = Some(frame.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        .child(dynamic(move || {
            let column = cell_size().map_or(COLUMN, |cell| cell.width.0);
            let room = width.get().map_or_else(term::columns, |w| {
                (w / column + 0.01).floor().max(0.0) as usize
            });
            let shown = data.get();
            untracked(|| terminal_content(app, &shown, room))
        }))
        .into_element();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(
            app,
            TITLE,
            Some(SUBTITLE.to_owned()),
            vec![period_seg(app)],
        ))
        .child(body(app, "usage", content))
        .into_element()
}

/// The terminal page at `room` Cells: the gauge lines (a blank row between providers), the
/// table, the cost bars, the sessions, the note, a blank row between the parts.
fn terminal_content(app: AppState, shown: &Shown, room: usize) -> AnyElement {
    let page = &shown.page;
    let mut lines = Vec::with_capacity(page.providers.len() * 2);
    for (at, item) in page.providers.iter().enumerate() {
        if at > 0 {
            lines.push(term::blank());
        }
        lines.push(providers::terminal_line(app, item, room));
    }
    view()
        .children(status(app, shown))
        .child(term::blank())
        .children(
            page.providers_empty
                .map(|line| dim(app, line).into_element()),
        )
        .children(lines)
        .children((!page.groups.is_empty()).then(|| {
            view()
                .child(term::blank())
                .child(h3(app, PROFILE_TITLE, None))
                .child(profiles::terminal(app, page, room))
                .into_element()
        }))
        .child(term::blank())
        .child(h3(app, page.costs.title.clone(), None))
        .child(costs::terminal(app, &page.costs, room))
        .child(term::blank())
        .child(h3(app, SESSION_TITLE, None))
        .child(sessions::table(
            app,
            &page.sessions,
            page.sessions_note.as_deref(),
            room < TERM_TABLES_MIN,
        ))
        .child(term::blank())
        .child(note(app, page.disclaimer))
        .child(term::blank())
        .into_element()
}

/// The page's one-line note on where the figures come from, quiet and small; it wraps rather
/// than clips where the width runs short.
fn note(app: AppState, content: &'static str) -> AnyElement {
    let words = text(content);
    match app.form {
        Form::Terminal => words.text_color(color(Token::Ink3)).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(11.5))
            .line_height(px(16.0))
            .text_color(color(Token::Ink3))
            .into_element(),
    }
}
