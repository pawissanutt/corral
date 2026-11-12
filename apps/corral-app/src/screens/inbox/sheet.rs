//! The handover sheet: its head, the S B A R Thread History bands in each form, the decide bar,
//! the focus page's head and context column. Each band is made again only when its own part of
//! the model changes.

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use super::{
    act, blind, button_as, code, evidence, graph, icon_for, kind, live, options, press, rich_at,
    ring, thread, words,
};
use crate::model::board::Mark;
use crate::model::inbox::{
    self, Action, Background, Band, BandKind, DecideBar, Header, Notice, Sheet, Situation, State,
    Wait, WaitKind,
};
use crate::model::{Destination, Env, Form, Hue, Rich};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::band::{band, band_head};
use crate::ui::kit::button::{ButtonOptions, button_with};
use crate::ui::kit::chip::{chip, illus, stamp};
use crate::ui::kit::marks::{dot, hue_fill, mark};
use crate::ui::kit::notice::skeleton;
use crate::ui::kit::page::{bar, bar_pane, context, qhead};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// Background's placeholder bars while task detail loads (`.skel` 90 %, 70 %, 80 % × 60).
const PLACE_BARS: [(f32, f32); 3] = [(0.9, 10.0), (0.7, 10.0), (0.8, 60.0)];

/// Where a band's content sits: the desktop inline sheet lays Situation and Background out in two
/// columns; the context column and the phone stack them.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    Wide,
    Narrow,
}

fn inline_sheet(state: &State, env: Env) -> Sheet {
    inbox::sheet_desk(state, env, false)
}

/// The Inbox's inline handover: head and bands scrolling, the decide bar pinned under them.
/// `compact` (a terminal, a narrow window): Situation and Background stack their parts, and the
/// decide bar keeps to one row of buttons with its attribution at the end of the scroll.
pub(super) fn inline(app: AppState, compact: bool) -> AnyElement {
    let sheet = blind(app, inline_sheet);
    let place = if compact { Place::Narrow } else { Place::Wide };
    let content = view()
        .child(main(app, sheet, place))
        .when(compact, |v| v.child(decide_note(app)))
        .into_element();
    bar_pane(app, "inbox-sheet", content, decide(app, compact))
}

/// The focus page's centre: head, S A R, the decide bar pinned. `with_side` puts the context
/// bands under them in the same scroll (a narrow terminal or window); `compact` as for
/// [`inline`].
pub(super) fn focus_main(app: AppState, with_side: bool, compact: bool) -> AnyElement {
    let sheet = blind(app, inbox::question);
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
        .when(compact, |v| v.child(decide_note(app)))
        .into_element();
    bar_pane(app, "question-main", content, decide(app, compact))
}

/// The focus page's context column: B, Thread and History under their heads.
pub(super) fn side(app: AppState) -> AnyElement {
    let sheet = blind(app, inbox::question);
    context(app, "question-side", side_content(app, sheet))
}

/// The phone's question body: the pager, the head, the bands in phone order, the thread sheet.
pub(super) fn phone_body(app: AppState) -> AnyElement {
    let sheet = blind(app, inbox::question);
    let (pager, next, talk) = sheet.with_untracked(|s| (s.pager, s.next.clone(), s.thread.clone()));
    view()
        .child(
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
                })),
        )
        .child(main(app, sheet, Place::Narrow))
        .child(thread::sheet(app, talk))
        .into_element()
}

/// The phone header's Ask-about control: the chat icon, a 44 pt target.
pub(super) fn ask_icon(app: AppState) -> AnyElement {
    let ask = untracked(|| {
        let env = app.env();
        app.inbox.with(|state| inbox::question(state, env).ask)
    });
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
}

/// The focus page's head (`.pagehead`): back to Inbox and the pager, then the keys, Ask about
/// this and Next at the trailing edge. Terminal: one row with a joined line under it, without
/// the keys (the shell's key legend shows them): back, the whole pager, then Next, and Ask
/// about this only on a wide grid, where it fits.
pub(super) fn focus_head(app: AppState) -> AnyElement {
    let head = untracked(|| {
        let env = app.env();
        app.inbox.with(|state| inbox::question(state, env))
    });
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
        let leading: Vec<AnyElement> = head
            .back
            .iter()
            .map(|back| whole(control(back)))
            .chain(
                head.pager
                    .map(|pager| whole(term::quiet(pager).into_element())),
            )
            .collect();
        let ask = head.ask.iter().map(|ask| {
            view()
                .flex_row()
                .flex_shrink_0()
                .style_when(move || !term::wide(), |s| s.hidden())
                .child(control(ask))
                .child(term::space(2.0))
                .into_element()
        });
        let next = head.next.iter().map(|next| whole(control(next)));
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
    let leading: Vec<AnyElement> = head
        .back
        .iter()
        .map(&control)
        .chain(
            head.pager
                .map(|pager| words(app, pager, 12.0, Token::Ink3).into_element()),
        )
        .collect();
    let trailing: Vec<AnyElement> = head
        .key_help
        .as_ref()
        .map(|keys| keys_line(app, keys))
        .into_iter()
        .chain(head.ask.iter().map(&control))
        .chain(head.next.iter().map(&control))
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
    let bands = sheet.with_untracked(|s| s.bands.clone());
    view()
        .child(live(header, move |header| head(app, header)))
        .children(
            bands
                .into_iter()
                .map(|lettered| band_of(app, sheet, lettered, place)),
        )
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
        let (preview, action) =
            sheet.with_untracked(|s| (s.thread_preview.clone(), s.thread_action.clone()));
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
                rich_at(app, &preview, 14.5, color(Token::Ink)).into_element(),
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
    let bands = sheet.with_untracked(|s| s.side_bands.clone());
    let terminal = app.form == Form::Terminal;
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
        BandKind::Thread => {
            let part = sheet.with_untracked(|s| s.thread.clone());
            thread::thread(app, part)
        }
        BandKind::History => {
            let part = sheet.with_untracked(|s| s.history.clone());
            thread::history(app, part)
        }
    }
}

/// The question as a lede (14 pt, 1.55; the phone's 15.5 pt), the waits beside it (desktop
/// inline) or under it, the answer's stamp under both.
fn situation(app: AppState, part: Situation, place: Place) -> AnyElement {
    let form = app.form;
    let lede = match form {
        Form::Phone => words(app, part.text, 15.5, Token::Ink).line_height(px(23.0)),
        _ => words(app, part.text, 14.0, Token::Ink)
            .line_height(px(21.7))
            .max_w(px(640.0)),
    };
    let waits_head = match form {
        Form::Terminal => h3(app, part.waits_title, None),
        _ => band_head(app, None, part.waits_title, None, Hue::Neutral),
    };
    let waits = view().child(waits_head).child(
        view()
            .mt(px(if form == Form::Desktop { 12.0 } else { 6.0 }))
            .gap(px(4.0))
            .children(part.waits.iter().map(|wait| wait_row(app, wait))),
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
fn wait_row(app: AppState, wait: &Wait) -> AnyElement {
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
                .child(code(app, wait.id, size - 1.0, Token::Ink)),
        )
        .child(
            words(app, wait.text, size, reason_ink)
                .flex_1()
                .min_w(px(0.0))
                .line_height(px(size * 1.45)),
        )
        .into_element()
}

/// A notice that leads with its stamp: ANSWERED, ANSWERED ELSEWHERE, QUEUED (`.notice` with a
/// `.stamp`), on its hue's ground.
fn stamped_notice(app: AppState, notice: Notice) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 13.5 } else { 12.5 };
    let sentence =
        view()
            .flex_1()
            .min_w(px(0.0))
            .child(rich_at(app, &notice.body, size, color(Token::Ink)));
    let row = view()
        .flex_row()
        .items_start()
        .children(notice.stamp.map(|label| stamp(app, label, notice.hue)));
    if form == Form::Terminal {
        return row.child(term::space(1.0)).child(sentence).into_element();
    }
    row.gap(px(10.0))
        .px(px(14.0))
        .py(px(10.0))
        .rounded(px(if form == Form::Phone { 10.0 } else { 8.0 }))
        .bg(hue_fill(notice.hue))
        .child(sentence)
        .into_element()
}

/// The task's goal and place: goal and plan line beside the mini graph (desktop inline), or the
/// graph under them. Loading: placeholder bars and the lazy-detail note.
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
        Background::Ready {
            goal,
            plan_line,
            graph: mini,
        } => {
            let plan_line: Rich = plan_line.into_iter().map(|run| run.dim()).collect();
            let goal_size = if form == Form::Phone { 14.5 } else { 12.5 };
            let words_part = view()
                .gap(px(8.0))
                .child(rich_at(app, &goal, goal_size, color(Token::Ink2)).max_w(px(620.0)))
                .child(rich_at(
                    app,
                    &plan_line,
                    if form == Form::Phone { 13.0 } else { 12.0 },
                    color(Token::Ink3),
                ));
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

/// The decide bar, made again as the answer state, the connection or the alternate answer
/// changes. Desktop: the stamp of an answered question first, then the buttons (the answer's
/// shortcut as a key cap beside it), the attribution right-aligned. Phone: the buttons sharing
/// the width. `compact` keeps it to one row of buttons, without the key cap and the
/// attribution ([`decide_note`] shows it at the end of the scroll). Off the phone, Ctrl+Enter
/// answers while the bar shows, as its key cap says, even from the alternate-answer field.
pub(super) fn decide(app: AppState, compact: bool) -> AnyElement {
    if app.form != Form::Phone {
        let anywhere = KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        };
        on_key_with("ctrl+enter", anywhere, move |_| act(app, Action::Answer));
    }
    live(decide_model(app), move |part| {
        decide_view(app, part, compact)
    })
}

fn decide_model(app: AppState) -> Memo<DecideBar> {
    computed(move || {
        let env = app.env();
        app.inbox.with(|state| inbox::decide_bar(state, env))
    })
}

/// The decide bar's attribution as the last block of a compact sheet's scroll: the command it
/// runs, the offline rule or the one-answer-wins line.
fn decide_note(app: AppState) -> AnyElement {
    let model = decide_model(app);
    let note = computed(move || model.with(|part| part.note.clone()));
    live(note, move |note| {
        if note.is_empty() {
            return view().into_element();
        }
        let words_of = rich_at(app, &note, 11.5, color(Token::Ink3)).line_height(px(15.5));
        if app.form == Form::Terminal {
            return view()
                .child(term::blank())
                .child(view().px(term::cells(1.0)).child(words_of))
                .into_element();
        }
        view()
            .px(px(24.0))
            .py(px(12.0))
            .child(words_of)
            .into_element()
    })
}

fn decide_view(app: AppState, part: DecideBar, compact: bool) -> AnyElement {
    let form = app.form;
    let mut actions: Vec<AnyElement> = Vec::new();
    if let Some(label) = part.stamp {
        actions.push(stamp(app, label, Hue::Ok));
    }
    for model in &part.buttons {
        let options = ButtonOptions {
            // The phone's ask-the-worker button keeps its icon and its own width; the others
            // share the rest of the bar.
            icon: (form == Form::Phone && model.action == Action::Thread).then_some(Icon::Ask),
            wide: form == Form::Phone && model.action != Action::Thread,
            ..ButtonOptions::default()
        };
        let control = button_as(app, model, options);
        actions.push(match model.shortcut {
            Some(keys) if form == Form::Desktop && !compact => view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(control)
                .child(shortcut(app, keys))
                .into_element(),
            _ => control,
        });
    }
    let note = (!part.note.is_empty() && !compact && form != Form::Phone).then(|| {
        rich_at(app, &part.note, 11.5, color(Token::Ink3))
            .text_right()
            .line_height(px(15.5))
            .into_element()
    });
    if form == Form::Terminal {
        return terminal_bar(actions);
    }
    bar(app, actions, note)
}

/// The terminal's decide bar: a joined line, then the controls whole and two Cells apart,
/// wrapping onto a further row rather than cutting a label where the pane is narrower than the
/// row (`page::bar` keeps one row). test_id "decide-bar", role toolbar.
fn terminal_bar(actions: Vec<AnyElement>) -> AnyElement {
    view()
        .test_id("decide-bar")
        .role(AccessibilityRole::Toolbar)
        .flex_none()
        .child(term::hrule())
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .px(term::cells(1.0))
                .children(actions.into_iter().map(|action| {
                    // 24 pt is two Cells of padding in a terminal.
                    view()
                        .flex_row()
                        .flex_shrink_0()
                        .pr(px(24.0))
                        .child(action)
                        .into_element()
                })),
        )
        .into_element()
}

/// A key cap (`.kbd`) after the answer button: the shortcut that answers.
fn shortcut(app: AppState, keys: &'static str) -> AnyElement {
    view()
        .px(px(4.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .accessibility_elements_hidden(true)
        .child(code(app, keys, 10.5, Token::Ink3).line_height(px(15.0)))
        .into_element()
}
