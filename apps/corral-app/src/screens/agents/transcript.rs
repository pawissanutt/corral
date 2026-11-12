//! A session's transcript (`.tx`, `.tm`, `.tool`, `.qcard`) and its reply box (`.composer`,
//! the phone's `.reply`). Desktop and phone share the layout at their sizes; a terminal shows
//! each message as a block of plain rows.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Message, MessageItem, QuestionCard, Reply, Tool, Transcript};
use crate::model::hue::Hue;
use crate::model::text::Rich;
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::{dot, hue_fill, hue_ink, mark};
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

/// The transcript: every message in order, then the illustrative disclaimer. Desktop centres it
/// at the mockup's measure; the phone uses the full width with 16 pt sides.
pub fn transcript(app: AppState, transcript: &Transcript) -> AnyElement {
    let form = app.form;
    let messages = transcript
        .messages
        .iter()
        .enumerate()
        .map(|(at, message)| self::message(app, at, message));
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
                .children(parts)
                .child(term::blank())
                .child(illus(app, transcript.disclaimer))
                .into_element()
        }
        Form::Desktop | Form::Phone => {
            let sides = if form == Form::Phone { 16.0 } else { 24.0 };
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
                        .children(messages.collect::<Vec<_>>())
                        .child(
                            view()
                                .px(px(sides))
                                .pt(px(4.0))
                                .pb(px(16.0))
                                .child(illus(app, transcript.disclaimer)),
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

/// What a screen reader hears for a tool call.
fn tool_label(tool: &Tool) -> String {
    format!("{} {}, {}", tool.kind, tool.argument, tool.result)
}

/// A tool call (`.tool`): its result mark, the tool, its argument cut to fit and its result;
/// output the model shows expanded is in the terminal look under it. Pressing the row runs the
/// model's tool action.
fn tool(app: AppState, tool: &Tool, test_id: String) -> AnyElement {
    let form = app.form;
    let action = tool.action;
    let head = view()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(tool_label(tool))
        .test_id(&test_id)
        .on_press(move || run(app, action));
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
                            .child(term::quiet(tool.kind)),
                    )
                    .child(
                        text(tool.argument)
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_color(color(Token::Ink)),
                    )
                    .child(term::space(1.0))
                    .child(term::quiet(tool.result)),
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
    let mono = |content: &'static str, size: f32, ink: Token| {
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
                .child(
                    view()
                        .w(px(44.0))
                        .flex_shrink_0()
                        .child(mono(tool.kind, 11.0, Token::Ink3)),
                )
                .child(
                    mono(tool.argument, 11.5, Token::Ink)
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate(),
                )
                .child(mono(tool.result, 11.0, Token::Ink3).flex_shrink_0()),
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

/// The inline handover (`.qcard`): owner only and blocking, in the needs-you colour; the
/// question; the button that opens the question and the options hint.
fn question(app: AppState, card: &QuestionCard) -> AnyElement {
    let form = app.form;
    let action = card.action;
    let open = button(
        app,
        card.button,
        ButtonKind::Primary,
        "btn-open-question",
        move || run(app, action),
    );
    if form == Form::Terminal {
        return term::framed(
            view()
                .px(term::cells(1.0))
                .child(
                    text(card.label)
                        .number_of_lines(1u32)
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::You)),
                )
                .child(plain(app, card.question))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .child(open)
                        .child(term::space(2.0))
                        .child(term::quiet(card.hint)),
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
        .bg(color(Token::YouBg))
        .border(px(1.5))
        .border_color(color(Token::You))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(dot(app, Hue::You))
                .child(
                    text(card.label)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 12.5 } else { 11.5 }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::You)),
                ),
        )
        .child(
            view()
                .mt(px(4.0))
                .mb(px(8.0))
                .child(plain(app, card.question)),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .child(open)
                .child(dim(app, card.hint)),
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

/// The reply box under the transcript: the field and Send in one raised box, then how the
/// message is sent and where the open question is answered. Desktop: `.composer` (a rule
/// above, 10 24 12, the box at most 772 pt wide). Terminal: a rule, the field and Send, the
/// hint quiet. The phone docks [`phone_reply`] instead.
pub fn reply(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let form = app.form;
    let action = reply.action;
    let send = button_with(
        app,
        reply.button,
        ButtonKind::Primary,
        ButtonOptions {
            icon: Some(Icon::Send),
            ..ButtonOptions::default()
        },
        "btn-send",
        move || run(app, action),
    );
    if form == Form::Terminal {
        return view()
            .flex_none()
            .child(term::hrule())
            .child(
                view()
                    .flex_row()
                    .items_end()
                    .px(term::cells(1.0))
                    .child(field(app, reply, draft))
                    .child(term::space(2.0))
                    .child(send),
            )
            .child(
                view()
                    .px(term::cells(1.0))
                    .child(line(app, &reply.hint, 0.0, Token::Ink3)),
            )
            .into_element();
    }
    view()
        .flex_none()
        .px(px(24.0))
        .pt(px(10.0))
        .pb(px(12.0))
        .bg(color(Token::Bg))
        .border_t_1()
        .border_t_color(color(Token::Rule))
        .items_center()
        .child(
            view()
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
                .child(send),
        )
        .child(
            view()
                .w_full()
                .max_w(px(772.0))
                .mt(px(6.0))
                .flex_row()
                .flex_wrap()
                .gap(px(12.0))
                .child(line(app, &reply.hint, 11.0, Token::Ink4))
                .child(
                    text(reply.question_hint)
                        .font_family(FONT_TEXT)
                        .text_size(px(11.0))
                        .text_color(color(Token::Ink4)),
                ),
        )
        .into_element()
}

/// The phone's docked reply (`.reply`): the field and the Send button in one rounded box, for
/// the page's bar.
pub fn phone_reply(app: AppState, reply: &Reply, draft: Signal<String>) -> AnyElement {
    let action = reply.action;
    view()
        .flex_1()
        .min_w(px(0.0))
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
        .child(button_with(
            app,
            reply.button,
            ButtonKind::Primary,
            ButtonOptions {
                icon: Some(Icon::Send),
                ..ButtonOptions::default()
            },
            "btn-send",
            move || run(app, action),
        ))
        .into_element()
}
