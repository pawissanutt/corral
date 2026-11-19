//! The Agents tab (`session::AgentsTab`). The list: the session's child agents in the host's
//! order, one line each (state mark and word, id in bold mono, title, profile and model, how long
//! it ran, cost, context share, turns, files, then what it does now); the background jobs as
//! their own group; under a rule the selected agent's or job's detail with its controls, the
//! timeline of when each agent ran and the messages between them. An opened agent's
//! conversation (`conversation`) or a job's output (`job`) takes the list's place. A cancel or
//! stop asks once in a card above what shows; an agent's model choices open there too. What the
//! host did not report reads `–`. While the host is unreachable or stopped the rows are its last
//! fleet, said so above them. Desktop and phone set the lines in points (the phone in two
//! lines), a terminal in Cells.
//!
//! Nothing pressable is built again because the clock moved or an agent's words changed: every
//! part is mounted from the tab without its clock (`AgentsTab::steady`) through a memo of its
//! own, the agents and jobs are kept one view per id, and what moves (running times, the
//! timeline's bars, an agent's activity, cost and files) is words and bars updating in place.
//! test ids `session-agents`, `agent-<session>-child-<id>`, `agent-<session>-msg-<seq>`,
//! `job-<id>`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{
    Action, AgentRowView, AgentsTab, Body, MessageList, Opened, SessionPage,
};
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

/// The tab as the page builds it: `full` with what the clock moves, `steady` without it (equal
/// across a tick).
#[derive(Clone, Copy)]
pub(super) struct Tab {
    pub(super) full: Memo<Option<AgentsTab>>,
    pub(super) steady: Memo<Option<AgentsTab>>,
}

impl Tab {
    /// Agent `child`'s running time, as the clock has it now (tracked).
    pub(super) fn time(self, child: &str) -> String {
        self.full
            .with(|tab| tab.as_ref().map(|tab| tab.clock.time(child)))
            .unwrap_or_else(|| "–".into())
    }

    /// Job `job`'s elapsed time, as the clock has it now (tracked).
    pub(super) fn elapsed(self, job: &str) -> Option<String> {
        self.full.with(|tab| {
            tab.as_ref()
                .and_then(|tab| tab.clock.job(job).map(str::to_owned))
        })
    }

    /// A part of the steady tab, as its own memo: what is mounted from it is built again only
    /// when that part changes.
    pub(super) fn part<T: Clone + PartialEq + 'static>(
        self,
        pick: impl Fn(&AgentsTab) -> T + 'static,
    ) -> Memo<Option<T>> {
        let steady = self.steady;
        computed(move || steady.with(|tab| tab.as_ref().map(&pick)))
    }
}

/// Which view the tab shows: drawn again only when that changes.
#[derive(Clone, PartialEq, Eq)]
enum Shown {
    None,
    List,
    Conversation(String),
    Job(String),
}

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let full = computed(move || {
        page.with(|p| match &p.body {
            Body::Agents(tab) => Some(tab.clone()),
            _ => None,
        })
    });
    let steady = computed(move || full.with(|tab| tab.as_ref().map(AgentsTab::steady)));
    let tab = Tab { full, steady };
    let shown = computed(move || {
        steady.with(|tab| match tab.as_ref().map(|tab| &tab.opened) {
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

/// The list's scrolling page: each part mounted from its own memo.
fn list(app: AppState, tab: Tab) -> AnyElement {
    let form = app.form;
    let gap = if form == Form::Terminal { 0.0 } else { 12.0 };
    let unbound = tab.part(|t| t.unbound.clone());
    let unbound = dynamic(move || {
        unbound
            .get()
            .flatten()
            .map(|words| untracked(|| notice(app, Hue::Held, &vec![span(words)], None)))
    });
    let empty_words = tab.part(|t| t.empty.clone());
    let empty_state = dynamic(move || {
        empty_words
            .get()
            .flatten()
            .map(|words| untracked(|| empty(app, "No agents", words, None)))
    });
    let any = tab.part(|t| !t.rows.is_empty());
    let rows = each(
        move || {
            tab.steady
                .with(|t| t.as_ref().map_or_else(Vec::new, |t| t.rows.clone()))
        },
        |row: &AgentRowView| row.child.clone(),
        move |row, _| line(app, tab, row),
    );
    let rows = view()
        .role(AccessibilityRole::List)
        .label("Agents")
        .style_when(move || any.get() != Some(true), |s| s.hidden())
        .child(rows);
    let content = view()
        .gap(px(gap))
        .child(unbound)
        .child(agents_detail::asking(app, tab))
        .child(empty_state)
        .child(rows)
        .child(job::list(app, tab))
        .child(selected(app, tab))
        .child(agents_detail::timeline(app, tab))
        .child(messages(app, tab));
    let surface = scroll().test_id("session-agents").flex_1().min_h(px(0.0));
    match form {
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
pub(super) fn rule(app: AppState) -> AnyElement {
    match app.form {
        Form::Terminal => term::hrule(),
        _ => view().h(px(1.0)).bg(color(Token::Rule)).into_element(),
    }
}

/// Under a rule, the selected agent's detail or the selected job's, whichever is selected.
fn selected(app: AppState, tab: Tab) -> AnyElement {
    #[derive(Clone, PartialEq)]
    enum Which {
        None,
        Agent(String),
        Job(String),
    }
    let which = tab.part(|t| match (&t.detail, &t.job) {
        (Some(detail), _) => Which::Agent(detail.child.clone()),
        (None, Some(job)) => Which::Job(job.job.clone()),
        (None, None) => Which::None,
    });
    let gap = if app.form == Form::Terminal {
        0.0
    } else {
        12.0
    };
    dynamic(move || {
        let which = which.get().unwrap_or(Which::None);
        untracked(|| {
            let shown = match which {
                Which::None => return None,
                Which::Agent(_) => agents_detail::detail(app, tab),
                Which::Job(_) => job::detail(app, tab),
            };
            Some(
                view()
                    .gap(px(gap))
                    .child(rule(app))
                    .child(shown)
                    .into_element(),
            )
        })
    })
    .into_element()
}

/// Quiet words of a cell, following `words`; mono for ids, times and money.
pub(super) fn cell(
    form: Form,
    words: impl IntoReactive<SharedString>,
    ink: Token,
    mono: bool,
) -> Text {
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

/// A list line's pressable frame, mounted once per id: the selection bar in a terminal, the
/// selected fill and hover on desktop, a hairline under each phone row. Its name, selection and
/// what a press does follow the row.
pub(super) fn pressable(
    app: AppState,
    test_id: &str,
    label: impl Fn() -> String + 'static,
    selected: Memo<bool>,
    action: impl Fn() -> Action + 'static,
    body: View,
) -> AnyElement {
    let form = app.form;
    let frame = view()
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(move || AccessibilityState {
            selected: selected.get(),
            ..AccessibilityState::default()
        })
        .cursor_pointer()
        .on_press(move || run(app, action()))
        .style_when(selected, |s| s.bg(color(Token::Sel)))
        .child(body);
    match form {
        Form::Terminal => view()
            .flex_row()
            .child(
                text(move || if selected.get() { "▌" } else { " " }).text_color(color(Token::Ink)),
            )
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

/// `profile · model · 1m 02s · $0.21 · ctx 18% · 4 turns · files 3` of a row, its time the
/// clock's.
fn row_facts(row: &AgentRowView, time: String) -> String {
    [
        row.profile_model.clone(),
        time,
        row.cost.clone(),
        format!("ctx {}", row.ctx),
        row.turns.clone(),
        format!("files {}", row.files),
    ]
    .join(" · ")
}

/// One agent's line, mounted once per agent id, which selects it; on the selected line, opens
/// its conversation. Its mark, words and facts follow the row in place. Desktop keeps it to one
/// line: the title gives way first, then the facts and what it does now clip.
fn line(app: AppState, tab: Tab, row: ReadSignal<AgentRowView>) -> AnyElement {
    let form = app.form;
    let (test_id, child, name) =
        untracked(|| row.with(|r| (r.test_id.clone(), r.child.clone(), r.name.clone())));
    let look = computed(move || row.with(|r| (r.mark, r.hue)));
    let mark_of = dynamic(move || {
        let (glyph, hue) = look.get();
        untracked(|| mark(app, glyph, hue))
    });
    let state = text(move || row.with(|r| r.state.to_owned()))
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(move || hue_words(form, look.get().1))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT).text_size(px(12.0))
        });
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mark_of)
        .child(
            cell(form, name.clone(), Token::Ink, true)
                .flex_shrink_0()
                .font_weight(FontWeight::BOLD),
        )
        .child(state)
        .child(
            cell(
                form,
                move || row.with(|r| r.title.clone()),
                Token::Ink,
                false,
            )
            .flex_1()
            .min_w(px(if form == Form::Terminal { 0.0 } else { 48.0 })),
        );
    let timed = child.clone();
    let facts = cell(
        form,
        move || {
            let time = tab.time(&timed);
            row.with(|r| row_facts(r, time))
        },
        Token::Ink3,
        true,
    )
    .text_color(move || {
        color(if row.with(|r| r.outside) {
            Token::Held
        } else {
            Token::Ink3
        })
    });
    let has_now = computed(move || row.with(|r| r.now.is_some()));
    let now = dynamic(move || {
        has_now.get().then(|| {
            untracked(|| {
                let words = text(move || row.with(|r| r.now.clone().unwrap_or_default()))
                    .number_of_lines(1u32)
                    .truncate()
                    .min_w(px(0.0))
                    .text_color(move || {
                        if row.with(|r| r.asks) {
                            hue_words(form, Hue::You)
                        } else {
                            color(Token::Ink2)
                        }
                    })
                    .when(form != Form::Terminal, |t| {
                        t.font_family(FONT_TEXT)
                            .text_size(px(if form == Form::Phone { 13.0 } else { 12.5 }))
                    });
                match form {
                    Form::Desktop => words.flex_shrink(1.0).max_w(px(320.0)).into_element(),
                    Form::Terminal => view().pl(term::cells(2.0)).child(words).into_element(),
                    Form::Phone => words.into_element(),
                }
            })
        })
    });
    let body = match form {
        Form::Phone => view().gap(px(2.0)).child(head).child(now).child(facts),
        Form::Desktop => view().child(head.child(facts.flex_shrink(2.0)).child(now)),
        Form::Terminal => view().child(head.child(facts.flex_shrink(2.0))).child(now),
    };
    let selected = computed(move || row.with(|r| r.selected));
    pressable(
        app,
        &test_id,
        move || {
            row.with(|r| {
                if r.selected {
                    format!(
                        "{}, {}, {}; press to open its conversation",
                        r.name, r.state, r.title
                    )
                } else {
                    format!("{}, {}, {}", r.name, r.state, r.title)
                }
            })
        },
        selected,
        move || row.with_untracked(|r| r.action.clone()),
        body,
    )
}

/// The messages between the agents, oldest first, each head (`parent → review · 14:02`) over
/// its words and how it was delivered when it did not simply arrive. Nothing here presses.
fn messages(app: AppState, tab: Tab) -> AnyElement {
    let list = tab.part(|t| t.messages.clone());
    dynamic(move || {
        list.get()
            .flatten()
            .map(|list| untracked(|| message_list(app, &list)))
    })
    .into_element()
}

fn message_list(app: AppState, list: &MessageList) -> AnyElement {
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
