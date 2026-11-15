//! Situation: the question as a lede, its deadline, the tasks waiting on the answer, the move
//! corrald records for it, and the answer's stamp.

use nsh_core::prelude::*;

use super::sheet::Place;
use super::{attention, button, code, rich_at, ring, words};
use crate::model::board::Mark;
use crate::model::inbox::{Notice, Situation, WaitKind, WaitLine};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::band::band_head;
use crate::ui::kit::chip::stamp;
use crate::ui::kit::marks::{dot, hue_fill, mark};
use crate::ui::kit::page_phone::short_window;
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// The question as a lede (14 pt, 1.55; the phone's 15.5 pt), its deadline, the waits beside it
/// (desktop inline) or under it, the attention move, the answer's stamp under all of them. In a
/// narrow window, and on a phone held sideways, the waits fold to one line so the options keep
/// the first screen.
pub(super) fn situation(app: AppState, part: Situation, place: Place) -> AnyElement {
    let form = app.form;
    let lede = match form {
        Form::Phone => words(app, part.text.clone(), 15.5, Token::Ink).line_height(px(23.0)),
        _ => words(app, part.text.clone(), 14.0, Token::Ink)
            .line_height(px(21.7))
            .max_w(px(640.0)),
    };
    let lede = view()
        .child(lede)
        .children(part.deadline.clone().map(|deadline| {
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
    let waits = match form {
        Form::Desktop if place == Place::Narrow => folding(app, &part),
        Form::Phone => {
            let part = part.clone();
            dynamic(move || {
                let fold = short_window();
                Some(untracked(|| {
                    if fold {
                        folding(app, &part)
                    } else {
                        waits_list(app, &part)
                    }
                }))
            })
        }
        _ => waits_list(app, &part),
    };
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
        .children(part.attention.map(|line| {
            view()
                .mt(px(16.0))
                .when(form == Form::Terminal, |v| v.child(term::blank()))
                .child(attention::block(app, line))
        }))
        .children(part.notice.map(|notice| {
            view()
                .mt(px(12.0))
                .when(form == Form::Terminal, |v| v.child(term::blank()))
                .child(stamped_notice(app, notice))
        }))
        .into_element()
}

/// The waits under their head, one row each, or what is recorded when none wait.
fn waits_list(app: AppState, part: &Situation) -> AnyElement {
    let form = app.form;
    let head = match form {
        Form::Terminal => h3(app, part.waits_title, None),
        _ => band_head(app, None, part.waits_title, None, Hue::Neutral),
    };
    let size = if form == Form::Phone { 13.0 } else { 12.5 };
    view()
        .child(head)
        .child(
            view()
                .mt(px(if form == Form::Desktop { 12.0 } else { 6.0 }))
                .gap(px(4.0))
                .children(part.waits.iter().map(|wait| wait_row(app, wait)))
                .children(
                    part.waits_empty
                        .map(|none| words(app, none, size, Token::Ink3)),
                ),
        )
        .into_element()
}

/// The waits as one line, "Waiting on this answer · T-871, T-872", the ids in mono; pressing it
/// opens the full list in place and pressing again folds it. With nothing waiting the line says
/// so and does not open.
fn folding(app: AppState, part: &Situation) -> AnyElement {
    if part.waits.is_empty() {
        return waits_list(app, part);
    }
    let phone = app.form == Form::Phone;
    let size = if phone { 13.0 } else { 12.5 };
    let open = signal(false);
    let ids = part
        .waits
        .iter()
        .map(|wait| wait.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let full = waits_list(app, part);
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .min_h(px(if phone { 44.0 } else { 24.0 }))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(format!("{}: {ids}", part.waits_title))
        .accessibility_state(move || AccessibilityState {
            expanded: Some(open.get()),
            ..AccessibilityState::default()
        })
        .test_id("btn-waits")
        .on_press(move || open.update(|open| *open = !*open))
        .rounded(px(4.0))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .child(
            words(app, part.waits_title, size, Token::Ink2)
                .flex_shrink_0()
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(words(app, "·", size, Token::Ink3).flex_shrink_0())
        .child(
            code(app, ids, size - 1.0, Token::Ink)
                .flex_1()
                .min_w(px(0.0))
                .truncate(),
        )
        .child(
            view()
                .flex_shrink_0()
                .rotate(move || deg(if open.get() { 90.0 } else { 0.0 }))
                .child(icon(Icon::Chevron, 12.0, color(Token::Ink3))),
        );
    view()
        .child(line)
        .child(
            view()
                .mt(px(8.0))
                .style_when(move || !open.get(), |s| s.hidden())
                .accessibility_elements_hidden(move || !open.get())
                .child(full),
        )
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
