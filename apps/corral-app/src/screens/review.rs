//! Review: the tasks in Review beside the selected candidate, its Brief (ready to land? and
//! what changed) or its evidence (Diff, Gate log, Summary, Artifacts), its side facts and the
//! pinned decide bar. Every word, count and branch comes from `model::review::loop_page_filtered`,
//! in the Review screen's own project scope; this module lays that page out for each form and
//! sends each control's action back through the feed-backed reducer (`apply_live_filtered`).
//!
//! Desktop is the mock's three columns: the outline, the candidate, its side facts beside the
//! Brief. Under `SIDE_BESIDE` points of content the facts move under the Brief; under
//! `LIST_BESIDE` the outline is the page and a picked candidate takes its place, with a way
//! back. The candidate's column measures itself, so the model draws split diff rows only where
//! they fit. Phone: the outline, each row pushing its candidate's page, which scrolls with the
//! decide bar pinned in place of the tab bar. Terminal: outline | candidate in Cells, the facts
//! under the Brief, the diffs unified.

mod head;
mod list;

use nsh_core::prelude::*;

use crate::model::review::{Body, LoopContent, LoopPage};
use crate::model::{Form, Id, Live};
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::glance::{page_head, stats};
use crate::ui::kit::page::{Width, bar_pane, columns, pane};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

// The candidate's pieces live with the task page; these paths stay for their other callers.
use crate::screens::task::candidate::{
    Ctx, act, command, decide, evidence, fits_split, keep_current, layout_switch,
};
pub(crate) use crate::screens::task::candidate::{brief, diff};

/// The outline's column: `272px` beside the side facts, `260px` once they stand under the Brief.
const LIST_WIDE: f32 = 272.0;
const LIST_NARROW: f32 = 260.0;
/// The side facts' column (`.split.s-review`).
const SIDE_WIDTH: f32 = 300.0;
/// The content widths from which the side facts stand beside the candidate, and the outline
/// beside it (the mock's 1180 and 900 rules).
const SIDE_BESIDE: f32 = 1180.0;
const LIST_BESIDE: f32 = 900.0;
/// `.is-stale .stalefx`: the last known state reads quieter while offline or down.
const STALE_OPACITY: f32 = 0.78;
/// How the desktop arranges the outline, the candidate and its side facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    /// Outline | candidate | side facts (with the Brief).
    Three,
    /// Outline | candidate, the facts under the Brief.
    Two,
    /// The outline or, once one is picked, the candidate.
    One,
}

impl Arrangement {
    fn at(width: f32) -> Arrangement {
        if width >= SIDE_BESIDE {
            Arrangement::Three
        } else if width >= LIST_BESIDE {
            Arrangement::Two
        } else {
            Arrangement::One
        }
    }
}

/// The Review destination on every form. Desktop and terminal fill the content container and
/// scroll inside themselves; a phone lists the candidates, each pushing its page.
pub fn screen(app: AppState) -> AnyElement {
    let cx = Ctx::new(app, None);
    match app.form {
        Form::Desktop => desktop(cx),
        Form::Terminal => terminal(cx),
        Form::Phone => phone_list(cx),
    }
}

/// A candidate pushed on the phone, by its task id: the scrolling body and the decide bar, which
/// carries Ask, in place of the tab bar.
pub fn live_review_phone(app: AppState, task: Id) -> PhonePage {
    let title = task.to_string();
    let cx = Ctx::new(app, Some(task));
    PhonePage {
        title,
        body: phone_body(cx.clone()),
        trailing: None,
        bar: Some(PhoneBar::always(decide::decide(cx))),
        ask: false,
        scroll: true,
    }
}

/// The page head (`.phead`): Review and its counts.
fn page_title(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let head = computed(move || page.with(|page| (page.title, page.stats.clone())));
    dynamic(move || {
        let (title, stats) = head.get();
        page_head(app, title, &stats, Vec::new())
    })
}

/// Tracked: whether there is anything to lay out, a candidate listed or shown.
fn anything(page: Memo<LoopPage>) -> Memo<bool> {
    computed(move || page.with(|page| !page.outline.is_empty() || page.detail.is_some()))
}

/// Desktop: the head and the scope over the arrangement the content's width allows.
fn desktop(cx: Ctx) -> AnyElement {
    let app = cx.app;
    keep_current(&cx);
    let page = cx.page;
    let room = signal(None::<f32>);
    let arrangement = computed(move || room.get().map(Arrangement::at));
    let anything = anything(page);
    let stale = computed(move || page.with(|page| page.stale));
    let opened = signal(false);
    // Below `LIST_BESIDE` the outline comes first after a plain `go`; a candidate opened by
    // name (from Now, the palette) shows at once, and again on each further open. A row
    // pressed here opens it too, without moving this key.
    let arrival = computed(move || {
        app.nav.with(|nav| {
            let named = matches!(nav.opened, Some(Live::Review(_))).then_some(nav.opens);
            (nav.goes, named)
        })
    });
    effect(move || {
        let named = arrival.get().1.is_some();
        untracked(|| opened.set(named));
    });
    let body = {
        let cx = cx.clone();
        dynamic(move || {
            let arrangement = arrangement.get()?;
            if !anything.get() {
                return Some(empty_page(&cx));
            }
            Some(match arrangement {
                Arrangement::Three => beside(&cx, LIST_WIDE, true),
                Arrangement::Two => beside(&cx, LIST_NARROW, false),
                Arrangement::One if opened.get() => candidate(&cx, true, Some(back(&cx, opened))),
                Arrangement::One => outline_pane(&cx, Some(opened)),
            })
        })
    };
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .opacity(move || if stale.get() { STALE_OPACITY } else { 1.0 })
        .child(page_title(&cx))
        .child(
            view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
                .child(body),
        )
        .into_element()
}

/// Terminal: the head and the scope over outline | candidate, which stack when the grid is
/// narrow (`page::columns`).
fn terminal(cx: Ctx) -> AnyElement {
    let app = cx.app;
    keep_current(&cx);
    let anything = anything(cx.page);
    let body = {
        let cx = cx.clone();
        dynamic(move || {
            if !anything.get() {
                return empty_page(&cx);
            }
            columns(
                app,
                vec![
                    (Width::Fixed(LIST_WIDE), outline_pane(&cx, None)),
                    (Width::Fill, candidate(&cx, true, None)),
                ],
                1,
            )
        })
    };
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(page_title(&cx))
        .child(
            view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(body),
        )
        .into_element()
}

/// The phone's destination, pushed over More: its counts and the scope over the outline, each
/// row pushing its candidate's page. The shell's pushed head already carries the page's title
/// (`page-title`), so this page draws no head of its own.
fn phone_list(cx: Ctx) -> AnyElement {
    let app = cx.app;
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(phone_counts(&cx))
        .child(pane(
            app,
            "review-list",
            view().child(list::outline(&cx, None)).into_element(),
        ))
        .into_element()
}

/// The page's counts on a line of their own under the shell's head (`.stats`); nothing while
/// nothing is counted.
fn phone_counts(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let counts = computed(move || page.with(|page| page.stats.clone()));
    dynamic(move || {
        let counts = counts.get();
        (!counts.is_empty()).then(|| {
            view()
                .flex_row()
                .flex_none()
                .px(px(18.0))
                .pt(px(4.0))
                .pb(px(10.0))
                .child(stats(app, &counts))
                .into_element()
        })
    })
}

/// The pushed candidate's scrolling body: its head and tabs, Run gate and Start review (and Land
/// anyway when offered), then the Brief with the side facts at its end, or the selected
/// evidence.
fn phone_body(cx: Ctx) -> AnyElement {
    keep_current(&cx);
    let page = cx.page;
    let stale = computed(move || page.with(|page| page.stale));
    view()
        .opacity(move || if stale.get() { STALE_OPACITY } else { 1.0 })
        .child(head::head(&cx, None))
        .child(decide::phone_secondary(&cx))
        .child(content(&cx, true))
        .into_element()
}

/// Why nothing is listed, in the place of the columns (`.split.one`).
fn empty_page(cx: &Ctx) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let line = computed(move || page.with(|page| page.empty.clone()));
    dynamic(move || line.get().map(|line| list::empty_line(app, line)))
}

/// The outline in its scrolling pane.
fn outline_pane(cx: &Ctx, opened: Option<Signal<bool>>) -> AnyElement {
    pane(cx.app, "review-list", list::outline(cx, opened))
}

/// Desktop: the outline `list` points wide beside the candidate, and the side facts beside the
/// Brief when `side_beside`.
fn beside(cx: &Ctx, list: f32, side_beside: bool) -> AnyElement {
    view()
        .flex_1()
        .flex_row()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(
            view()
                .w(px(list))
                .flex_shrink_0()
                .flex_col()
                .min_h(px(0.0))
                .child(outline_pane(cx, None)),
        )
        .child(
            view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .border_l(px(1.0))
                .border_l_color(color(Token::Rule))
                .child(candidate(cx, !side_beside, None)),
        )
        .when(side_beside, |row| row.child(side_column(cx)))
        .into_element()
}

/// The side facts' column, while the Brief shows (`.aside`).
fn side_column(cx: &Ctx) -> AnyElement {
    let page = cx.page;
    let side = computed(move || {
        page.with(
            |page| match page.detail.as_ref().map(|detail| &detail.content) {
                Some(LoopContent::Brief(brief)) => Some(brief.side.clone()),
                _ => None,
            },
        )
    });
    let cx = cx.clone();
    dynamic(move || {
        side.get().map(|facts| {
            view()
                .w(px(SIDE_WIDTH))
                .flex_shrink_0()
                .flex_col()
                .min_h(px(0.0))
                .border_l(px(1.0))
                .border_l_color(color(Token::Rule))
                .child(pane(cx.app, "review-side", brief::side(&cx, &facts)))
                .into_element()
        })
    })
}

/// The way back to the outline where the candidate stands in its place: the page's title.
fn back(cx: &Ctx, opened: Signal<bool>) -> AnyElement {
    let app = cx.app;
    let title = cx.page.with_untracked(|page| page.title);
    button_with(
        app,
        title,
        ButtonKind::Quiet,
        ButtonOptions {
            icon: Some(Icon::Back),
            ..ButtonOptions::default()
        },
        "page-back",
        move || {
            opened.set(false);
            app.nav.update(|nav| nav.dismiss_opened());
        },
    )
}

/// The candidate's column: its head and tabs, the scrolling content (the side facts under the
/// Brief when `side_below`), the decide bar pinned under it. A desktop column measures itself
/// for the model's split rows.
fn candidate(cx: &Ctx, side_below: bool, back: Option<AnyElement>) -> AnyElement {
    let app = cx.app;
    let split = cx.split;
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .when(app.form == Form::Desktop, |column| {
            column.on_layout(move |frame: &Bounds<Points>| {
                let fits = fits_split(app, frame.size.width.0);
                if split.with_untracked(|now| *now != Some(fits)) {
                    split.set(Some(fits));
                }
            })
        })
        .child(head::head(cx, back))
        .child(bar_pane(
            app,
            "review-detail",
            content(cx, side_below),
            decide::decide(cx.clone()),
        ))
        .into_element()
}

/// What the candidate's column scrolls: the Brief or the selected evidence, or why no candidate
/// is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Main {
    Content(LoopContent),
    Empty(String),
}

/// The selected content, rebuilt only when the model's content changes (a tab, the layout, the
/// served data, the column crossing the split threshold). A desktop draws nothing until its
/// column is measured, so it never shows split rows it has to take back.
fn content(cx: &Ctx, side_below: bool) -> AnyElement {
    let page = cx.page;
    let split = cx.split;
    let main = computed(move || {
        split.get()?;
        page.with(|page| {
            Some(match &page.detail {
                Some(detail) => Main::Content(detail.content.clone()),
                None => Main::Empty(page.empty.clone().unwrap_or_default()),
            })
        })
    });
    let cx = cx.clone();
    dynamic(move || {
        Some(match main.get()? {
            Main::Content(LoopContent::Brief(brief)) => brief::brief(&cx, &brief, side_below),
            Main::Content(LoopContent::Evidence(body)) => evidence(&cx, &body),
            Main::Empty(line) => list::empty_line(cx.app, line),
        })
    })
}

/// The Diff, Gate log, Report or Artifacts tab, padded as a block (`.blk`); the Diff tab led by
/// its layout control.
fn evidence(cx: &Ctx, body: &Body) -> AnyElement {
    let app = cx.app;
    let content = match body {
        Body::Diff(files) => view()
            .child(layout_switch(cx))
            .child(diff::diff(cx, files))
            .into_element(),
        Body::Report(report) => evidence::report(app, report),
        Body::Artifacts(list) => evidence::artifacts(cx, list),
        Body::Log { lines, note } => evidence::log(app, lines, note),
        Body::Notice(line) => dim(app, line.clone()).into_element(),
    };
    match app.form {
        Form::Desktop => view()
            .px(px(20.0))
            .py(px(16.0))
            .child(content)
            .into_element(),
        Form::Phone => view()
            .px(px(14.0))
            .pt(px(12.0))
            .pb(px(20.0))
            .child(content)
            .into_element(),
        Form::Terminal => view()
            .px(term::cells(1.0))
            .child(term::blank())
            .child(content)
            .into_element(),
    }
}
