//! The selected task's story in Now's side column and on the phone's task page (`.insp`): its
//! Story (header, a Blocked task's reason, detail, reply box and next steps, stepper, body and
//! key/value facts, every engine operation and this device's last operation's outcome), the
//! Evidence list of its artifacts, and the Session hand-off with the run's terminal capture,
//! each with its detail's loading, stale or failed state.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::text::span;
use crate::model::work::{
    self, BlockedCard, Dependency, DetailNote, Fact, Inspector, LiveEvidence, LiveSession,
    LiveStory, Press, ReplyBox, Stage,
};
use crate::model::{Form, Hue};
use crate::screens::markdown::{markdown, provide_styles};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::{loading, notice};
use crate::ui::kit::table::{kv, steps};
use crate::ui::kit::text::{dim, h3, mono, plain, rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::context::tail;
use super::parts::{model_button, press, press_id, pressable, quiet};
use crate::ui::kit::more::reasons;

/// The phone's least touch target: a dependency id or the plan link is pressed as a button is.
const TARGET: f32 = 44.0;

/// `.insp`'s padding: the side column's 14 18 24 on desktop, 14 18 on the phone, a Cell in on a
/// terminal.
fn padded(app: AppState, child: AnyElement) -> AnyElement {
    let frame = view();
    match app.form {
        Form::Desktop => frame.px(px(18.0)).pt(px(14.0)).pb(px(24.0)),
        Form::Phone => frame.px(px(18.0)).py(px(14.0)),
        Form::Terminal => frame.px(px(8.0)),
    }
    .child(child)
    .into_element()
}

/// The inspector for whatever `inspector` holds now. A story is drawn in three parts so its
/// reply box survives the story's every change (an age ticking, a run starting): the parts
/// above and below it are drawn again, the box only when its step changes, keeping the words
/// being typed and the focus.
pub(crate) fn view_of(app: AppState, inspector: Memo<Option<Inspector>>) -> AnyElement {
    let story = computed(move || {
        inspector.with(|inspector| match inspector {
            Some(Inspector::LiveStory(story)) => Some((**story).clone()),
            _ => None,
        })
    });
    let is_story = computed(move || story.with(Option::is_some));
    dynamic(move || {
        if is_story.get() {
            untracked(|| story_parts(app, story))
        } else {
            untracked(|| {
                dynamic(move || {
                    inspector.with(|inspector| inspector.as_ref().map(|it| content(app, it)))
                })
            })
        }
    })
}

fn story_parts(app: AppState, story: Memo<Option<LiveStory>>) -> AnyElement {
    let reply =
        computed(move || story.with(|story| story.as_ref()?.blocked.as_ref()?.reply.clone()));
    let body = view()
        .child(dynamic(move || {
            story.with(|story| story.as_ref().map(|story| story_top(app, story)))
        }))
        .child(dynamic(move || {
            reply.get().map(|reply| untracked(|| reply_box(app, reply)))
        }))
        .child(dynamic(move || {
            story.with(|story| story.as_ref().map(|story| story_rest(app, story)))
        }))
        .into_element();
    padded(app, body)
}

/// The inspector's body for `inspector`, padded as `.insp` is.
fn content(app: AppState, inspector: &Inspector) -> AnyElement {
    let body = match inspector {
        Inspector::Loading => loading(
            app,
            work::INSPECTOR_LOADING,
            &[(0.3, 10.0), (0.6, 18.0), (1.0, 60.0)],
        ),
        Inspector::Quiet(words) => return quiet(app, *words),
        Inspector::Unavailable(reason) => note(app, reason),
        Inspector::LiveStory(story) => view()
            .child(story_top(app, story))
            .child(story_rest(app, story))
            .into_element(),
        Inspector::LiveEvidence(evidence) => live_evidence(app, evidence),
        Inspector::LiveSession(session) => live_session(app, session),
    };
    padded(app, body)
}

fn gap(app: AppState) -> f32 {
    if app.form == Form::Terminal {
        0.0
    } else {
        12.0
    }
}

/// The model's action row under a story: each button by what it does, then why each command
/// that cannot run now cannot.
fn actions(app: AppState, buttons: &[work::Button]) -> AnyElement {
    view()
        .pt(px(if app.form == Form::Terminal {
            0.0
        } else {
            16.0
        }))
        .child(
            view().flex_row().flex_wrap().gap(px(8.0)).children(
                buttons
                    .iter()
                    .map(|button| pressable(app, button, false))
                    .collect::<Vec<_>>(),
            ),
        )
        .children(reasons(app, buttons))
        .into_element()
}

/// A goal or summary paragraph under the title.
fn paragraph(app: AppState, child: AnyElement) -> AnyElement {
    view()
        .pb(px(if app.form == Form::Terminal {
            0.0
        } else {
            14.0
        }))
        .child(child)
        .into_element()
}

/// A detail's loading, stale or failed line in its hue.
fn note(app: AppState, note: &DetailNote) -> AnyElement {
    let words = rich_small(app, &vec![span(note.text.clone()).hue(note.hue)]);
    view()
        .pb(px(if app.form == Form::Terminal {
            0.0
        } else {
            10.0
        }))
        .child(words)
        .into_element()
}

fn space(app: AppState, pixels: f32) -> Points {
    px(if app.form == Form::Terminal {
        0.0
    } else {
        pixels
    })
}

/// A Blocked task's card: the engine's reason on the needs-you ground and its detail as served.
/// The reply box and the other next steps follow it.
fn blocked(app: AppState, card: &BlockedCard) -> AnyElement {
    view()
        .gap(space(app, 8.0))
        .pb(space(app, 8.0))
        .test_id("blocked-card")
        .child(notice(
            app,
            Hue::You,
            &vec![span(card.reason.clone()).bold()],
            None,
        ))
        .children(
            card.detail
                .as_ref()
                .map(|detail| plain(app, detail.clone()).into_element()),
        )
        .into_element()
}

/// The card's next steps other than a reply box's, one button each, the first primary
/// (`btn-next-<n>`). A forced step reads "… anyway"; the app asks once before it runs.
fn next_steps(app: AppState, card: &BlockedCard) -> Option<AnyElement> {
    (!card.next.is_empty()).then(|| {
        view()
            .flex_row()
            .flex_wrap()
            .gap(px(8.0))
            .pb(space(app, 14.0))
            .children(
                card.next
                    .iter()
                    .map(|button| pressable(app, button, false))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    })
}

/// Sends the box's words as its step, then empties the box; nothing goes out empty. The step
/// takes the task out of Blocked and the box with it, so beside the list the keyboard focus goes
/// to the task's row first rather than out of the app.
fn send_reply(app: AppState, draft: Signal<String>, reply: &ReplyBox) {
    let words = draft.get_untracked();
    if words.trim().is_empty() {
        return;
    }
    draft.set(String::new());
    if app.form != Form::Phone {
        app.focused_row.set(Some(reply.task.to_string()));
    }
    press(app, reply.press(words));
}

/// The recommended reply in place on the card (`field-reply`, its send `btn-next-<n>`): one
/// raised box with the field and the send, Return in the field sends. Its words live here, so
/// the story changing around it keeps them.
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
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2))
            .focus_within(|s| s.border_color(color(Token::Focus))),
    };
    view()
        .pb(space(app, 10.0))
        .child(boxed.child(entry).child(send))
        .into_element()
}

/// The story above the reply box: the header, the title, the detail's state and the Blocked
/// card's reason and detail.
fn story_top(app: AppState, story: &LiveStory) -> AnyElement {
    let mut out: Vec<AnyElement> = vec![
        head(
            app,
            story.id.clone(),
            Some((story.who, story.who_hue)),
            story.plan.clone(),
            story.changed.clone(),
        ),
        title(app, story.title.clone()),
    ];
    if let Some(detail) = &story.note {
        out.push(note(app, detail));
    }
    if let Some(card) = &story.blocked {
        out.push(blocked(app, card));
    }
    view().children(out).into_element()
}

/// The story below the reply box: the card's other next steps, the stepper, the body, the
/// facts, this device's last operation and every engine operation.
fn story_rest(app: AppState, story: &LiveStory) -> AnyElement {
    let mut out: Vec<AnyElement> = Vec::new();
    if let Some(card) = &story.blocked {
        out.extend(next_steps(app, card));
    }
    if !story.summary.is_empty() {
        out.push(paragraph(app, rich_small(app, &story.summary)));
    }
    out.push(stepper(app, &story.stages));
    if let Some(source) = &story.body {
        // The task's canonical Markdown, laid out as the transcripts lay out Markdown.
        provide_styles(app);
        out.push(paragraph(app, markdown(app, source, false)));
    }
    let rows = story
        .facts
        .iter()
        .map(|(label, value)| {
            let value = match value {
                Fact::Text(words) => rich(app, words),
                Fact::Tasks(tasks) => ids(app, tasks),
                Fact::Answer(button) => view()
                    .flex_row()
                    .child(model_button(app, button, "btn-answer".into(), false))
                    .into_element(),
            };
            ((*label).to_owned(), value)
        })
        .collect();
    out.push(
        view()
            .pt(space(app, 14.0))
            .child(kv(app, rows))
            .into_element(),
    );
    if let Some(sent) = &story.sent {
        out.push(
            view()
                .pt(space(app, 12.0))
                .child(rich_small(
                    app,
                    &vec![span(sent.text.clone()).hue(sent.hue)],
                ))
                .into_element(),
        );
    }
    if !story.actions.is_empty() {
        out.push(actions(app, &story.actions));
    }
    view().children(out).into_element()
}

/// The selected daemon task's artifacts: each one's title over its source reference, then the
/// way to every task's artifacts with this task's token filled in.
fn live_evidence(app: AppState, evidence: &LiveEvidence) -> AnyElement {
    view()
        .child(h3(app, evidence.title.clone(), None))
        .children(evidence.note.as_ref().map(|detail| note(app, detail)))
        .children(evidence.empty.map(|empty| dim(app, empty).into_element()))
        .when(!evidence.rows.is_empty(), |list| {
            list.child(steps(
                app,
                evidence
                    .rows
                    .iter()
                    .map(|(title, source)| {
                        let row = view()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(8.0))
                            .child(plain(app, title.clone()))
                            .child(mono(app, source.clone()).text_color(color(Token::Ink3)));
                        (Mark::Na, Hue::Neutral, row.into_element())
                    })
                    .collect(),
            ))
        })
        .when(app.form == Form::Terminal, |list| list.child(term::blank()))
        .child(
            view()
                .flex_row()
                .when(app.form != Form::Terminal, |row| row.mt(px(10.0)))
                .child(pressable(app, &evidence.find, false)),
        )
        .into_element()
}

/// The selected daemon task's Session tab: the hand-off to its transcript, its run, and the
/// captured log tail.
fn live_session(app: AppState, session: &LiveSession) -> AnyElement {
    view()
        .gap(px(gap(app)))
        .child(dim(app, session.description))
        .children(session.note.as_ref().map(|detail| note(app, detail)))
        .children(session.open.as_ref().map(|open| {
            view()
                .flex_row()
                .child(model_button(app, open, "btn-session-open".into(), false))
                .into_element()
        }))
        .when(!session.run.is_empty(), |panel| {
            panel.child(rich_small(app, &session.run))
        })
        .children(
            session
                .tail
                .as_ref()
                .map(|captured| tail(app, captured, Some("btn-live-session".into()))),
        )
        .children(session.empty.map(|empty| dim(app, empty).into_element()))
        .into_element()
}

/// The id, who acts next (in the Daemon state), the plan it belongs to (a press opens the plan),
/// and the changed-since chip.
fn head(
    app: AppState,
    id: impl Into<SharedString>,
    who: Option<(&'static str, Hue)>,
    plan: Option<(String, Press)>,
    changed: Option<String>,
) -> AnyElement {
    let form = app.form;
    let plan = plan.map(|(label, action)| {
        let words = text(label.clone())
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        let words = if form == Form::Terminal {
            words
        } else {
            words.font_family(FONT_TEXT).text_size(px(12.0))
        };
        view()
            .when(form == Form::Phone, |link| {
                link.justify_center().min_h(px(TARGET))
            })
            .cursor_pointer()
            .role(AccessibilityRole::Link)
            .label(label)
            .test_id(&press_id(&action))
            .on_press(move || press(app, action.clone()))
            .child(words)
            .into_element()
    });
    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(px(8.0))
        // Ids are one token: cut at the edge, never broken across lines (TT-19).
        .child(
            mono(app, id)
                .number_of_lines(1u32)
                .truncate()
                .text_color(color(Token::Ink3)),
        )
        .children(who.map(|(who, hue)| chip(app, who, hue)))
        .children(plan)
        .children(changed.map(|changed| chip(app, changed, Hue::Neutral)))
        .into_element()
}

/// The task's title (`.insp h2`): 16 pt bold on desktop, 18 pt on the phone, bold in a terminal.
fn title(app: AppState, title: impl Into<SharedString>) -> AnyElement {
    let words = text(title.into())
        .role(AccessibilityRole::Header)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    match app.form {
        Form::Terminal => words.into_element(),
        form => words
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 18.0 } else { 16.0 }))
            .letter_spacing(px(-0.16))
            .mt(px(2.0))
            .mb(px(2.0))
            .into_element(),
    }
}

/// Run, gate, review, land side by side (`.stepper`): each stage's name in faint capitals, its
/// mark and state word, its detail in mono; the active stage under a 2 pt line in its hue.
fn stepper(app: AppState, stages: &[Stage; 4]) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    let columns = stages.iter().map(|stage| {
        let edge = if stage.active {
            hue_ink(stage.hue)
        } else {
            color(Token::Rule)
        };
        let label = text(stage.label)
            .uppercase()
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        let word = text(stage.word)
            .number_of_lines(1u32)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(Token::Ink));
        let detail = text(stage.detail.clone())
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(Token::Ink3));
        let (label, word, detail) = if form == Form::Terminal {
            (label, word, detail)
        } else {
            (
                label
                    .font_family(FONT_TEXT)
                    .text_size(px(if phone { 12.0 } else { 11.0 }))
                    .letter_spacing(px(0.66)),
                word.font_family(FONT_TEXT)
                    .text_size(px(if phone { 16.0 } else { 13.0 })),
                detail
                    .font_family(FONT_MONO)
                    .text_size(px(if phone { 12.0 } else { 11.0 })),
            )
        };
        let mark_hue = if stage.active {
            stage.hue
        } else {
            Hue::Neutral
        };
        view()
            .flex_1()
            .min_w(px(0.0))
            .when(form != Form::Terminal, |column| {
                column
                    .pr(px(10.0))
                    .pt(px(if stage.active { 8.0 } else { 9.0 }))
                    .border_t(px(if stage.active { 2.0 } else { 1.0 }))
                    .border_t_color(edge)
            })
            .role(AccessibilityRole::Text)
            .label(format!("{} {} {}", stage.label, stage.word, stage.detail))
            .child(label)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .py(px(if form == Form::Terminal { 0.0 } else { 2.0 }))
                    .child(mark(app, stage.mark, mark_hue))
                    .child(word),
            )
            .child(detail)
            .into_element()
    });
    view()
        .flex_row()
        .gap(px(if form == Form::Terminal { 8.0 } else { 0.0 }))
        .pt(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .pb(px(if form == Form::Terminal { 0.0 } else { 16.0 }))
        .children(columns.collect::<Vec<_>>())
        .into_element()
}

/// Each dependency's id, mark and status word (`T-14 ◐ running`); pressing a known one opens its
/// task, an id corrald does not know is plain text.
fn ids(app: AppState, dependencies: &[Dependency]) -> AnyElement {
    let form = app.form;
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(if form == Form::Terminal {
            term::cells(2.0)
        } else {
            px(12.0)
        })
        .children(
            dependencies
                .iter()
                .map(|dependency| {
                    let spoken = format!("{} {}", dependency.id, dependency.status);
                    let item = view()
                        .flex_row()
                        .items_center()
                        .gap(if form == Form::Terminal {
                            term::cells(1.0)
                        } else {
                            px(4.0)
                        })
                        .test_id(&format!("dep-{}", dependency.id))
                        .child(
                            mono(app, dependency.id.clone())
                                .number_of_lines(1u32)
                                .truncate(),
                        )
                        .children(dependency.mark.map(|it| mark(app, it, Hue::Neutral)))
                        .child(dim(app, dependency.status));
                    match dependency.action.clone() {
                        Some(action) => item
                            .when(form == Form::Phone, |link| link.min_h(px(TARGET)))
                            .cursor_pointer()
                            .role(AccessibilityRole::Link)
                            .label(spoken)
                            .on_press(move || press(app, action.clone()))
                            .focus_visible(|s| {
                                s.outline(px(2.0))
                                    .outline_color(color(Token::Focus))
                                    .outline_offset(px(1.0))
                            })
                            .into_element(),
                        None => item
                            .role(AccessibilityRole::Text)
                            .label(spoken)
                            .into_element(),
                    }
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}
