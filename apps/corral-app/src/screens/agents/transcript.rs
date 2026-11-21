//! A session's transcript (`.tx`, `.tm`, `.tool`, `.qcard`), replayed as the terminal UI shows
//! it: the agent's messages with their words as rendered Markdown, thinking, tool rows (folded
//! runs, Code Mode cells, build cards), images and cards under its avatar; the owner's words
//! and the other bubbles on the raised block; what the host made as quiet rows with no avatar;
//! this device's own sends the transcript does not hold yet (echoes, each with where it
//! stands); then the live turn while one runs. Desktop and phone share the layout at their
//! sizes; a terminal shows each message as a block of rows. A daemon transcript holds its
//! newest page and the earlier pages read since, with the control for the page before them and
//! the state of its source. `jump::follow` keeps the transcript's scroll at its newest rows,
//! offers the way back to them and reads the page before as the reader scrolls up to it.
//!
//! Every item carries the test id `item-<n>`, numbered across the page (a folded run's or a
//! cell's rows `item-<n>-<k>`), and a row that opens carries `btn-expand-<n>`; an echo carries
//! `echo-<command id>`.

mod anchors;
pub(crate) mod attachments;
mod bubbles;
mod build_cards;
mod code_cell;
mod group;
mod image;
mod jump;
#[cfg(test)]
mod jump_tests;
mod older;
#[cfg(test)]
mod older_tests;
mod parts;
mod prose;
mod rows;
mod tool;
mod watch;

use nsh_core::prelude::*;

pub(super) use build_cards::gate_log;
pub use jump::follow;
pub use older::Earlier;
pub use watch::watch;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::Action;
use crate::model::session::transcript::{
    Fill, Line, LineClass, Message, MessageItem, MessageKind, Note, QuestionCard, Transcript,
};
use crate::model::text::{Rich, Span, span};
use crate::screens::markdown::provide_styles;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::{dot, hue_fill, hue_ink};
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::kit::text::{dim, plain, rich};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, MEASURE_CONVERSATION, Token, color};

use super::run;

/// `.tm p`: paragraphs stop near 70 characters.
const PARAGRAPH: f32 = 600.0;

/// A run of rich text at `size`, in `base` ink: bold runs in ink and semibold (`.hd b`), dim
/// runs in ink-3, mono runs in the mono font, hued runs in their state colour. A terminal sets
/// its own size and font.
pub fn line(app: AppState, rich: &Rich, size: f32, base: Token) -> AnyElement {
    let form = app.form;
    let root = text("")
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(base));
    let root = if form == Form::Terminal {
        root
    } else {
        root.font_family(FONT_TEXT)
            .text_size(px(size))
            .line_height(px((size * 1.45).round()))
    };
    root.children(rich.iter().map(|span| {
        let mut run = text(span.text.clone());
        if span.bold {
            run = run
                .font_weight(if form == Form::Terminal {
                    FontWeight::BOLD
                } else {
                    FontWeight::SEMIBOLD
                })
                .text_color(color(Token::Ink));
        }
        if span.mono && form != Form::Terminal {
            run = run.font_family(FONT_MONO).text_size(px(size * 0.93));
        }
        if span.hue != Hue::Neutral {
            run = run.text_color(hue_ink(span.hue));
        } else if span.dim {
            run = run.text_color(color(Token::Ink3));
        }
        run
    }))
    .into_element()
}

/// The control for earlier messages above them: a quiet button, dimmed while a page loads. It
/// is read on its own as the reader scrolls up; the button stays for a transcript too short to
/// scroll and to try a failed page again.
fn pager(
    app: AppState,
    action: &Option<Action>,
    label: &str,
    test_id: &str,
    paging: bool,
) -> Option<AnyElement> {
    let action = action.clone()?;
    Some(button_with(
        app,
        if paging { "Loading…" } else { label },
        ButtonKind::Quiet,
        ButtonOptions {
            disabled: paging,
            ..ButtonOptions::default()
        },
        test_id,
        move || run(app, action.clone()),
    ))
}

/// What stands in for the messages while there are none to show: the loading state (announced
/// and captioned once the wait is noticeable), the failure or the empty state.
fn fill(app: AppState, fill: &Fill) -> Option<AnyElement> {
    match fill {
        Fill::Ready => None,
        Fill::Loading(caption) => Some(
            view()
                .when(app.form != Form::Terminal, |pad| {
                    pad.px(px(24.0)).py(px(12.0))
                })
                .child(loading(
                    app,
                    *caption,
                    &[(0.5, 10.0), (0.8, 10.0), (0.65, 10.0), (0.3, 10.0)],
                ))
                .into_element(),
        ),
        Fill::Failed(words) => Some(error(app, &vec![span(words.clone())], None)),
        Fill::Empty { heading, paragraph } => Some(empty(app, *heading, paragraph.clone(), None)),
    }
}

/// The note under the messages: a quiet sentence.
fn disclaimer(app: AppState, note: &Option<Note>) -> Option<AnyElement> {
    let Note::Plain(words) = note.as_ref()?;
    Some(dim(app, words.clone()).into_element())
}

/// Rows lined up under the messages' bodies: two Cells in, or the avatar's column and its gap.
fn body_column(app: AppState, rows: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view().pl(term::cells(2.0)).children(rows).into_element();
    }
    let sides = if form == Form::Phone { 16.0 } else { 24.0 };
    view()
        .pl(px(sides + 34.0))
        .pr(px(sides))
        .py(px(4.0))
        .children(rows)
        .into_element()
}

/// A message's block, placed among the transcript's rows by its anchor so the reading
/// position can hold on to it.
fn anchored(transcript: &Transcript, anchor: &str, block: AnyElement) -> AnyElement {
    let token = anchors::token();
    let scope = transcript.scope.to_string();
    let key = anchor.to_owned();
    {
        let (scope, key) = (scope.clone(), key.clone());
        cleanup(move || anchors::forget(&scope, &key, token));
    }
    view()
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| {
            anchors::placed(&scope, &key, token, frame.origin.y.0);
        })
        .child(block)
        .into_element()
}

/// The transcript: a stale notice when its source is unreachable, the control for earlier
/// messages, every message of the page in order (or why there are none), this device's sends
/// the page does not hold yet, the live turn and the quiet rows after it, then the note. Desktop centres it at the mockup's measure; the phone uses
/// the full width with 16 pt sides.
pub fn transcript(app: AppState, transcript: &Transcript) -> AnyElement {
    let form = app.form;
    provide_styles(app);
    let mut next = 0;
    let mut messages = Vec::new();
    for message in &transcript.messages {
        let block = self::message(app, message, next);
        messages.push(anchored(transcript, &message.anchor, block));
        next += message.items.len() + usize::from(message.footer.is_some());
    }
    messages.extend(
        transcript
            .echoes
            .iter()
            .map(|sent| bubbles::echo(app, sent)),
    );
    for message in &transcript.live {
        let block = self::message(app, message, next);
        messages.push(anchored(transcript, &message.anchor, block));
        next += message.items.len() + usize::from(message.footer.is_some());
    }
    let after = (!transcript.after.is_empty()).then(|| {
        body_column(
            app,
            transcript
                .after
                .iter()
                .enumerate()
                .map(|(n, entry)| item(app, entry, next + n)),
        )
    });
    let stale = transcript
        .stale
        .as_ref()
        .map(|words| notice(app, Hue::Held, &vec![span(words.clone())], None));
    let older = pager(
        app,
        &transcript.older,
        "Show earlier messages",
        "btn-older",
        transcript.paging,
    );
    let fill = self::fill(app, &transcript.fill);
    let note = disclaimer(app, &transcript.disclaimer);
    match form {
        Form::Terminal => {
            let mut parts = Vec::new();
            for (at, block) in messages.into_iter().enumerate() {
                if at > 0 {
                    parts.push(term::blank());
                }
                parts.push(block);
            }
            view()
                .test_id("session-transcript-items")
                .children(stale)
                .children(older.map(|button| view().flex_row().child(button)))
                .children(fill)
                .children(parts)
                .children(after.map(|rows| view().child(term::blank()).child(rows)))
                .when_some(note, |list, note| list.child(term::blank()).child(note))
                .into_element()
        }
        Form::Desktop | Form::Phone => {
            let sides = if form == Form::Phone { 16.0 } else { 24.0 };
            let edge = move |content: AnyElement| {
                view().flex_row().px(px(sides)).py(px(4.0)).child(content)
            };
            view()
                .w_full()
                .items_center()
                .child(
                    view()
                        .test_id("session-transcript-items")
                        .w_full()
                        .max_w(px(MEASURE_CONVERSATION))
                        .pt(px(if form == Form::Phone { 8.0 } else { 16.0 }))
                        .pb(px(10.0))
                        .children(stale.map(|notice| edge(notice).pb(px(8.0))))
                        .children(older.map(edge))
                        .children(fill)
                        .children(messages)
                        .children(after)
                        .children(
                            note.map(|note| {
                                view().px(px(sides)).pt(px(4.0)).pb(px(16.0)).child(note)
                            }),
                        ),
                )
                .into_element()
        }
    }
}

/// One item of a message, numbered `n` across the page.
fn item(app: AppState, entry: &MessageItem, n: usize) -> AnyElement {
    let id = n.to_string();
    let body = match entry {
        MessageItem::Paragraph(words) => paragraph(app, words),
        MessageItem::Prose(words) => prose::prose(app, words, &id),
        MessageItem::Thinking(thought) => prose::thinking(app, thought, &id),
        MessageItem::Tool(call) => tool::tool(app, call, &id),
        MessageItem::Group(folded) => group::group(app, folded, &id),
        MessageItem::Cell(cell) => code_cell::cell(app, cell, &id),
        MessageItem::Build(card) => build_cards::card(app, card, &id),
        MessageItem::Answered(answer) => bubbles::answered(app, answer, &id),
        MessageItem::Picture(picture) => image::picture(app, picture, &id),
        MessageItem::Divider(divider) => rows::divider(app, divider, &id),
        MessageItem::Marker(marker) => rows::marker(app, marker, &id),
        MessageItem::Card(card) => rows::card(app, card, &id),
        MessageItem::Question(card) => question(app, card),
    };
    view()
        .test_id(&format!("item-{n}"))
        .min_w(px(0.0))
        .child(body)
        .into_element()
}

/// One message, drawn as its kind says: the host's quiet rows with no avatar or header; a
/// bubble with its lead; an agent's message (`.tm`) with its avatar in its hue, the header line,
/// then its items in recorded order and the turn's footer. A live message carries the streaming
/// mark in its header.
fn message(app: AppState, message: &Message, first: usize) -> AnyElement {
    let form = app.form;
    let mut items = message
        .items
        .iter()
        .enumerate()
        .map(|(n, entry)| item(app, entry, first + n))
        .collect::<Vec<_>>();
    if let Some(footer) = &message.footer {
        items.push(rows::footer(
            app,
            footer,
            &(first + message.items.len()).to_string(),
        ));
    }
    match message.kind {
        MessageKind::Quiet => return body_column(app, items),
        MessageKind::Bubble => return bubbles::bubble(app, message, items),
        MessageKind::Agent => {}
    }
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(line(app, &message.head, 11.5, Token::Ink3))
        .when(message.live, |head| head.child(prose::caret(app)));
    if form == Form::Terminal {
        return view()
            .child(
                view()
                    .flex_row()
                    .child(
                        text(message.avatar)
                            .flex_shrink_0()
                            .font_weight(FontWeight::BOLD)
                            .text_color(hue_ink(message.hue)),
                    )
                    .child(term::space(1.0))
                    .child(head),
            )
            .child(view().pl(term::cells(2.0)).children(items))
            .into_element();
    }
    let sides = if form == Form::Phone { 16.0 } else { 24.0 };
    view()
        .flex_row()
        .items_start()
        .gap(px(10.0))
        .px(px(sides))
        .py(px(8.0))
        .child(
            view()
                .w(px(24.0))
                .h(px(24.0))
                .flex_shrink_0()
                .rounded(px(6.0))
                .items_center()
                .justify_center()
                .bg(hue_fill(message.hue))
                .accessibility_elements_hidden(true)
                .child(
                    text(message.avatar)
                        .font_family(FONT_TEXT)
                        .text_size(px(10.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(hue_ink(message.hue)),
                ),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(view().mb(px(3.0)).child(head))
                .children(items),
        )
        .into_element()
}

/// Output or log lines in a real terminal: one row each in its class's role, a span in its hue.
pub(super) fn cell_lines(lines: &[Line]) -> AnyElement {
    view()
        .min_w(px(0.0))
        .children(lines.iter().map(|line| {
            if line.text.iter().all(|span| span.text.is_empty()) {
                return term::blank();
            }
            let (ink, weight) = class_look(line.class);
            text("")
                .min_w(px(0.0))
                .number_of_lines(1u32)
                .font_weight(weight)
                .text_color(ink)
                .children(line.text.iter().map(|span| run_of(span, ink)))
                .into_element()
        }))
        .into_element()
}

/// Lines of a log as the form sets them: mono rows on the page (wrapping on the phone), each
/// in its class's role; one row each in a terminal.
pub(super) fn log_lines(app: AppState, lines: &[Line]) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return cell_lines(lines);
    }
    let (size, leading) = if form == Form::Phone {
        (11.5, 17.0)
    } else {
        (12.0, 18.0)
    };
    view()
        .min_w(px(0.0))
        .children(lines.iter().map(|line| {
            let (ink, weight) = class_look(line.class);
            text("")
                .min_w(px(0.0))
                .min_h(px(leading))
                .font_family(FONT_MONO)
                .text_size(px(size))
                .line_height(px(leading))
                .font_weight(weight)
                .text_color(ink)
                .children(line.text.iter().map(|span| run_of(span, ink)))
                .into_element()
        }))
        .into_element()
}

/// A line class's colour and weight.
fn class_look(class: LineClass) -> (AdaptiveColor, FontWeight) {
    match class {
        LineClass::Prompt => (color(Token::Worker), FontWeight::NORMAL),
        LineClass::Ok => (color(Token::Ok), FontWeight::NORMAL),
        LineClass::Dim => (color(Token::Ink3), FontWeight::NORMAL),
        LineClass::Bold => (color(Token::Ink), FontWeight::BOLD),
        LineClass::Plain => (color(Token::Ink), FontWeight::NORMAL),
    }
}

/// One run of a line: its own hue over the class, dim or bold when the span says so.
fn run_of(span: &Span, base: AdaptiveColor) -> Text {
    let ink = if span.hue != Hue::Neutral {
        hue_ink(span.hue)
    } else if span.dim {
        color(Token::Ink3)
    } else {
        base
    };
    text(span.text.clone())
        .text_color(ink)
        .when(span.bold, |t| t.font_weight(FontWeight::BOLD))
}

/// A paragraph of a message (`.tm p`): body text, 6 pt under it, at most ~70 characters wide.
fn paragraph(app: AppState, words: &Rich) -> AnyElement {
    if app.form == Form::Terminal {
        return rich(app, words);
    }
    view()
        .max_w(px(PARAGRAPH))
        .mb(px(6.0))
        .child(rich(app, words))
        .into_element()
}

/// An asked question in the transcript (`.qcard`): an open one in the needs-you colour, a
/// settled one quiet. It is answered through its task's Blocked card or a reply, not here.
fn question(app: AppState, card: &QuestionCard) -> AnyElement {
    let form = app.form;
    let (label_ink, hue) = if card.open {
        (Token::You, Hue::You)
    } else {
        (Token::Ink2, Hue::Neutral)
    };
    if form == Form::Terminal {
        return term::framed(
            view()
                .px(term::cells(1.0))
                .child(
                    text(card.label.clone())
                        .number_of_lines(1u32)
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(label_ink)),
                )
                .child(plain(app, card.question.clone()))
                .child(term::quiet(card.hint.clone())),
        )
        .test_id("session-question")
        .into_element();
    }
    let phone = form == Form::Phone;
    view()
        .test_id("session-question")
        .mt(px(6.0))
        .mb(px(6.0))
        .px(px(14.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .map(|card_box| {
            if card.open {
                card_box
                    .bg(color(Token::YouBg))
                    .border(px(1.5))
                    .border_color(color(Token::You))
            } else {
                card_box
                    .bg(color(Token::Surface))
                    .border_1()
                    .border_color(color(Token::Rule2))
            }
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(dot(app, hue))
                .child(
                    text(card.label.clone())
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 12.5 } else { 11.5 }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(label_ink)),
                ),
        )
        .child(
            view()
                .mt(px(4.0))
                .mb(px(8.0))
                .child(plain(app, card.question.clone())),
        )
        .child(dim(app, card.hint.clone()))
        .into_element()
}
