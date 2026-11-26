//! The dashboard's left half: what waits on the owner as cards (`model::home::cards`). One card
//! is open and holds the decision; the others are two lines with their one next step. A press
//! or `j` `k` opens a card, Enter opens its page. A card tells the app while it holds the
//! keyboard focus (`AppState::focused_row`), so Enter opens the card that has it.

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::home::{self, Card, Decision};
use crate::model::list;
use crate::model::pipeline::pipeline;
use crate::model::projects::Scope;
use crate::model::work::{self, Press, ReplyBox};
use crate::model::{Form, Id};
use crate::screens::agents::{Compact, ask_session, ask_view};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::more::press;
use crate::ui::kit::next::{more_button, next_button};
use crate::ui::kit::status::{look_mark, pipeline_marks};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::parts::{empty, figure, surface, watch_run, words};

/// The Needs you panel: one card per thing that waits, the selected one open.
pub(super) fn needs_you(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let form = app.form;
    let ids = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| home::needs_you(feed, &scope))
    });
    let open = computed(move || {
        ids.with(|ids| {
            app.home
                .with(|state| list::settled(ids, state.selected.as_ref()))
        })
    });
    let cards = dynamic(move || {
        let ids = ids.get();
        untracked(move || {
            view()
                .children(
                    ids.into_iter()
                        .map(|id| card(app, id, open))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    });
    let said = computed(move || {
        let none = ids.with(Vec::is_empty);
        app.feed
            .with(|feed| home::notice(feed, none, home::NOTHING_WAITS))
    });
    let said = dynamic(move || said.get().map(|words| empty(app, words)));
    let body = view().child(cards).child(said);
    // The head follows the count; the cards keep their state under it.
    let name = dynamic(move || {
        let count = ids.with(Vec::len);
        untracked(move || {
            text(format!("{} {count}", home::NEEDS_YOU))
                .role(AccessibilityRole::Header)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
                .when(form != Form::Terminal, |t| {
                    t.font_family(FONT_TEXT).text_size(px(12.5))
                })
                .into_element()
        })
    });
    match form {
        Form::Terminal => term::framed(view().px(term::cells(1.0)).child(body))
            .test_id("list-home")
            .child(term::top_title(name))
            .into_element(),
        Form::Desktop | Form::Phone => surface("list-home")
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .pt(px(8.0))
                    .pb(px(7.0))
                    .child(name)
                    .child(
                        words(form, NEEDS_NOTE, 12.0, Token::Ink3)
                            .flex_1()
                            .text_right(),
                    ),
            )
            .child(body)
            .into_element(),
    }
}

/// What the Needs you panel says beside its name on the desktop.
const NEEDS_NOTE: &str = "j k next · Enter opens the page";

/// One card: its head and either its two closed lines or, while open, its decision.
fn card(app: AppState, id: Id, open: Memo<Option<Id>>) -> AnyElement {
    let form = app.form;
    let key = id.clone();
    watch_run(app, id.clone());
    let facts = computed(move || app.feed.with(|feed| home::card(feed, &key)));
    let me = id.clone();
    let is_open = computed(move || open.with(|open| open.as_ref() == Some(&me)));
    let (test_id, record) = app.feed.with_untracked(|feed| {
        (
            list::row_id(&home::live_of(feed, &id)),
            home::record_of(feed, &id),
        )
    });
    let head = {
        let record = record.clone();
        dynamic(move || {
            let shown = facts.get();
            let opened = is_open.get();
            let record = record.clone();
            untracked(move || shown.map(|card| head(app, &card, opened, record)))
        })
    };
    let alert = dynamic(move || {
        facts.with(|card| {
            let reason = card.as_ref()?.attention.clone()?;
            Some(
                view()
                    .when(form == Form::Terminal, |line| line.pl(term::cells(2.0)))
                    .child(
                        dim(app, reason)
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::You)),
                    )
                    .into_element(),
            )
        })
    });
    let rest = {
        let id = id.clone();
        dynamic(move || {
            let opened = is_open.get();
            let session = facts.with(|card| card.as_ref().is_some_and(|card| card.session));
            let id = id.clone();
            untracked(move || {
                if !opened {
                    closed(app, facts)
                } else if session {
                    session_decision(app, id, facts)
                } else {
                    task_decision(app, id, facts)
                }
            })
        })
    };
    let handle = element_ref();
    let focused_row = app.focused_row;
    let (focused, blurred) = (test_id.clone(), test_id.clone());
    let select = id.clone();
    let spoken = facts.with_untracked(|card| {
        card.as_ref()
            .map(|card| {
                let spoken = format!("{} {}, {}, {}", card.id, card.title, card.word, card.why);
                match &card.attention {
                    Some(reason) => format!("{spoken}, {reason}"),
                    None => spoken,
                }
            })
            .unwrap_or_default()
    });
    effect(move || {
        if is_open.get() {
            after_answer(move || {
                handle.reveal(RevealOptions::default().vertical(ScrollAlignment::Nearest));
            });
        }
    });
    let frame = view()
        .element_ref(&handle)
        .test_id(&test_id)
        .role(AccessibilityRole::Button)
        .label(spoken)
        .accessibility_state(move || AccessibilityState {
            selected: is_open.get(),
            ..AccessibilityState::default()
        })
        .on_focus(move || focused_row.set(Some(focused.clone())))
        .on_blur(move || {
            focused_row.update(|row| {
                if row.as_deref() == Some(blurred.as_str()) {
                    *row = None;
                }
            })
        })
        .on_press(move || {
            let id = select.clone();
            app.home.update(|state| state.selected = Some(id));
        })
        .min_w(px(0.0))
        .child(head)
        .child(alert)
        .child(rest);
    let frame = match form {
        Form::Terminal => {
            let has_focus = term::focus_of(handle);
            view()
                .flex_row()
                .min_w(px(0.0))
                .child(
                    view()
                        .w(term::cells(1.0))
                        .flex_shrink_0()
                        .accessibility_elements_hidden(true)
                        .child(
                            text(move || {
                                if has_focus.get() {
                                    term::FOCUS_MARK
                                } else if is_open.get() {
                                    "▌"
                                } else {
                                    " "
                                }
                            })
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::You)),
                        ),
                )
                .child(frame.flex_1())
                .into_element()
        }
        Form::Desktop | Form::Phone => frame
            .relative()
            .cursor_pointer()
            .gap(px(5.0))
            .px(px(12.0))
            .pt(px(8.0))
            .pb(px(9.0))
            .border_t_1()
            .border_color(color(Token::Rule))
            .hover(|s| s.bg(color(Token::Hover)))
            .style_when(is_open, |s| s.bg(color(Token::Bg)).pb(px(12.0)))
            .child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .w(px(2.0))
                    .bg(color(Token::You))
                    .opacity(move || if is_open.get() { 1.0_f32 } else { 0.0_f32 }),
            )
            .into_element(),
    };
    crate::ui::kit::more::with_menu(app, record, frame)
}

/// A card's first line: mark, id, title with its tag, age; the open card adds Open page and
/// More.
fn head(app: AppState, card: &Card, opened: bool, record: work::Record) -> AnyElement {
    let form = app.form;
    let live = card.live.clone();
    let title = words(form, card.title.clone(), 13.0, Token::Ink).font_weight(FontWeight::SEMIBOLD);
    let tag = card
        .tag
        .clone()
        .map(|tag| figure(form, tag, 11.5, Token::Ink3).into_element());
    view()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 8.0 } else { 10.0 }))
        .child(look_mark(app, card.look, card.word))
        .child(figure(form, card.id.to_string(), 12.0, Token::Ink2))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(title.flex_shrink(1.0))
                .children(tag),
        )
        .child(figure(form, card.age.clone(), 11.5, Token::Ink3))
        .when(opened, |row| {
            row.children(crate::ui::ask::open_beside(app, &live))
                .child(button_with(
                    app,
                    "Open page",
                    ButtonKind::Ghost,
                    ButtonOptions::default(),
                    "btn-open-page",
                    move || app.open_live(live.clone()),
                ))
                .child(more_button(app, record, true))
        })
        .into_element()
}

/// A closed card's second line: why it waits, its project and profile, its one next step.
fn closed(app: AppState, facts: Memo<Option<Card>>) -> AnyElement {
    let form = app.form;
    dynamic(move || {
        let shown = facts.get();
        untracked(move || {
            shown.map(|card| {
                let shown_id = card.id.to_string();
                view()
                    .flex_row()
                    .items_center()
                    .min_w(px(0.0))
                    .gap(px(10.0))
                    .when(form == Form::Terminal, |row| row.pl(term::cells(2.0)))
                    .child(
                        words(form, card.why.clone(), 12.5, Token::Ink2)
                            .flex_1()
                            .flex_shrink(1.0),
                    )
                    .child(words(form, card.facts.clone(), 12.0, Token::Ink3).flex_shrink(2.0))
                    .children(
                        card.next
                            .as_ref()
                            .map(|next| next_button(app, &shown_id, next, false)),
                    )
                    .into_element()
            })
        })
    })
    .into_element()
}

/// A task's open card: the decision (reason, the engine's detail, the reply box or the next
/// steps), then its pipeline and facts. The reply box is drawn on its own, so the decision
/// changing around it keeps the words being typed and the focus.
fn task_decision(app: AppState, id: Id, facts: Memo<Option<Card>>) -> AnyElement {
    let form = app.form;
    let key = id.clone();
    let decision = computed(move || app.feed.with(|feed| home::decision(feed, &key)));
    let reply = computed(move || decision.with(|d| d.as_ref().and_then(|d| d.reply.clone())));
    let top = dynamic(move || {
        let shown = decision.get();
        untracked(move || shown.map(|decision| decision_top(app, &decision)))
    });
    let reply = dynamic(move || reply.get().map(|reply| untracked(|| reply_box(app, reply))));
    let steps = {
        let id = id.clone();
        dynamic(move || {
            let steps = decision.with(|d| d.as_ref().map(|d| d.steps.clone()).unwrap_or_default());
            let next = facts.with(|card| {
                card.as_ref()
                    .filter(|_| steps.is_empty())
                    .and_then(|card| card.next.clone())
            });
            let shown = id.to_string();
            untracked(move || {
                let mut buttons: Vec<AnyElement> =
                    steps.iter().map(|step| step_button(app, step)).collect();
                buttons.extend(next.map(|next| next_button(app, &shown, &next, false)));
                (!buttons.is_empty()).then(|| {
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(8.0))
                        .children(buttons)
                        .into_element()
                })
            })
        })
    };
    let pipeline_row = {
        let key = id.clone();
        let steps = computed(move || app.feed.with(|feed| feed.board.task(&key).map(pipeline)));
        dynamic(move || {
            let shown = steps.get();
            let said = facts.with(|card| card.as_ref().map(|card| card.facts.clone()));
            untracked(move || {
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .min_w(px(0.0))
                    .children(shown.map(|steps| pipeline_marks(app, &steps)))
                    .child(view().flex_1())
                    .children(said.map(|said| words(form, said, 12.0, Token::Ink3).into_element()))
                    .into_element()
            })
        })
    };
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .when(form == Form::Terminal, |body| body.pl(term::cells(2.0)))
        .child(top)
        .child(reply)
        .child(steps)
        .child(pipeline_row)
        .into_element()
}

/// The decision's head: what is happening in the needs-you hue, then the engine's detail as
/// served (a log excerpt keeps its lines).
fn decision_top(app: AppState, decision: &Decision) -> AnyElement {
    let form = app.form;
    let what = text(decision.what.clone())
        .font_weight(FontWeight::BOLD)
        .text_color(hue_ink(crate::model::hue::Hue::You));
    let what = match form {
        Form::Terminal => what,
        Form::Desktop | Form::Phone => what.font_family(FONT_TEXT).text_size(px(13.0)),
    };
    let detail = decision.detail.clone().map(|detail| {
        let words = text(detail)
            .number_of_lines(8u32)
            .text_color(color(Token::Ink2));
        match form {
            Form::Terminal => words.into_element(),
            Form::Desktop | Form::Phone => view()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(6.0))
                .bg(color(Token::Surface2))
                .child(words.font_family(FONT_MONO).text_size(px(11.5)))
                .into_element(),
        }
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(what)
        .children(detail)
        .into_element()
}

/// One served next step as its button (`btn-next-<n>`), the first primary. A forced step reads
/// "… anyway"; the app asks once before it runs.
fn step_button(app: AppState, step: &work::Button) -> AnyElement {
    let action = step.action.clone();
    let test_id = match &action {
        Some(Press::Next { index, .. }) => format!("btn-next-{index}"),
        Some(press @ Press::Question { .. }) => crate::ui::kit::work::press_id(press),
        _ => String::new(),
    };
    button_with(
        app,
        step.label.to_string(),
        match step.kind {
            work::ButtonKind::Primary => ButtonKind::Primary,
            work::ButtonKind::Ghost => ButtonKind::Ghost,
            work::ButtonKind::Quiet => ButtonKind::Quiet,
        },
        ButtonOptions::default(),
        test_id,
        move || {
            if let Some(action) = &action {
                press(app, action.clone());
            }
        },
    )
}

/// Sends the box's words as its step, then empties the box; nothing goes out empty. The step
/// takes the task out of Blocked and its card with it, so the keyboard focus goes to the card
/// first rather than out of the app.
fn send_reply(app: AppState, draft: Signal<String>, reply: &ReplyBox) {
    let words = draft.get_untracked();
    if words.trim().is_empty() {
        return;
    }
    draft.set(String::new());
    app.focused_row.set(Some(format!("task-{}", reply.task)));
    press(app, reply.press(words));
}

/// The recommended reply in place (`field-reply`, its send `btn-next-<n>`): one box with the
/// field and the send; Return in the field sends. Its words live here.
pub(super) fn reply_box(app: AppState, reply: ReplyBox) -> AnyElement {
    let form = app.form;
    let draft = signal(String::new());
    let empty = computed(move || draft.with(|words| words.trim().is_empty()));
    let on_enter = reply.clone();
    let handle = element_ref();
    app.focus_when_asked(work::REPLY_FIELD, handle);
    let entry = copyable_input(app, Some(handle), move || draft.get())
        .test_id(work::REPLY_FIELD)
        .label(reply.label.clone())
        .placeholder(reply.placeholder)
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
            .min_h(px(44.0)),
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
    let boxed = view()
        .flex_row()
        .items_center()
        .test_id("blocked-reply")
        .border_1()
        .border_color(color(Token::Rule2))
        .focus_within(|s| s.border_color(color(Token::Focus)));
    match form {
        Form::Terminal => boxed.gap(term::cells(1.0)),
        Form::Desktop | Form::Phone => boxed
            .gap(px(8.0))
            .pl(px(12.0))
            .pr(px(6.0))
            .py(px(if form == Form::Phone { 6.0 } else { 5.0 }))
            .rounded(px(if form == Form::Phone { 12.0 } else { 8.0 }))
            .bg(color(Token::Surface)),
    }
    .child(entry)
    .child(send)
    .into_element()
}

/// A session's open card: what it is doing or asks, then its own dock and composer, where its
/// question opens as an answer card in the composer's place.
fn session_decision(app: AppState, id: Id, facts: Memo<Option<Card>>) -> AnyElement {
    let form = app.form;
    let why = dynamic(move || {
        let said = facts.with(|card| {
            card.as_ref()
                .map(|card| (card.why.clone(), card.facts.clone()))
        });
        untracked(move || {
            said.map(|(why, facts)| {
                view()
                    .gap(px(4.0))
                    .when(!why.is_empty(), |lines| {
                        lines.child(
                            text(why)
                                .number_of_lines(4u32)
                                .text_color(hue_ink(crate::model::hue::Hue::You))
                                .when(form != Form::Terminal, |t| {
                                    t.font_family(FONT_TEXT).text_size(px(13.0))
                                }),
                        )
                    })
                    .when(!facts.is_empty(), |lines| {
                        lines.child(words(form, facts, 12.0, Token::Ink3))
                    })
                    .into_element()
            })
        })
    });
    let page = ask_view(app, id.clone());
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .when(form == Form::Terminal, |body| body.pl(term::cells(2.0)))
        .child(why)
        .child(
            view()
                .test_id("card-session")
                .min_w(px(0.0))
                .child(ask_session(app, id, page, Compact::Card)),
        )
        .into_element()
}
