//! The Agents tab (`session::AgentsTab`). The list: the session's child agents in the host's
//! order, one line each (state mark and word, id in bold mono, title, profile and model, how long
//! it ran, cost, context share, turns, files, then what it does now); the background jobs as
//! their own group; under a rule the selected agent's or job's detail with its controls, the
//! timeline of when each agent ran and the messages between them. An opened agent's
//! conversation (`conversation`) or a job's output (`job`) takes the list's place. A cancel or
//! stop asks once in a card above what shows; an agent's model choices open there too. What the
//! host did not report reads `–`. While the host is unreachable or stopped the rows are its last
//! fleet, said so above them. Desktop and phone set the lines in points (the phone in two
//! lines), a terminal in Cells. test ids `session-agents`, `agent-<session>-child-<id>`,
//! `agent-<session>-msg-<seq>`, `job-<id>`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{AgentRowView, AgentsTab, Body, MessageList, Opened, SessionPage};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::marks::mark;
use crate::ui::kit::notice::{empty, notice};
use crate::ui::kit::text::{h3, rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::hue_words;

#[path = "agents_detail.rs"]
mod agents_detail;
#[path = "conversation.rs"]
mod conversation;
#[path = "job.rs"]
mod job;

/// Which view the tab shows: drawn again only when that changes.
#[derive(Clone, PartialEq, Eq)]
enum Shown {
    None,
    List,
    Conversation(String),
    Job(String),
}

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let tab = computed(move || {
        page.with(|p| match &p.body {
            Body::Agents(tab) => Some(tab.clone()),
            _ => None,
        })
    });
    let shown = computed(move || {
        tab.with(|tab| match tab.as_ref().map(|tab| &tab.opened) {
            None => Shown::None,
            Some(None) => Shown::List,
            Some(Some(Opened::Conversation(conversation))) => {
                Shown::Conversation(conversation.child.clone())
            }
            Some(Some(Opened::Job(output))) => Shown::Job(output.job.clone()),
        })
    });
    dynamic(move || {
        let shown = shown.get();
        untracked(|| match shown {
            Shown::None => None,
            Shown::List => Some(list(app, tab)),
            Shown::Conversation(_) => Some(conversation::page(app, tab)),
            Shown::Job(_) => Some(job::output(app, tab)),
        })
    })
    .into_element()
}

/// The list's scrolling page, drawn again as the tab changes.
fn list(app: AppState, tab: Memo<Option<AgentsTab>>) -> AnyElement {
    let content =
        dynamic(move || tab.with(|tab| tab.as_ref().map(|tab| untracked(|| face(app, tab)))));
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

/// A rule between the list and what is selected, the form's way.
fn rule(app: AppState) -> AnyElement {
    match app.form {
        Form::Terminal => term::hrule(),
        _ => view().h(px(1.0)).bg(color(Token::Rule)).into_element(),
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
    let rows = (!tab.rows.is_empty()).then(|| {
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
    let selected = tab
        .detail
        .as_ref()
        .map(|detail| agents_detail::detail(app, detail))
        .or_else(|| tab.job.as_ref().map(|detail| job::detail(app, detail)));
    let below = selected.map(|selected| {
        view()
            .gap(px(gap))
            .child(rule(app))
            .child(selected)
            .into_element()
    });
    view()
        .gap(px(gap))
        .children(unbound)
        .children(agents_detail::asked(app, tab))
        .children(empty_state)
        .children(rows)
        .children(tab.jobs.as_ref().map(|jobs| job::list(app, jobs)))
        .children(below)
        .children(
            tab.timeline
                .as_ref()
                .map(|timeline| agents_detail::timeline(app, timeline)),
        )
        .children(
            tab.messages
                .as_ref()
                .map(|messages| self::messages(app, messages)),
        )
        .into_element()
}

/// Quiet words of a cell; mono for ids, times and money.
pub(super) fn cell(form: Form, words: String, ink: Token, mono: bool) -> Text {
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

/// A list line's pressable frame: the selection bar in a terminal, the selected fill and hover
/// on desktop, a hairline under each phone row.
pub(super) fn pressable(
    app: AppState,
    test_id: &str,
    label: String,
    selected: bool,
    action: crate::model::session::Action,
    body: View,
) -> AnyElement {
    let form = app.form;
    let frame = view()
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .label(label)
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
            .child(frame.flex_1().min_w(px(0.0)))
            .into_element(),
        Form::Desktop => frame
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .into_element(),
        Form::Phone => frame
            .py(px(8.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .into_element(),
    }
}

/// One agent's line, which selects it; on the selected line, opens its conversation. Desktop
/// keeps it to one line: the title gives way first, then the facts and what it does now clip.
fn line(app: AppState, row: &AgentRowView) -> AnyElement {
    let form = app.form;
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mark(app, row.mark, row.hue))
        .child(
            cell(form, row.name.clone(), Token::Ink, true)
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
        .child(
            cell(form, row.title.clone(), Token::Ink, false)
                .flex_1()
                .min_w(px(if form == Form::Terminal { 0.0 } else { 48.0 })),
        );
    let facts = [
        row.profile_model.clone(),
        row.time.clone(),
        row.cost.clone(),
        format!("ctx {}", row.ctx),
        row.turns.clone(),
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
    let now = row.now.clone().map(|said| {
        text(said)
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .text_color(if row.asks {
                hue_words(form, Hue::You)
            } else {
                color(Token::Ink2)
            })
            .when(form != Form::Terminal, |t| {
                t.font_family(FONT_TEXT)
                    .text_size(px(if form == Form::Phone { 13.0 } else { 12.5 }))
            })
    });
    let body = match form {
        Form::Phone => view().gap(px(2.0)).child(head).children(now).child(facts),
        Form::Desktop => view().child(
            head.child(facts.flex_shrink(2.0))
                .children(now.map(|now| now.flex_shrink(1.0).max_w(px(320.0)))),
        ),
        Form::Terminal => view()
            .child(head.child(facts.flex_shrink(2.0)))
            .children(now.map(|now| view().pl(term::cells(2.0)).child(now))),
    };
    let label = if row.selected {
        format!(
            "{}, {}, {}; press to open its conversation",
            row.name, row.state, row.title
        )
    } else {
        format!("{}, {}, {}", row.name, row.state, row.title)
    };
    pressable(
        app,
        &row.test_id,
        label,
        row.selected,
        row.action.clone(),
        body,
    )
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
