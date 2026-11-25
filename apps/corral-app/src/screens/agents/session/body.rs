//! The session page's main view (`session::Body`): the transcript, or a part's full view opened
//! in its place with the bar over it that leads back to the conversation. On a wide desktop the
//! transcript and what sits under it keep to the conversation's measure and a full view to a
//! record's, each centred, while their rules and bands run edge to edge.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::{self, Body, EmptyChat, SessionPage};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::page::{measure, measure_fill};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, MEASURE_CONVERSATION, MEASURE_RECORD, Token, color};

use super::super::run;
use super::super::transcript;
use super::{agents_tab, board, changes, files, pager, plan_review, proof, task, usage};

/// The rows a terminal's conversation or view keeps however much waits in the dock.
const MAIN_ROWS: f32 = 6.0;

/// `content` kept to the transcript's measure where `measured` (the page on desktop), so the
/// dock, the composer and the alerts stay under the conversation on a wide window; Ask and
/// the cards fill their own narrower places as they are. The measured part keeps its height, as
/// the parts of the page's foot do.
pub(super) fn conversation(app: AppState, measured: bool, content: AnyElement) -> AnyElement {
    if measured && app.form == Form::Desktop {
        view()
            .flex_none()
            .child(measure(app, MEASURE_CONVERSATION, content))
            .into_element()
    } else {
        content
    }
}

/// Over a part's full view (Changes, Files, Agents, the board, Usage): what it is, and the way
/// back to the conversation, which Escape also takes. A terminal's rows are few, so it has none;
/// the ⋯ menu's keys say Escape goes back. On desktop its words line up with the view under it.
pub(super) fn view_bar(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view().into_element();
    }
    let shown = computed(move || {
        page.with(|p| {
            // The plan's way back puts its review aside; the card stays pending.
            if let Body::PlanReview(doc) = &p.body {
                return Some(("Plan review".to_owned(), doc.back.clone()));
            }
            let on_view = p.tab != session::Tab::Chat || p.sub.is_some();
            on_view.then(|| {
                let label = p
                    .tabs
                    .iter()
                    .find(|item| item.selected)
                    .map_or_else(|| p.tab.label().to_owned(), |item| item.label.clone());
                (
                    label,
                    session::Action::Tab {
                        session: p.id.clone(),
                        tab: session::Tab::Chat,
                    },
                )
            })
        })
    });
    dynamic(move || {
        shown.get().map(|(label, back)| {
            untracked(|| {
                let line = view()
                    .test_id("session-view-bar")
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .child(button(
                        app,
                        "‹ Conversation",
                        ButtonKind::Quiet,
                        "btn-view-back",
                        move || run(app, back.clone()),
                    ))
                    .child(
                        text(label)
                            .number_of_lines(1u32)
                            .truncate()
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                    );
                match form {
                    Form::Terminal => line.px(term::cells(1.0)).into_element(),
                    Form::Desktop => view()
                        .flex_none()
                        .border_b(px(1.0))
                        .border_color(color(Token::Rule))
                        .child(measure(
                            app,
                            MEASURE_RECORD,
                            line.h(px(36.0)).px(px(14.0)).into_element(),
                        ))
                        .into_element(),
                    Form::Phone => line
                        .h(px(36.0))
                        .px(px(14.0))
                        .border_b(px(1.0))
                        .border_color(color(Token::Rule))
                        .into_element(),
                }
            })
        })
    })
}

/// Which main view shows: drawn again only when that changes, each following its own part.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Chat,
    Usage,
    Agents,
    Changes,
    Files,
    Board,
    PlanReview,
    Task,
    Proof,
    Log,
}

/// The main view. A full view keeps to a record's measure on desktop: the widest of them, the
/// Changes list beside its diff, still has its 900 pt for both within it.
pub(super) fn main(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Chat(_) => Shown::Chat,
            Body::Usage(_) => Shown::Usage,
            Body::Agents(_) => Shown::Agents,
            Body::Changes(_) => Shown::Changes,
            Body::Files(_) => Shown::Files,
            Body::Board(_) => Shown::Board,
            Body::PlanReview(_) => Shown::PlanReview,
            Body::Task(_) => Shown::Task,
            Body::Proof(_) => Shown::Proof,
            Body::Log(_) => Shown::Log,
        })
    });
    // A small terminal keeps rows of the conversation or view; the dock scrolls instead.
    let least = if app.form == Form::Terminal {
        term::rows(MAIN_ROWS)
    } else {
        px(0.0)
    };
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(least)
        .flex_col()
        .child(dynamic(move || {
            let shown = shown.get();
            untracked(|| {
                let full = match shown {
                    Shown::Chat => return chat(app, page),
                    Shown::Usage => usage::tab(app, page),
                    Shown::Agents => agents_tab::tab(app, page),
                    Shown::Changes => changes::tab(app, page),
                    // Give the explorer and its editor the whole available workspace.
                    Shown::Files => return files::tab(app, page),
                    Shown::Board => board::tab(app, page),
                    // The plan keeps to the prose measure itself.
                    Shown::PlanReview => return plan_review::document(app, page),
                    Shown::Task => task::tab(app, page),
                    Shown::Proof => proof::tab(app, page),
                    Shown::Log => pager::pager(app, page),
                };
                measure_fill(app, MEASURE_RECORD, full)
            })
        }))
        .into_element()
}

/// Chat: the transcript in its own scroll, which keeps to the newest rows as they arrive unless
/// the owner scrolled up (then a control counts what arrived and returns to the end); before
/// the first row, who receives the message.
pub(super) fn chat(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let said = computed(move || {
        page.with(|p| match &p.body {
            Body::Chat(chat) => Some(chat.transcript.clone()),
            _ => None,
        })
    });
    let empty = computed(move || {
        page.with(|p| match &p.body {
            Body::Chat(chat) => Some(chat.empty.clone()),
            _ => None,
        })
    });
    let shown = dynamic(move || {
        let empty = empty.get()?;
        Some(untracked(|| match empty {
            Some(empty) => chat_empty(app, &empty),
            None => transcript::transcript(app, said),
        }))
    });
    let content = if app.form == Form::Terminal {
        view().px(term::cells(1.0)).child(shown).into_element()
    } else {
        shown
    };
    transcript::follow(
        app,
        scroll()
            .test_id("session-transcript")
            .flex_1()
            .min_h(px(0.0)),
        content,
        move || said.with(|t| t.as_ref().map(|t| t.following())),
        move || said.with(|t| t.as_ref().map(transcript::Earlier::of)),
        move |key| said.with(|t| t.as_ref()?.rows_below(key)),
        move || page.with(|p| p.jump),
    )
}

fn chat_empty(app: AppState, empty: &EmptyChat) -> AnyElement {
    let form = app.form;
    let sentence = text(empty.sentence.clone()).text_color(color(Token::Ink2));
    let keys = view()
        .flex_row()
        .flex_wrap()
        .gap(px(if form == Form::Terminal { 16.0 } else { 12.0 }))
        .children(
            empty
                .keys
                .iter()
                .map(|&(keys, name)| term_or_hint(app, keys, name)),
        );
    let block = view().test_id("session-empty").gap(px(8.0));
    match form {
        Form::Terminal => block
            .child(term::blank())
            .child(sentence)
            .child(keys)
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .w_full()
            .items_center()
            .pt(px(32.0))
            .px(px(24.0))
            .child(
                block
                    .max_w(px(560.0))
                    .child(
                        sentence
                            .font_family(FONT_TEXT)
                            .text_size(px(14.0))
                            .line_height(px(21.0)),
                    )
                    .child(keys),
            )
            .into_element(),
    }
}

/// A key and what it does: the terminal's legend, the desktop's cap.
fn term_or_hint(app: AppState, keys: &'static str, name: &'static str) -> AnyElement {
    if app.form == Form::Terminal {
        return term::legend(keys, name);
    }
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(crate::ui::atoms::key_hint(keys))
        .child(
            text(name)
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}
