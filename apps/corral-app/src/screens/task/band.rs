//! The decision band (`.decide`), at the top of the task page and of the task inspector: what is
//! happening in bold, the plain phrase under it, then what the owner can do there. A Blocked
//! task: the engine's detail, the reply box (`field-reply`) and the next steps (`btn-next-<n>`).
//! A review the owner decides: the Onto line and the decide bar (`btn-approve`,
//! `btn-land-anyway`). Anything else: its one next step (`next-<id>`). Every step stays
//! pressable; the engine decides, and a refusal opens the refusal sheet.
//!
//! The band is drawn in parts so the reply box survives every change around it (an age
//! ticking, a run starting): it is drawn again only when its step changes, keeping the words
//! being typed and the focus.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::tasks::band::{Band, Hold, band};
use crate::model::work::{self, ReplyBox};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::more::press;
use crate::ui::kit::next::next_button;
use crate::ui::kit::text::plain;
use crate::ui::kit::work::pressable;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::candidate::{Ctx, decide};

/// The phone's least touch target.
const TARGET: f32 = 44.0;

/// What the band holds, apart from the words that change with every tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Blocked,
    Decide,
    Step,
}

fn space(app: AppState, pixels: f32) -> Points {
    px(if app.form == Form::Terminal {
        0.0
    } else {
        pixels
    })
}

/// The band of task `id`. `flat` draws it without its ground, as the inspector's first block.
pub(crate) fn decision(app: AppState, id: Id, flat: bool) -> AnyElement {
    let form = app.form;
    let model = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| band(feed, id.as_str())))
    };
    let kind = computed(move || {
        model.with(|band| {
            band.as_ref().map(|band| match band.hold {
                Hold::Blocked(_) => Kind::Blocked,
                Hold::Decide => Kind::Decide,
                Hold::Step(_) => Kind::Step,
            })
        })
    });
    // A terminal page's band runs to the page's edge, so its words are set a Cell in, as the
    // decide bar's are; the inspector has its own Cell.
    let inset = form == Form::Terminal && !flat;
    let words = dynamic(move || {
        model.with(|band| {
            band.as_ref().map(|band| {
                view()
                    .when(inset, |words| words.px(term::cells(1.0)))
                    .child(head(app, band))
                    .into_element()
            })
        })
    });
    let held = dynamic(move || {
        let kind = kind.get()?;
        let id = id.clone();
        Some(untracked(move || match kind {
            Kind::Blocked => blocked(app, model),
            Kind::Decide => decided(app, id),
            Kind::Step => step(app, model),
        }))
    });
    let band = view().test_id("task-band").child(words).child(held);
    match (form, flat) {
        (Form::Terminal, true) => band.into_element(),
        (Form::Terminal, false) => band.child(term::hrule()).into_element(),
        (Form::Desktop | Form::Phone, true) => band.pb(px(12.0)).into_element(),
        (Form::Desktop | Form::Phone, false) => band
            .py(px(12.0))
            .px(px(if form == Form::Phone { 18.0 } else { 20.0 }))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .bg(color(Token::Surface))
            .into_element(),
    }
}

/// What is happening in bold, the phrase under it.
fn head(app: AppState, band: &Band) -> AnyElement {
    let what = plain(app, band.what.clone()).font_weight(FontWeight::BOLD);
    let how = (!band.how.is_empty()).then(|| {
        text(band.how.clone())
            .text_color(color(Token::Ink2))
            .when(app.form != Form::Terminal, |t| {
                t.font_family(FONT_TEXT).text_size(px(12.5))
            })
    });
    view()
        .gap(space(app, 4.0))
        .pb(space(app, 8.0))
        .child(what)
        .children(how)
        .into_element()
}

/// A Blocked task's detail, its reply box and its other next steps, the first one primary.
fn blocked(app: AppState, model: Memo<Option<Band>>) -> AnyElement {
    let card = computed(move || {
        model.with(|band| match band.as_ref().map(|band| &band.hold) {
            Some(Hold::Blocked(card)) => Some(card.clone()),
            _ => None,
        })
    });
    let reply = computed(move || card.with(|card| card.as_ref()?.reply.clone()));
    let detail = dynamic(move || {
        card.with(|card| {
            card.as_ref()?.detail.clone().map(|detail| {
                view()
                    .pb(space(app, 8.0))
                    .child(plain(app, detail))
                    .into_element()
            })
        })
    });
    let reply_box = dynamic(move || reply.get().map(|reply| untracked(|| reply_box(app, reply))));
    let steps = dynamic(move || {
        card.with(|card| {
            let card = card.as_ref()?;
            (!card.next.is_empty()).then(|| {
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(8.0))
                    .children(
                        card.next
                            .iter()
                            .map(|button| pressable(app, button, false))
                            .collect::<Vec<_>>(),
                    )
                    .into_element()
            })
        })
    });
    view()
        .test_id("blocked-card")
        .child(detail)
        .child(reply_box)
        .child(steps)
        .into_element()
}

/// Sends the box's words as its step, then empties the box; nothing goes out empty.
fn send_reply(app: AppState, draft: Signal<String>, reply: &ReplyBox) {
    let words = draft.get_untracked();
    if words.trim().is_empty() {
        return;
    }
    draft.set(String::new());
    press(app, reply.press(words));
}

/// The recommended reply in place (`field-reply`, its send `btn-next-<n>`): one raised box with
/// the field and the send; Return in the field sends.
fn reply_box(app: AppState, reply: ReplyBox) -> AnyElement {
    let form = app.form;
    let draft = signal(String::new());
    let empty = computed(move || draft.with(|words| words.trim().is_empty()));
    let on_enter = reply.clone();
    let handle = element_ref();
    app.focus_when_asked(work::REPLY_FIELD, handle);
    let entry = input()
        .test_id(work::REPLY_FIELD)
        .element_ref(&handle)
        .label(reply.label.clone())
        .placeholder(reply.placeholder)
        .value(move || draft.get())
        .on_change(move |typed: &str| draft.set(typed.to_owned()))
        .on_submit(move |_: &str| send_reply(app, draft, &on_enter))
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    let entry = match form {
        Form::Terminal => entry.h(term::rows(1.0)),
        Form::Desktop => entry.font_family(FONT_TEXT).text_size(px(13.0)).h(px(30.0)),
        Form::Phone => entry
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .min_h(px(TARGET)),
    };
    let id = format!("btn-next-{}", reply.index);
    let label = reply.send.clone();
    let send = dynamic(move || {
        let reply = reply.clone();
        button_with(
            app,
            label.clone(),
            ButtonKind::Primary,
            ButtonOptions {
                icon: Some(Icon::Send),
                disabled: empty.get(),
                ..ButtonOptions::default()
            },
            id.clone(),
            move || send_reply(app, draft, &reply),
        )
    });
    let boxed = view().flex_row().items_center().test_id("blocked-reply");
    let boxed = match form {
        Form::Terminal => boxed
            .gap(term::cells(1.0))
            .border_1()
            .border_color(color(Token::Rule2))
            .focus_within(|s| s.border_color(color(Token::Focus))),
        Form::Desktop | Form::Phone => boxed
            .gap(px(8.0))
            .pl(px(12.0))
            .pr(px(6.0))
            .py(px(if form == Form::Phone { 6.0 } else { 5.0 }))
            .rounded(px(if form == Form::Phone { 12.0 } else { 8.0 }))
            .bg(color(Token::Bg))
            .border_1()
            .border_color(color(Token::Rule2))
            .focus_within(|s| s.border_color(color(Token::Focus))),
    };
    view()
        .pb(space(app, 10.0))
        .child(boxed.child(entry).child(send))
        .into_element()
}

/// A review the owner decides: the Onto line and the decide bar of the review model, for this
/// task. A phone keeps the Onto field and Run gate, Start review and Land anyway above its
/// buttons.
fn decided(app: AppState, id: Id) -> AnyElement {
    let cx = Ctx::new(app, Some(id));
    match app.form {
        Form::Phone => view()
            .child(decide::phone_secondary(&cx))
            .child(decide::decide(cx))
            .into_element(),
        Form::Desktop | Form::Terminal => decide::decide(cx),
    }
}

/// The one next step, when the status has one.
fn step(app: AppState, model: Memo<Option<Band>>) -> AnyElement {
    let next = computed(move || {
        model.with(|band| {
            let band = band.as_ref()?;
            match &band.hold {
                Hold::Step(Some(step)) => Some((band.id.clone(), step.clone())),
                _ => None,
            }
        })
    });
    dynamic(move || {
        next.get().map(|(id, step)| {
            untracked(|| {
                view()
                    .flex_row()
                    .child(next_button(
                        app,
                        id.as_str(),
                        &step,
                        app.form == Form::Phone,
                    ))
                    .into_element()
            })
        })
    })
}
