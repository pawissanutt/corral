//! A session's transcript (`.tx`, `.tm`, `.tool`, `.qcard`), replayed as the terminal UI shows
//! it: every message with its words as rendered Markdown, thinking, tool rows, images,
//! dividers, quiet marker rows, cards and the turn's footer, then the live turn while one runs.
//! Desktop and phone share the layout at their sizes; a terminal shows each message as a block
//! of rows. A daemon transcript is one bounded page, with the controls to page through it and
//! the state of its source. The reply box under it is drawn by `super::reply`.
//!
//! Every item carries the test id `item-<n>`, numbered across the page, and a row that opens
//! carries `btn-expand-<n>`.

pub mod fleet;
mod parts;
mod prose;
mod rows;
mod tool;
mod watch;

use nsh_core::prelude::*;

pub use watch::watch;

use crate::model::Form;
use crate::model::agents::{Action, Fill, Message, MessageItem, Note, QuestionCard, Transcript};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};
use crate::screens::markdown::provide_styles;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::marks::{dot, hue_fill, hue_ink};
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::kit::text::{dim, plain, rich};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::run;

/// `.tx`: at most 820 pt wide, centred in its column.
const MEASURE: f32 = 820.0;
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

/// A paging control above or below the messages: a quiet button, dimmed while a page loads.
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

/// The transcript: a stale notice when its source is unreachable, the control for earlier
/// messages, every message of the page in order (or why there are none), the live turn and the
/// quiet rows after it, the way back to the latest messages, then the note. Desktop centres it
/// at the mockup's measure; the phone uses the full width with 16 pt sides.
pub fn transcript(app: AppState, transcript: &Transcript) -> AnyElement {
    let form = app.form;
    provide_styles(app);
    let mut next = 0;
    let mut messages = Vec::new();
    for message in transcript.messages.iter().chain(&transcript.live) {
        messages.push(self::message(app, message, next));
        next += message.items.len() + usize::from(message.footer.is_some());
    }
    let after = (!transcript.after.is_empty()).then(|| {
        let rows = transcript
            .after
            .iter()
            .enumerate()
            .map(|(n, entry)| item(app, entry, next + n));
        if form == Form::Terminal {
            view().pl(term::cells(2.0)).children(rows).into_element()
        } else {
            let sides = if form == Form::Phone { 16.0 } else { 24.0 };
            // Lined up under the messages' bodies: the avatar's column and its gap.
            view()
                .pl(px(sides + 34.0))
                .pr(px(sides))
                .py(px(4.0))
                .children(rows)
                .into_element()
        }
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
    let newer = pager(
        app,
        &transcript.newer,
        "Back to the latest messages",
        "btn-newer",
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
                .children(newer.map(|button| view().flex_row().child(button)))
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
                        .max_w(px(MEASURE))
                        .pt(px(if form == Form::Phone { 8.0 } else { 16.0 }))
                        .pb(px(10.0))
                        .children(stale.map(|notice| edge(notice).pb(px(8.0))))
                        .children(older.map(edge))
                        .children(fill)
                        .children(messages)
                        .children(after)
                        .children(newer.map(edge))
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
    let body = match entry {
        MessageItem::Paragraph(words) => paragraph(app, words),
        MessageItem::Prose(words) => prose::prose(app, words, n),
        MessageItem::Thinking(thought) => prose::thinking(app, thought, n),
        MessageItem::Tool(call) => tool::tool(app, call, n),
        MessageItem::Picture(picture) => rows::picture(app, picture, n),
        MessageItem::Divider(divider) => rows::divider(app, divider, n),
        MessageItem::Marker(marker) => rows::marker(app, marker, n),
        MessageItem::Card(card) => rows::card(app, card, n),
        MessageItem::Question(card) => question(app, card),
    };
    view()
        .test_id(&format!("item-{n}"))
        .min_w(px(0.0))
        .child(body)
        .into_element()
}

/// One message (`.tm`): the avatar in its hue, the header line, then its items in recorded
/// order and the turn's footer; a live message carries the streaming mark in its header.
fn message(app: AppState, message: &Message, first: usize) -> AnyElement {
    let form = app.form;
    let mut items = message
        .items
        .iter()
        .enumerate()
        .map(|(n, entry)| item(app, entry, first + n))
        .collect::<Vec<_>>();
    if let Some(footer) = &message.footer {
        items.push(rows::footer(app, footer, first + message.items.len()));
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

/// The inline handover (`.qcard`): an open question in the needs-you colour with the button
/// that opens it to decide; a settled one quiet, still opening its handover.
fn question(app: AppState, card: &QuestionCard) -> AnyElement {
    let form = app.form;
    let action = card.action.clone();
    let open = button(
        app,
        card.button,
        if card.open {
            ButtonKind::Primary
        } else {
            ButtonKind::Ghost
        },
        "btn-open-question",
        move || run(app, action.clone()),
    );
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
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .child(open)
                        .child(term::space(2.0))
                        .child(term::quiet(card.hint.clone())),
                ),
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
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .child(open)
                .child(dim(app, card.hint.clone())),
        )
        .into_element()
}
