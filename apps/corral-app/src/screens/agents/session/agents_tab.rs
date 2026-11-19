//! The Agents tab (`session::AgentsTab`): the session's child agents in the host's order, one
//! line each (state mark and word, id in bold mono, title, profile and model, how long it ran,
//! cost, context share, files), the selected one's facts under the list, the timeline of when
//! each ran, and the messages between them. What the host did not report reads `–`. While the
//! host is unreachable or stopped the rows are its last fleet, said so above them. Desktop and
//! phone set the lines in points (the phone in two lines), a terminal in Cells. test ids
//! `agent-<session>-child-<id>`, `agent-<session>-msg-<seq>`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{
    AgentDetail, AgentRowView, AgentsTab, Body, MessageList, SessionPage, Timeline,
};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::{empty, notice};
use crate::ui::kit::text::{h3, rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::hue_words;

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Agents(tab) => Some(tab.clone()),
            _ => None,
        })
    });
    let content =
        dynamic(move || shown.with(|tab| tab.as_ref().map(|tab| untracked(|| face(app, tab)))));
    let surface = scroll().test_id("session-agents").flex_1().min_h(px(0.0));
    match app.form {
        Form::Terminal => surface
            .child(view().px(term::cells(1.0)).child(content))
            .into_element(),
        Form::Desktop => surface
            .child(view().px(px(18.0)).py(px(12.0)).child(content))
            .into_element(),
        Form::Phone => surface
            .child(view().px(px(16.0)).py(px(10.0)).child(content))
            .into_element(),
    }
}

fn face(app: AppState, tab: &AgentsTab) -> AnyElement {
    let form = app.form;
    let gap = if form == Form::Terminal { 0.0 } else { 12.0 };
    let unbound = tab
        .unbound
        .clone()
        .map(|words| notice(app, Hue::Held, &vec![span(words)], None));
    let empty_state = tab
        .empty
        .clone()
        .map(|words| empty(app, "No agents", words, None));
    let list = (!tab.rows.is_empty()).then(|| {
        view()
            .role(AccessibilityRole::List)
            .label("Agents")
            .children(
                tab.rows
                    .iter()
                    .map(|row| line(app, row))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    });
    view()
        .gap(px(gap))
        .children(unbound)
        .children(empty_state)
        .children(list)
        .children(tab.detail.as_ref().map(|detail| self::detail(app, detail)))
        .children(
            tab.timeline
                .as_ref()
                .map(|timeline| self::timeline(app, timeline)),
        )
        .children(
            tab.messages
                .as_ref()
                .map(|messages| self::messages(app, messages)),
        )
        .into_element()
}

/// Quiet words of a cell; mono for ids, times and money.
fn cell(form: Form, words: String, ink: Token, mono: bool) -> Text {
    let words = text(words)
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(color(ink));
    match form {
        Form::Terminal => words,
        _ => words
            .font_family(if mono { FONT_MONO } else { FONT_TEXT })
            .text_size(px(if form == Form::Phone { 13.0 } else { 12.5 })),
    }
}

/// One agent's line, which selects it (its facts show under the list).
fn line(app: AppState, row: &AgentRowView) -> AnyElement {
    let form = app.form;
    let action = row.action.clone();
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mark(app, row.mark, row.hue))
        .child(
            cell(form, row.child.clone(), Token::Ink, true)
                .flex_shrink_0()
                .font_weight(FontWeight::BOLD),
        )
        .child(
            text(row.state)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .text_color(hue_words(form, row.hue))
                .when(form != Form::Terminal, |t| {
                    t.font_family(FONT_TEXT).text_size(px(12.0))
                }),
        )
        .child(cell(form, row.title.clone(), Token::Ink, false).flex_1());
    let facts = [
        row.profile_model.clone(),
        row.time.clone(),
        row.cost.clone(),
        row.ctx.clone(),
        format!("files {}", row.files),
    ]
    .join(" · ");
    let facts = cell(
        form,
        facts,
        if row.outside {
            Token::Held
        } else {
            Token::Ink3
        },
        true,
    );
    let now = cell(form, row.now.clone(), Token::Ink2, false);
    let body = match form {
        Form::Phone => view().gap(px(2.0)).child(head).child(now).child(facts),
        Form::Desktop | Form::Terminal => view().child(head.child(facts.flex_shrink(2.0))).child(
            view()
                .pl(if form == Form::Terminal {
                    term::cells(2.0)
                } else {
                    px(20.0)
                })
                .child(now),
        ),
    };
    let selected = row.selected;
    let pressable = view()
        .test_id(&row.test_id)
        .role(AccessibilityRole::Button)
        .label(format!("{}, {}, {}", row.child, row.state, row.title))
        .accessibility_state(AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .cursor_pointer()
        .on_press(move || run(app, action.clone()))
        .when(selected, |line| line.bg(color(Token::Sel)))
        .child(body);
    match form {
        Form::Terminal => view()
            .flex_row()
            .child(text(if selected { "▌" } else { " " }).text_color(color(Token::Ink)))
            .child(pressable.flex_1().min_w(px(0.0)))
            .into_element(),
        Form::Desktop => pressable
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .into_element(),
        Form::Phone => pressable
            .py(px(8.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .into_element(),
    }
}

/// The selected agent's facts: what it owns and waits for, its verdict, a pending model, edits
/// outside its own files and its error.
fn detail(app: AppState, detail: &AgentDetail) -> AnyElement {
    let form = app.form;
    let mut lines: Vec<(String, AdaptiveColor)> = Vec::new();
    if let Some(owns) = &detail.owns {
        lines.push((owns.clone(), color(Token::Ink2)));
    }
    if detail.asks {
        lines.push(("▲ Waits for your answer".into(), hue_ink(Hue::You)));
    }
    if let Some((hue, words)) = &detail.verdict {
        lines.push((words.clone(), hue_ink(*hue)));
    }
    if let Some(model) = &detail.pending_model {
        lines.push((
            format!("Switches to {model} at its next turn"),
            color(Token::Ink2),
        ));
    }
    if let Some(outside) = &detail.outside {
        lines.push((format!("! {outside}"), hue_ink(Hue::Held)));
    }
    if let Some(error) = &detail.error {
        lines.push((format!("✗ {error}"), hue_ink(Hue::Fail)));
    }
    view()
        .test_id("session-agent-detail")
        .child(h3(
            app,
            format!("{} · {}", detail.child, detail.title),
            None,
        ))
        .children(lines.into_iter().map(|(words, ink)| {
            let words = text(words).text_color(ink);
            if form == Form::Terminal {
                words.into_element()
            } else {
                words
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .into_element()
            }
        }))
        .into_element()
}

/// When each agent ran: one bar per agent on the shared axis, its end glyph after it.
fn timeline(app: AppState, timeline: &Timeline) -> AnyElement {
    let form = app.form;
    let bars = timeline.rows.iter().map(|row| {
        let action = row.action.clone();
        let start = row.start.clamp(0.0, 1.0);
        let width = (row.end - row.start).clamp(0.0, 1.0).max(0.01);
        let label = cell(form, row.child.clone(), Token::Ink, true)
            .flex_shrink_0()
            .w(if form == Form::Terminal {
                term::cells(6.0)
            } else {
                px(48.0)
            });
        let track = view()
            .flex_1()
            .min_w(px(0.0))
            .h(if form == Form::Terminal {
                term::rows(1.0)
            } else {
                px(8.0)
            })
            .relative()
            .child(
                view()
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .left(relative(start))
                    .w(relative(width))
                    .rounded(px(if form == Form::Terminal { 0.0 } else { 3.0 }))
                    .bg(hue_ink(row.hue)),
            );
        view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label(format!("{} on the timeline", row.child))
            .on_press(move || run(app, action.clone()))
            .child(label)
            .child(track)
            .children(
                row.glyph
                    .map(|glyph| text(glyph).text_color(hue_ink(row.hue))),
            )
            .into_element()
    });
    view()
        .test_id("session-timeline")
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(h3(
            app,
            timeline.heading.clone(),
            Some(format!("0s – {}", timeline.axis_end)),
        ))
        .children(bars.collect::<Vec<_>>())
        .into_element()
}

/// The messages between the agents, oldest first, each head (`parent → review · 14:02`) over
/// its words and how it was delivered when it did not simply arrive.
fn messages(app: AppState, list: &MessageList) -> AnyElement {
    let form = app.form;
    view()
        .test_id("session-agent-messages")
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(h3(app, list.heading, list.note.clone()))
        .children(
            list.entries
                .iter()
                .map(|entry| {
                    view()
                        .test_id(&entry.test_id)
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(mark(app, entry.mark, entry.hue))
                                .child(rich_small(app, &entry.head)),
                        )
                        .children(entry.body.iter().map(|words| {
                            view()
                                .pl(if form == Form::Terminal {
                                    term::cells(2.0)
                                } else {
                                    px(18.0)
                                })
                                .child(rich(app, words))
                                .into_element()
                        }))
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}
