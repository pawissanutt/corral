//! The handover sheet: its head, the S B A R Thread History bands in each form, the focus
//! page's head and context column; the decide bar is in `decide`. Each band is made again only
//! when its own part of the model changes. A served question first loads its detail
//! (`detail.get`, revalidated when its inbox row changes) and says truthfully while it loads,
//! fails or is gone.

use nsh_core::prelude::*;

use super::decide::{decide, decide_note};
use super::{
    Key, blind, button, button_as, code, evidence, graph, icon_for, kind, live, options, press,
    rich_at, ring, thread, with_source, words,
};
use crate::model::board::Mark;
use crate::model::inbox::{
    self, Action, Background, Band, BandKind, Handover, Header, Notice, Sheet, Situation, WaitKind,
    WaitLine,
};
use crate::model::{Destination, Form, Hue, Rich};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::band::{band, band_head};
use crate::ui::kit::button::{ButtonOptions, button_with};
use crate::ui::kit::chip::{chip, illus, stamp};
use crate::ui::kit::marks::{dot, hue_fill, mark};
use crate::ui::kit::notice::{empty, notice, skeleton};
use crate::ui::kit::page::{bar_pane, context, qhead};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// Background's placeholder bars while task detail loads (`.skel` 90 %, 70 %, 80 % × 60).
const PLACE_BARS: [(f32, f32); 3] = [(0.9, 10.0), (0.7, 10.0), (0.8, 60.0)];
/// A handover's placeholder while its question loads.
const SHEET_BARS: [(f32, f32); 3] = [(0.3, 10.0), (0.7, 18.0), (1.0, 90.0)];

/// Where a band's content sits: the desktop inline sheet lays Situation and Background out in two
/// columns; the context column and the phone stack them.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    Wide,
    Narrow,
}

/// Served: request the shown question's detail when it is absent or stale, and revalidate it
/// with its etag whenever its inbox row's revision moves (an answer elsewhere, a follow-up).
/// A failed load waits for Retry rather than looping.
pub(super) fn load(app: AppState, key: Key) {
    let wanted = computed(move || {
        if !app.daemon() {
            return None;
        }
        app.feed.with(|feed| {
            app.inbox.with(|state| {
                let shown = inbox::shown_key(state, feed, key.as_ref())?;
                let rev = feed.inbox.get(shown.as_str()).map(|row| row.rev);
                let stale = feed
                    .detail(&inbox::question_ref(&shown))
                    .is_none_or(|cache| cache.stale && !cache.loading);
                Some((shown, rev, stale))
            })
        })
    });
    let last = stored(None::<(crate::model::Id, Option<u64>)>);
    effect(move || {
        let Some((shown, rev, _)) = wanted.get() else {
            return;
        };
        let previous = last.replace(Some((shown.clone(), rev)));
        let moved = previous.is_some_and(|(id, old)| id == shown && old != rev);
        let target = inbox::question_ref(&shown);
        untracked(|| {
            if moved {
                app.refresh(target);
            } else {
                app.load(target);
            }
        });
    });
}

/// The served question's handover once it can be drawn: its skeleton while it loads, the
/// failure with Retry, or why it is gone. The fixture is always ready.
pub(super) fn gate(
    app: AppState,
    key: Key,
    build: impl Fn(AppState) -> AnyElement + 'static,
) -> AnyElement {
    let phase =
        computed(move || with_source(app, |state, src| inbox::handover(state, src, key.as_ref())));
    let terminal = app.form == Form::Terminal;
    dynamic(move || {
        let phase = phase.get();
        Some(untracked(|| match phase {
            Handover::Ready => build(app),
            Handover::Loading => view()
                .flex_1()
                .label("Loading")
                .p(px(if terminal { 0.0 } else { 24.0 }))
                .child(skeleton(app, &SHEET_BARS))
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
    blind(app, move |state, src, env| {
        if focus {
            inbox::question(state, src, key.as_ref(), env)
        } else {
            inbox::sheet_desk(state, src, None, env, false)
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

/// The phone's question body: the pager, the head, the bands in phone order, the thread sheet.
pub(super) fn phone_body(app: AppState, key: Key) -> AnyElement {
    let sheet = sheet_of(app, key, true);
    let pager = computed(move || sheet.with(|s| (s.pager.clone(), s.next.clone())));
    let talk = computed(move || sheet.with(|s| s.thread.clone()));
    view()
        .child(live(pager, move |(pager, next)| {
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
        }))
        .child(main(app, sheet, Place::Narrow))
        .child(thread::sheet(app, talk))
        .into_element()
}

/// The phone header's Ask-about control: the chat icon, a 44 pt target.
pub(super) fn ask_icon(app: AppState, key: Key) -> AnyElement {
    let ask = computed(move || {
        let env = app.env();
        with_source(app, |state, src| {
            inbox::question(state, src, key.as_ref(), env).ask
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
/// about this only on a wide grid, where it fits.
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
        if model.action == Action::Go(Destination::Inbox) {
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
        }
    };
    if form == Form::Terminal {
        let whole = |item: AnyElement| view().flex_shrink_0().child(item).into_element();
        let leading: Vec<AnyElement> = back
            .iter()
            .map(|back| whole(control(back)))
            .chain(pager.map(|pager| whole(term::quiet(pager).into_element())))
            .collect();
        let ask = ask.iter().map(|ask| {
            view()
                .flex_row()
                .flex_shrink_0()
                .style_when(move || !term::wide(), |s| s.hidden())
                .child(control(ask))
                .child(term::space(2.0))
                .into_element()
        });
        let next = next.iter().map(|next| whole(control(next)));
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
        .chain(pager.map(|pager| words(app, pager, 12.0, Token::Ink3).into_element()))
        .collect();
    let trailing: Vec<AnyElement> = key_help
        .as_ref()
        .map(|keys| keys_line(app, keys))
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
            move |part| background(app, part, place),
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

/// The question as a lede (14 pt, 1.55; the phone's 15.5 pt), its deadline, the waits beside it
/// (desktop inline) or under it, the answer's stamp under both.
fn situation(app: AppState, part: Situation, place: Place) -> AnyElement {
    let form = app.form;
    let lede = match form {
        Form::Phone => words(app, part.text, 15.5, Token::Ink).line_height(px(23.0)),
        _ => words(app, part.text, 14.0, Token::Ink)
            .line_height(px(21.7))
            .max_w(px(640.0)),
    };
    let lede = view().child(lede).children(part.deadline.map(|deadline| {
        view()
            .mt(px(8.0))
            .when(form == Form::Terminal, |v| v.child(term::blank()))
            .child(rich_at(
                app,
                &deadline,
                if form == Form::Phone { 13.5 } else { 12.5 },
                color(Token::Ink2),
            ))
    }));
    let waits_head = match form {
        Form::Terminal => h3(app, part.waits_title, None),
        _ => band_head(app, None, part.waits_title, None, Hue::Neutral),
    };
    let size = if form == Form::Phone { 13.0 } else { 12.5 };
    let waits = view().child(waits_head).child(
        view()
            .mt(px(if form == Form::Desktop { 12.0 } else { 6.0 }))
            .gap(px(4.0))
            .children(part.waits.iter().map(|wait| wait_row(app, wait)))
            .children(
                part.waits_empty
                    .map(|none| words(app, none, size, Token::Ink3)),
            ),
    );
    let top = if place == Place::Wide && form == Form::Desktop {
        view()
            .flex_row()
            .items_start()
            .gap(px(22.0))
            .child(view().flex_1().min_w(px(0.0)).child(lede))
            .child(view().w(px(290.0)).flex_shrink_0().pt(px(2.0)).child(waits))
    } else {
        view().child(lede).child(
            view()
                .mt(px(14.0))
                .when(form == Form::Terminal, |v| v.child(term::blank()))
                .child(waits),
        )
    };
    view()
        .child(top)
        .children(part.notice.map(|notice| {
            view()
                .mt(px(12.0))
                .when(form == Form::Terminal, |v| v.child(term::blank()))
                .child(stamped_notice(app, notice))
        }))
        .into_element()
}

/// One dependency: needs-you dot, not-yet mark or not-applicable mark, the id, the reason
/// (quiet for the independent one).
fn wait_row(app: AppState, wait: &WaitLine) -> AnyElement {
    let form = app.form;
    let lead = match wait.kind {
        WaitKind::You => dot(app, Hue::You),
        WaitKind::Plan => mark(app, Mark::Next, Hue::Neutral),
        WaitKind::Info => mark(app, Mark::Na, Hue::Neutral),
    };
    let size = if form == Form::Phone { 13.0 } else { 12.5 };
    let reason_ink = if wait.kind == WaitKind::Info {
        Token::Ink3
    } else {
        Token::Ink
    };
    let (lead_w, id_w) = if form == Form::Terminal {
        (term::cells(2.0), term::cells(7.0))
    } else {
        (px(14.0), px(54.0))
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(view().w(lead_w).flex_shrink_0().items_center().child(lead))
        .child(
            view()
                .w(id_w)
                .flex_shrink_0()
                .child(code(app, wait.id.clone(), size - 1.0, Token::Ink).truncate()),
        )
        .child(
            words(app, wait.text.clone(), size, reason_ink)
                .flex_1()
                .min_w(px(0.0))
                .line_height(px(size * 1.45)),
        )
        .into_element()
}

/// A notice that leads with its stamp: ANSWERED, ANSWERED ELSEWHERE, SENDING, UNKNOWN, REFUSED
/// (`.notice` with a `.stamp`), on its hue's ground, its recovery control at the end.
fn stamped_notice(app: AppState, notice: Notice) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 13.5 } else { 12.5 };
    let sentence =
        view()
            .flex_1()
            .min_w(px(0.0))
            .child(rich_at(app, &notice.body, size, color(Token::Ink)));
    let retry = notice
        .retry
        .map(|retry| view().flex_row().flex_shrink_0().child(button(app, &retry)));
    let row = view()
        .flex_row()
        .items_start()
        .children(notice.stamp.map(|label| stamp(app, label, notice.hue)));
    if form == Form::Terminal {
        return row
            .child(term::space(1.0))
            .child(sentence)
            .children(retry.map(|retry| view().flex_row().child(term::space(2.0)).child(retry)))
            .into_element();
    }
    row.gap(px(10.0))
        .px(px(14.0))
        .py(px(10.0))
        .rounded(px(if form == Form::Phone { 10.0 } else { 8.0 }))
        .bg(hue_fill(notice.hue))
        .child(sentence)
        .children(retry)
        .into_element()
}

/// The task's goal and place: goal and plan line beside the mini graph (desktop inline), or the
/// graph under them; without a graph the words alone. Loading: placeholder bars and the
/// lazy-detail note. Absent: what is not recorded.
fn background(app: AppState, part: Background, place: Place) -> AnyElement {
    let form = app.form;
    match part {
        Background::Loading {
            skeleton_count,
            note,
        } => view()
            .label("Loading")
            .gap(px(8.0))
            .child(skeleton(
                app,
                &PLACE_BARS[..usize::from(skeleton_count).min(PLACE_BARS.len())],
            ))
            .child(illus(app, note))
            .into_element(),
        Background::Absent { note } => words(
            app,
            note,
            if form == Form::Phone { 14.0 } else { 12.5 },
            Token::Ink3,
        )
        .into_element(),
        Background::Ready {
            goal,
            plan_line,
            graph: mini,
        } => {
            let plan_line: Rich = plan_line.into_iter().map(|run| run.dim()).collect();
            let goal_size = if form == Form::Phone { 14.5 } else { 12.5 };
            let words_part = view()
                .gap(px(8.0))
                .when(!goal.is_empty(), |v| {
                    v.child(rich_at(app, &goal, goal_size, color(Token::Ink2)).max_w(px(620.0)))
                })
                .when(!plan_line.is_empty(), |v| {
                    v.child(rich_at(
                        app,
                        &plan_line,
                        if form == Form::Phone { 13.0 } else { 12.0 },
                        color(Token::Ink3),
                    ))
                });
            let Some(mini) = mini else {
                return words_part.into_element();
            };
            if place == Place::Wide && form == Form::Desktop {
                return view()
                    .flex_row()
                    .items_start()
                    .gap(px(22.0))
                    .child(view().flex_1().min_w(px(0.0)).child(words_part))
                    .child(
                        view()
                            .w(px(300.0))
                            .flex_shrink_0()
                            .child(graph::mini(app, mini)),
                    )
                    .into_element();
            }
            view()
                .child(words_part)
                .child(
                    view()
                        .mt(px(12.0))
                        .when(form == Form::Terminal, |v| v.child(term::blank()))
                        .child(graph::mini(app, mini)),
                )
                .into_element()
        }
    }
}
