//! A session's transcript (`.tx`, `.tm`, `.tool`, `.qcard`) and its reply box (`.composer`,
//! the phone's `.reply`). Desktop and phone share the layout at their sizes; a terminal shows
//! each message as a block of plain rows. A daemon transcript is one bounded page, with the
//! controls to page through it and the state of its source.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{
    Action, Fill, Message, MessageItem, Note, QuestionCard, Reply, ReplyMode, Tool, Transcript,
};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::{dot, hue_fill, hue_ink, mark};
use crate::ui::kit::notice::{empty, error, notice, skeleton};
use crate::ui::kit::text::{dim, plain, rich};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::console::{self, TOOL};
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

/// What stands in for the messages while there are none to show: the skeleton, the failure or
/// the empty state.
fn fill(app: AppState, fill: &Fill) -> Option<AnyElement> {
    match fill {
        Fill::Ready => None,
        Fill::Loading => Some(
            view()
                .when(app.form != Form::Terminal, |pad| {
                    pad.px(px(24.0)).py(px(12.0))
                })
                .child(skeleton(
                    app,
                    &[(0.5, 10.0), (0.8, 10.0), (0.65, 10.0), (0.3, 10.0)],
                ))
                .into_element(),
        ),
        Fill::Failed(words) => Some(error(app, &vec![span(words.clone())], None)),
        Fill::Empty { heading, paragraph } => Some(empty(app, *heading, paragraph.clone(), None)),
    }
}

/// The note under the messages: the fixture's illustrative tag, or a quiet sentence.
fn disclaimer(app: AppState, note: &Option<Note>) -> Option<AnyElement> {
    Some(match note.as_ref()? {
        Note::Illustrative(words) => illus(app, *words),
        Note::Plain(words) => dim(app, words.clone()).into_element(),
    })
}

/// The transcript: a stale notice when its source is unreachable, the control for earlier
/// messages, every message of the page in order (or why there are none), the way back to the
/// latest messages, then the note. Desktop centres it at the mockup's measure; the phone uses
/// the full width with 16 pt sides.
pub fn transcript(app: AppState, transcript: &Transcript) -> AnyElement {
    let form = app.form;
    let messages = transcript
        .messages
        .iter()
        .enumerate()
        .map(|(at, message)| self::message(app, at, message));
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
            for (at, block) in messages.enumerate() {
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
                        .children(messages.collect::<Vec<_>>())
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

/// One message (`.tm`): the avatar in its hue, the header line, then its paragraphs, tool calls
/// and question card.
fn message(app: AppState, at: usize, message: &Message) -> AnyElement {
    let form = app.form;
    let items = message
        .items
        .iter()
        .enumerate()
        .map(|(n, item)| match item {
            MessageItem::Paragraph(words) => paragraph(app, words),
            MessageItem::Tool(tool) => self::tool(app, tool, format!("btn-tool-{at}-{n}")),
            MessageItem::Question(card) => question(app, card),
        })
        .collect::<Vec<_>>();
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
                    .child(line(app, &message.head, 0.0, Token::Ink3)),
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
                .child(
                    view()
                        .mb(px(3.0))
                        .child(line(app, &message.head, 11.5, Token::Ink3)),
                )
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

/// What a screen reader hears for a tool call, with whether its output is shown.
fn tool_label(tool: &Tool) -> String {
    let output = match tool.expanded {
        Some(true) => ", output shown",
        Some(false) => ", output hidden",
        None => "",
    };
    format!("{} {}, {}{output}", tool.kind, tool.argument, tool.result)
}

/// A tool call (`.tool`): its result mark, the tool, its argument cut to fit and its result;
/// output the model shows expanded is in the terminal look under it. Pressing the row runs the
/// model's tool action.
fn tool(app: AppState, tool: &Tool, test_id: String) -> AnyElement {
    let form = app.form;
    let action = tool.action.clone();
    let head = view()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(tool_label(tool))
        .test_id(&test_id)
        .on_press(move || run(app, action.clone()));
    if form == Form::Terminal {
        return view()
            .child(
                head.h(term::rows(1.0))
                    .child(mark(app, tool.mark, Hue::Neutral))
                    .child(term::space(1.0))
                    .child(
                        view()
                            .w(term::cells(5.0))
                            .flex_shrink_0()
                            .child(term::quiet(tool.kind.clone())),
                    )
                    .child(
                        text(tool.argument.clone())
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_color(color(Token::Ink)),
                    )
                    .child(term::space(1.0))
                    .child(term::quiet(tool.result.clone())),
            )
            .when(!tool.output.is_empty(), |block| {
                block.child(
                    view()
                        .pl(term::cells(2.0))
                        .child(console::cell_lines(app, &tool.output)),
                )
            })
            .into_element();
    }
    let mono = |content: String, size: f32, ink: Token| {
        text(content)
            .number_of_lines(1u32)
            .font_family(FONT_MONO)
            .text_size(px(size))
            .text_color(color(ink))
    };
    view()
        .mt(px(4.0))
        .mb(px(4.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .overflow_hidden()
        .child(
            head.gap(px(8.0))
                .px(px(10.0))
                .py(px(5.0))
                .min_h(px(if form == Form::Phone { 36.0 } else { 26.0 }))
                // The phone row reaches the 44 pt target into the 4 pt margins around it.
                .when(form == Form::Phone, |row| {
                    row.hit_slop(EdgeInsets {
                        top: px(4.0),
                        right: px(0.0),
                        bottom: px(4.0),
                        left: px(0.0),
                    })
                })
                .hover(|s| s.bg(color(Token::Hover)))
                .active(|s| s.bg(color(Token::Sel)))
                .focus_visible(|s| {
                    s.outline(px(2.0))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(-2.0))
                })
                .child(
                    view()
                        .w(px(14.0))
                        .flex_shrink_0()
                        .child(mark(app, tool.mark, Hue::Neutral)),
                )
                .child(view().w(px(44.0)).flex_shrink_0().child(mono(
                    tool.kind.clone(),
                    11.0,
                    Token::Ink3,
                )))
                .child(
                    mono(tool.argument.clone(), 11.5, Token::Ink)
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate(),
                )
                .child(mono(tool.result.clone(), 11.0, Token::Ink3).flex_shrink_0()),
        )
        .when(!tool.output.is_empty(), |block| {
            block.child(
                view()
                    .px(px(12.0))
                    .py(px(10.0))
                    .bg(console::ground())
                    .child(console::lines(&tool.output, TOOL, false)),
            )
        })
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

/// The reply field: one draft per open session page, kept while the view switches.
fn field(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let form = app.form;
    let size = match form {
        Form::Phone => 15.0,
        Form::Desktop | Form::Terminal => text_size(form),
    };
    let entry = input()
        .test_id("session-reply")
        .label(reply.accessibility_label)
        .multiline(true)
        .placeholder(reply.placeholder.clone())
        .value(move || draft.get())
        .on_change(move |text: &str| draft.set(text.to_owned()))
        .flex_1()
        .min_w(px(0.0))
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    match form {
        Form::Desktop => entry.h(px(38.0)).line_height(px(19.0)).into_element(),
        Form::Phone => entry
            .h(px(40.0))
            .py(px(9.0))
            .line_height(px(21.0))
            .into_element(),
        Form::Terminal => entry.h(term::rows(2.0)).into_element(),
    }
}

/// Send: the field's words go to the model's reducer; while a reply is in flight it waits,
/// dimmed, so one intent never becomes two commands.
fn send(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let busy = reply.mode == ReplyMode::Busy;
    button_with(
        app,
        if busy { "Sending…" } else { reply.button },
        ButtonKind::Primary,
        ButtonOptions {
            icon: Some(Icon::Send),
            disabled: busy,
            ..ButtonOptions::default()
        },
        "btn-send",
        move || run(app, Action::Send(draft.get_untracked())),
    )
}

/// The latest reply's outcome in its tone, with "Check delivery" when it is unknown.
fn outcome(app: AppState, reply: &Reply) -> Option<AnyElement> {
    let (hue, words) = reply.status.clone()?;
    let ink = match hue {
        Hue::Neutral => color(Token::Ink3),
        hue => hue_ink(hue),
    };
    let words = text(words)
        .test_id("session-reply-status")
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(ink);
    let words = if app.form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(11.5))
    };
    Some(
        view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(words)
            .children(reply.recover.clone().map(|action| {
                button(
                    app,
                    "Check delivery",
                    ButtonKind::Quiet,
                    "btn-recover-reply",
                    move || run(app, action.clone()),
                )
            }))
            .into_element(),
    )
}

/// Why replies do not go to this session, with where to answer instead.
fn closed(app: AppState, reason: &str, action: &Option<(&'static str, Action)>) -> AnyElement {
    let go = action.clone().map(|(label, action)| {
        button(
            app,
            label,
            ButtonKind::Ghost,
            "btn-reply-elsewhere",
            move || run(app, action.clone()),
        )
    });
    if app.form == Form::Terminal {
        return view()
            .flex_row()
            .items_center()
            .px(term::cells(1.0))
            .child(term::quiet(reason.to_owned()).flex_1().min_w(px(0.0)))
            .children(go.map(|go| view().flex_row().child(term::space(2.0)).child(go)))
            .into_element();
    }
    view()
        .test_id("session-reply-closed")
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(if app.form == Form::Phone { 8.0 } else { 9.0 }))
        .rounded(px(if app.form == Form::Phone { 12.0 } else { 8.0 }))
        .bg(color(Token::BgSunk))
        .border_1()
        .border_color(color(Token::Rule))
        .child(
            text(reason.to_owned())
                .flex_1()
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(if app.form == Form::Phone { 14.0 } else { 12.5 }))
                .text_color(color(Token::Ink2)),
        )
        .children(go)
        .into_element()
}

/// The reply box under the transcript: the field and Send in one raised box, then how the
/// message is sent, the latest reply's outcome and where the open question is answered.
/// Desktop: `.composer` (a rule above, 10 24 12, the box at most 772 pt wide). Terminal: a
/// rule, the field and Send, the hint quiet. A session that takes no replies says why instead.
/// The phone docks [`phone_reply`] instead.
pub fn reply(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let form = app.form;
    let status = outcome(app, reply);
    if form == Form::Terminal {
        let entry = match &reply.mode {
            ReplyMode::Closed { reason, action } => closed(app, reason, action),
            ReplyMode::Open | ReplyMode::Busy => view()
                .flex_row()
                .items_end()
                .px(term::cells(1.0))
                .child(field(app, reply, draft))
                .child(term::space(2.0))
                .child(send(app, reply, draft))
                .into_element(),
        };
        let takes_words = !matches!(reply.mode, ReplyMode::Closed { .. });
        return view()
            .flex_none()
            .child(term::hrule())
            .child(entry)
            .children(status.map(|status| view().px(term::cells(1.0)).child(status)))
            .when(takes_words, |box_| {
                box_.child(view().px(term::cells(1.0)).child(line(
                    app,
                    &reply.hint,
                    0.0,
                    Token::Ink3,
                )))
            })
            .into_element();
    }
    let entry = match &reply.mode {
        ReplyMode::Closed { reason, action } => view()
            .w_full()
            .max_w(px(772.0))
            .flex_row()
            .child(closed(app, reason, action))
            .into_element(),
        ReplyMode::Open | ReplyMode::Busy => view()
            .w_full()
            .max_w(px(772.0))
            .flex_row()
            .items_end()
            .gap(px(8.0))
            .pl(px(12.0))
            .pr(px(8.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2))
            .child(field(app, reply, draft))
            .child(send(app, reply, draft))
            .into_element(),
    };
    let takes_words = !matches!(reply.mode, ReplyMode::Closed { .. });
    view()
        .flex_none()
        .px(px(24.0))
        .pt(px(10.0))
        .pb(px(12.0))
        .bg(color(Token::Bg))
        .border_t_1()
        .border_t_color(color(Token::Rule))
        .items_center()
        .child(entry)
        .children(status.map(|status| view().w_full().max_w(px(772.0)).mt(px(6.0)).child(status)))
        .when(takes_words, |box_| {
            box_.child(
                view()
                    .w_full()
                    .max_w(px(772.0))
                    .mt(px(6.0))
                    .flex_row()
                    .flex_wrap()
                    .gap(px(12.0))
                    .child(line(app, &reply.hint, 11.0, Token::Ink4))
                    .when(!reply.question_hint.is_empty(), |hints| {
                        hints.child(
                            text(reply.question_hint.clone())
                                .font_family(FONT_TEXT)
                                .text_size(px(11.0))
                                .text_color(color(Token::Ink4)),
                        )
                    }),
            )
        })
        .into_element()
}

/// The phone's docked reply (`.reply`): the field and the Send button in one rounded box, for
/// the page's bar, with the latest reply's outcome above it; or why the session takes none.
pub fn phone_reply(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let entry = match &reply.mode {
        ReplyMode::Closed { reason, action } => closed(app, reason, action),
        ReplyMode::Open | ReplyMode::Busy => view()
            .flex_row()
            .items_end()
            .gap(px(8.0))
            .pl(px(12.0))
            .pr(px(6.0))
            .py(px(6.0))
            .rounded(px(12.0))
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2))
            .child(field(app, reply, draft))
            .child(send(app, reply, draft))
            .into_element(),
    };
    view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(6.0))
        .children(outcome(app, reply).map(|status| view().px(px(4.0)).child(status)))
        .child(entry)
        .into_element()
}
