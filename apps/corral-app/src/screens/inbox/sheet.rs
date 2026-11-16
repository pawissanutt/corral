//! The handover sheet: its head, the S B A R Thread History bands in each form, the focus
//! page's head and context column; the decide bar is in `decide`, Situation in `situation`,
//! Background in `graph`. Each band is made again only when its own part of the model changes.
//! A served question first loads its detail (`detail.get`, revalidated when its inbox row
//! changes) and says truthfully while it loads, fails or is gone.

use corral_protocol::detail::{DetailGet, DetailRef};
use nsh_core::prelude::*;

use super::decide::{decide, decide_note};
use super::situation::situation;
use super::{
    Key, SIDE, blind, button, button_as, cells_of, code, evidence, graph, icon_for, kind, live,
    options, press, rich_at, ring, thread, verbs, with_source, words,
};
use crate::model::inbox::{self, Action, Band, BandKind, Handover, Header, Sheet};
use crate::model::{Destination, Form, Hue, Id, Rich};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::band::{band, band_head};
use crate::ui::kit::button::{ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::notice::{empty, loading, notice};
use crate::ui::kit::page::{bar_pane, context, give_way, qhead};
use crate::ui::kit::page_phone::short_window;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// A handover's placeholder while its question loads.
const SHEET_BARS: [(f32, f32); 3] = [(0.3, 10.0), (0.7, 18.0), (1.0, 90.0)];

/// Where a band's content sits: the desktop inline sheet lays Situation and Background out in two
/// columns; the context column and the phone stack them.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Place {
    Wide,
    Narrow,
}

/// What a served handover needs from corrald while it is shown.
#[derive(Clone, PartialEq)]
struct Want {
    /// The shown question.
    shown: Id,
    /// Its inbox row's revision.
    rev: Option<u64>,
    /// Its detail is absent or stale and not on its way.
    stale: bool,
    /// The plan and the selected image the question draws on, kept in the cache while shown.
    context: Vec<DetailRef>,
    /// Of those, the pages not yet asked for.
    gets: Vec<DetailGet>,
}

/// Request the shown question's detail when it is absent or stale, and revalidate it with its
/// etag whenever its inbox row's revision moves (an answer elsewhere, a follow-up); request the
/// plan and the selected image it draws on. All of them stay pinned in the client's cache while
/// the handover shows them and are let go when it moves on or closes, through the source it
/// mounted on (`AppState::scope`). A failed load waits for Retry rather than looping.
pub(super) fn load(app: AppState, key: Key) {
    let scoped = untracked(|| app.scope());
    let wanted = computed(move || {
        with_source(app, |state, src, filter| {
            let inbox::Src::Live(feed) = src;
            let shown = inbox::shown_key_filtered(state, feed, key.as_ref(), filter)?;
            let rev = feed.inbox.get(shown.as_str()).map(|row| row.rev);
            let stale = feed
                .detail(&inbox::question_ref(&shown))
                .is_none_or(|cache| cache.stale && !cache.loading);
            let context = inbox::question_context_refs(state, feed, &shown);
            let gets = inbox::question_dependencies(state, feed, &shown);
            Some(Want {
                shown,
                rev,
                stale,
                context,
                gets,
            })
        })
    });
    let last = stored(None::<(Id, Option<u64>)>);
    let pinned = stored(Vec::<DetailRef>::new());
    effect(move || {
        let Some(want) = wanted.get() else {
            untracked(|| pin(scoped, pinned, Vec::new()));
            return;
        };
        let previous = last.replace(Some((want.shown.clone(), want.rev)));
        let moved = previous.is_some_and(|(id, old)| id == want.shown && old != want.rev);
        let target = inbox::question_ref(&want.shown);
        untracked(|| {
            let mut shown = Vec::with_capacity(want.context.len() + 1);
            shown.push(target.clone());
            shown.extend(want.context);
            pin(scoped, pinned, shown);
            if moved {
                scoped.refresh(target);
            } else {
                scoped.load(target);
            }
            for get in want.gets {
                scoped.detail(get);
            }
        });
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        for gone in pinned.try_with(std::mem::take).unwrap_or_default() {
            scoped.watch_detail(gone, false);
        }
    });
}

/// Pins `wanted` in the client's cache and lets go of what was pinned before and no longer is.
fn pin(app: AppState, pinned: Stored<Vec<DetailRef>>, wanted: Vec<DetailRef>) {
    let was = pinned.replace(wanted.clone());
    for gone in was.iter().filter(|target| !wanted.contains(target)) {
        app.watch_detail(gone.clone(), false);
    }
    for new in wanted.iter().filter(|target| !was.contains(target)) {
        app.watch_detail(new.clone(), true);
    }
}

/// The question's handover once it can be drawn: its skeleton with the model's caption while it
/// loads, the failure with Retry, or why it is gone.
pub(super) fn gate(
    app: AppState,
    key: Key,
    build: impl Fn(AppState) -> AnyElement + 'static,
) -> AnyElement {
    let phase = computed(move || {
        with_source(app, |state, src, filter| {
            inbox::handover_filtered(state, src, key.as_ref(), filter)
        })
    });
    let terminal = app.form == Form::Terminal;
    dynamic(move || {
        let phase = phase.get();
        Some(untracked(|| match phase {
            Handover::Ready => build(app),
            Handover::Loading { caption } => view()
                .flex_1()
                .p(px(if terminal { 0.0 } else { 24.0 }))
                .child(loading(app, caption, &SHEET_BARS))
                .into_element(),
            Handover::Failed(failure) => view()
                .flex_1()
                .when(!terminal, |v| v.p(px(24.0)))
                .when(terminal, |v| v.px(term::cells(1.0)))
                .child(notice(
                    app,
                    failure.hue,
                    &failure.body,
                    failure.retry.map(|retry| button(app, &retry)),
                ))
                .into_element(),
            Handover::Gone(state) => view()
                .flex_1()
                .child(empty(
                    app,
                    state.title,
                    state.body,
                    state.action.map(|action| button(app, &action)),
                ))
                .into_element(),
        }))
    })
}

fn sheet_of(app: AppState, key: Key, focus: bool) -> Memo<Sheet> {
    blind(app, move |state, src, env, filter| {
        if focus {
            inbox::question_filtered(state, src, key.as_ref(), env, filter)
        } else {
            inbox::sheet_desk_filtered(state, src, None, env, false, filter)
        }
    })
}

/// The Inbox's inline handover: head and bands scrolling, the decide bar pinned under them.
/// `compact` (a terminal, a narrow window): Situation and Background stack their parts, and the
/// decide bar keeps to one row of buttons with its attribution at the end of the scroll.
pub(super) fn inline(app: AppState, compact: bool) -> AnyElement {
    let sheet = sheet_of(app, None, false);
    let place = if compact { Place::Narrow } else { Place::Wide };
    let content = view()
        .child(main(app, sheet, place))
        .child(verbs::row(app, None))
        .when(compact, |v| v.child(decide_note(app, None)))
        .into_element();
    bar_pane(app, "inbox-sheet", content, decide(app, None, compact))
}

/// The focus page's centre: head, S A R, the decide bar pinned. `with_side` puts the context
/// bands under them in the same scroll (a narrow terminal or window); `compact` as for
/// [`inline`].
pub(super) fn focus_main(app: AppState, key: Key, with_side: bool, compact: bool) -> AnyElement {
    let sheet = sheet_of(app, key.clone(), true);
    let place = if compact { Place::Narrow } else { Place::Wide };
    let terminal = app.form == Form::Terminal;
    let content = view()
        .child(main(app, sheet, place))
        .child(verbs::row(app, key.clone()))
        .when(with_side && terminal, |v| {
            v.child(term::blank())
                .child(view().px(term::cells(1.0)).child(side_content(app, sheet)))
        })
        .when(with_side && !terminal, |v| {
            v.child(
                view()
                    .px(px(24.0))
                    .pt(px(16.0))
                    .pb(px(24.0))
                    .bg(color(Token::BgSunk))
                    .child(side_content(app, sheet)),
            )
        })
        .when(compact, |v| v.child(decide_note(app, key.clone())))
        .into_element();
    bar_pane(app, "question-main", content, decide(app, key, compact))
}

/// The focus page's context column: B, Thread and History under their heads.
pub(super) fn side(app: AppState, key: Key) -> AnyElement {
    let sheet = sheet_of(app, key, true);
    context(app, "question-side", side_content(app, sheet))
}

/// The phone's question body: the pager row (held upright; held sideways the pager moves into
/// the header, [`phone_trailing`]), the head, the bands in phone order, the thread sheet.
pub(super) fn phone_body(app: AppState, key: Key) -> AnyElement {
    let sheet = sheet_of(app, key.clone(), true);
    let pager = computed(move || sheet.with(|s| (s.pager.clone(), s.next.clone())));
    let talk = computed(move || sheet.with(|s| s.thread.clone()));
    view()
        .child(dynamic(move || {
            if short_window() {
                return None;
            }
            Some(untracked(|| {
                live(pager, move |(pager, next)| pager_row(app, pager, next))
            }))
        }))
        .child(main(app, sheet, Place::Narrow))
        .child(verbs::row(app, key))
        .child(thread::sheet(app, talk))
        .into_element()
}

/// The upright phone's pager row: "1 of 5" quiet, Next with its chevron at the trailing edge.
fn pager_row(app: AppState, pager: Option<String>, next: Option<inbox::Button>) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .pl(px(18.0))
        .pr(px(8.0))
        .bg(color(Token::BgSunk))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .children(pager.map(|pager| words(app, pager, 13.0, Token::Ink3)))
        .child(view().flex_1())
        .children(next.map(|next| {
            view()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .min_h(px(44.0))
                .px(px(10.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(next.label.clone())
                .test_id("btn-next")
                .on_press(press(app, next.action.clone()))
                .active(|s| s.opacity(0.6))
                .focus_visible(ring)
                .child(
                    words(app, next.label.clone(), 13.0, Token::Ink2)
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(icon(Icon::Chevron, 14.0, color(Token::Ink2)))
        }))
        .into_element()
}

/// The phone header's trailing controls. Held sideways, where the window is too short for a
/// pager row, the pager comes up here: "1/5"-style position quiet, then Next as a 44 pt chevron
/// named by its full label; then the Ask-about icon, always.
pub(super) fn phone_trailing(app: AppState, key: Key) -> AnyElement {
    let sheet = sheet_of(app, key.clone(), true);
    let pager = computed(move || sheet.with(|s| (s.pager.clone(), s.next.clone())));
    view()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .child(dynamic(move || {
            if !short_window() {
                return None;
            }
            Some(untracked(|| {
                live(pager, move |(pager, next)| {
                    view()
                        .flex_row()
                        .items_center()
                        .children(pager.map(|pager| {
                            code(app, pager, 13.0, Token::Ink3)
                                .number_of_lines(1u32)
                                .pr(px(2.0))
                        }))
                        .children(next.map(|next| {
                            view()
                                .size(px(44.0))
                                .items_center()
                                .justify_center()
                                .rounded(px(10.0))
                                .cursor_pointer()
                                .role(AccessibilityRole::Button)
                                .label(next.label.clone())
                                .test_id("btn-next")
                                .on_press(press(app, next.action.clone()))
                                .active(|s| s.opacity(0.6))
                                .focus_visible(ring)
                                .child(icon(Icon::Chevron, 18.0, color(Token::Ink2)))
                        }))
                        .into_element()
                })
            }))
        }))
        .child(ask_icon(app, key))
        .into_element()
}

/// The phone header's Ask-about control: the chat icon, a 44 pt target.
fn ask_icon(app: AppState, key: Key) -> AnyElement {
    let ask = computed(move || {
        let env = app.env();
        with_source(app, |state, src, filter| {
            inbox::question_filtered(state, src, key.as_ref(), env, filter).ask
        })
    });
    live(ask, move |ask| {
        let Some(ask) = ask else {
            return view().into_element();
        };
        view()
            .size(px(44.0))
            .items_center()
            .justify_center()
            .rounded(px(10.0))
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label(ask.label.clone())
            .test_id("btn-ask-about")
            .on_press(press(app, ask.action.clone()))
            .active(|s| s.opacity(0.6))
            .focus_visible(ring)
            .child(icon(Icon::Ask, 20.0, color(Token::Ink2)))
            .into_element()
    })
}

/// The focus page's head (`.pagehead`): back to Inbox and the pager, then the keys, Ask about
/// this and Next at the trailing edge. Terminal: one row with a joined line under it, without
/// the keys (the shell's key legend shows them): back, the whole pager, then Next, and Ask
/// about this only while the page holds the context column beside the bands (`term::beside`),
/// where the row has room for it.
pub(super) fn focus_head(app: AppState, key: Key) -> AnyElement {
    let sheet = sheet_of(app, key, true);
    let head = computed(move || {
        sheet.with(|s| {
            (
                s.back.clone(),
                s.pager.clone(),
                s.key_help.clone(),
                s.ask.clone(),
                s.next.clone(),
            )
        })
    });
    live(head, move |(back, pager, key_help, ask, next)| {
        head_row(app, back, pager, key_help, ask, next)
    })
}

/// The head row. Desktop: when the window runs short the keys legend gives way first (whole,
/// never cut; the palette and `?` list the same keys), and the controls never shrink, so Next
/// stays whole at 900 pt.
fn head_row(
    app: AppState,
    back: Option<inbox::Button>,
    pager: Option<String>,
    key_help: Option<Rich>,
    ask: Option<inbox::Button>,
    next: Option<inbox::Button>,
) -> AnyElement {
    let form = app.form;
    let control = |model: &inbox::Button| {
        let options = ButtonOptions {
            icon: icon_for(&model.action),
            ..ButtonOptions::default()
        };
        let drawn = if model.action == Action::Go(Destination::Inbox) {
            button_with(
                app,
                model.label.clone(),
                kind(model.kind),
                options,
                "page-back",
                press(app, model.action.clone()),
            )
        } else {
            button_as(app, model, options)
        };
        view()
            .flex_row()
            .flex_shrink_0()
            .child(drawn)
            .into_element()
    };
    if form == Form::Terminal {
        let leading: Vec<AnyElement> = back
            .iter()
            .map(&control)
            .chain(pager.map(|pager| {
                view()
                    .flex_shrink_0()
                    .child(term::quiet(pager))
                    .into_element()
            }))
            .collect();
        let ask = ask.iter().map(|ask| {
            view()
                .flex_row()
                .flex_shrink_0()
                .style_when(move || !term::beside(app, cells_of(SIDE)), |s| s.hidden())
                .child(control(ask))
                .child(term::space(2.0))
                .into_element()
        });
        let next = next.iter().map(&control);
        return view()
            .flex_none()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .px(term::cells(1.0))
                    .children(spaced(leading))
                    .child(view().flex_1().min_w(px(0.0)))
                    .children(ask)
                    .children(next),
            )
            .child(term::hrule())
            .into_element();
    }
    let leading: Vec<AnyElement> = back
        .iter()
        .map(&control)
        .chain(pager.map(|pager| {
            words(app, pager, 12.0, Token::Ink3)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate()
                .into_element()
        }))
        .collect();
    let trailing: Vec<AnyElement> = key_help
        .as_ref()
        .map(|keys| give_way(keys_line(app, keys)))
        .into_iter()
        .chain(ask.iter().map(&control))
        .chain(next.iter().map(&control))
        .collect();
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(12.0))
        .h(px(46.0))
        .px(px(18.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .children(leading)
        .child(view().flex_1().min_w(px(0.0)))
        .children(trailing)
        .into_element()
}

/// Terminal items two blank Cells apart, where gaps collapse unevenly.
fn spaced(items: Vec<AnyElement>) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(items.len() * 2);
    for (at, item) in items.into_iter().enumerate() {
        if at > 0 {
            out.push(term::space(2.0));
        }
        out.push(item);
    }
    out
}

/// "Keys 1 2 choose · Ctrl ↵ answer · Esc back" on desktop: the mono runs as key caps (`.kbd`).
fn keys_line(app: AppState, keys: &Rich) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .children(keys.iter().map(|run| {
            if !run.mono {
                return words(app, run.text.clone(), 12.0, Token::Ink3)
                    .flex_shrink_0()
                    .into_element();
            }
            view()
                .px(px(4.0))
                .mx(px(2.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .child(code(app, run.text.clone(), 10.5, Token::Ink3).line_height(px(15.0)))
                .into_element()
        }))
        .into_element()
}

/// The head and the bands of `sheet`, in its order.
fn main(app: AppState, sheet: Memo<Sheet>, place: Place) -> AnyElement {
    let header = computed(move || sheet.with(|s| s.header.clone()));
    let bands = computed(move || sheet.with(|s| s.bands.clone()));
    view()
        .child(live(header, move |header| head(app, header)))
        .child(live(bands, move |bands| {
            view()
                .children(
                    bands
                        .into_iter()
                        .map(|lettered| band_of(app, sheet, lettered, place)),
                )
                .into_element()
        }))
        .into_element()
}

/// The handover head: chips, the record in mono and (off the phone) the attribution over the
/// title; the phone moves the attribution under the title.
fn head(app: AppState, header: Header) -> AnyElement {
    let mut meta: Vec<AnyElement> = header
        .chips
        .iter()
        .map(|state| chip(app, state.label, state.hue))
        .collect();
    meta.push(code(app, header.record, 12.0, Token::Ink3).into_element());
    let phone = app.form == Form::Phone;
    if !phone {
        meta.push(words(app, header.attribution.clone(), 12.0, Token::Ink3).into_element());
    }
    qhead(
        app,
        meta,
        header.title,
        phone.then_some(header.attribution),
        Vec::new(),
    )
}

/// One lettered band. The phone's Thread band is the collapsed preview, pressing it opens the
/// thread sheet.
fn band_of(app: AppState, sheet: Memo<Sheet>, lettered: Band, place: Place) -> AnyElement {
    if app.form == Form::Phone && lettered.kind == BandKind::Thread {
        let action = sheet.with_untracked(|s| s.thread_action.clone());
        let preview = computed(move || sheet.with(|s| s.thread_preview.clone()));
        return view()
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label(lettered.title)
            .test_id("btn-thread")
            .on_press(press(app, action))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring)
            .child(band(
                app,
                lettered.letter,
                lettered.title,
                lettered.detail,
                live(preview, move |preview| {
                    rich_at(app, &preview, 14.5, color(Token::Ink)).into_element()
                }),
            ))
            .into_element();
    }
    band(
        app,
        lettered.letter,
        lettered.title,
        lettered.detail,
        body(app, sheet, lettered.kind, place),
    )
}

/// The focus page's context bands, each under its lettered head.
fn side_content(app: AppState, sheet: Memo<Sheet>) -> AnyElement {
    let bands = computed(move || sheet.with(|s| s.side_bands.clone()));
    let terminal = app.form == Form::Terminal;
    live(bands, move |bands| {
        view()
            .children(bands.into_iter().enumerate().map(|(at, lettered)| {
                view()
                    .when(at > 0 && !terminal, |v| v.pt(px(18.0)))
                    .when(at > 0 && terminal, |v| v.child(term::blank()))
                    .child(band_head(
                        app,
                        Some(lettered.letter),
                        lettered.title,
                        lettered.detail,
                        Hue::Neutral,
                    ))
                    .child(view().when(!terminal, |v| v.mt(px(8.0))).child(body(
                        app,
                        sheet,
                        lettered.kind,
                        Place::Narrow,
                    )))
                    .into_element()
            }))
            .into_element()
    })
}

/// A band's content, made again when its own part of the sheet changes.
fn body(app: AppState, sheet: Memo<Sheet>, which: BandKind, place: Place) -> AnyElement {
    match which {
        BandKind::Situation => live(
            computed(move || sheet.with(|s| s.situation.clone())),
            move |part| situation(app, part, place),
        ),
        BandKind::Background => live(
            computed(move || sheet.with(|s| s.background.clone())),
            move |part| graph::background(app, part),
        ),
        BandKind::Assessment => live(
            computed(move || sheet.with(|s| s.assessment.clone())),
            move |part| view().child(evidence::assessment(app, part)).into_element(),
        ),
        BandKind::Recommendation => live(
            computed(move || sheet.with(|s| s.recommendation.clone())),
            move |part| options::recommendation(app, part),
        ),
        BandKind::Thread => live(
            computed(move || sheet.with(|s| s.thread.clone())),
            move |part| thread::thread(app, part),
        ),
        BandKind::History => live(
            computed(move || sheet.with(|s| (s.history.clone(), s.history_empty))),
            move |(rows, none)| thread::history(app, rows, none),
        ),
    }
}
