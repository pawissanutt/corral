//! The transcript: every `ThreadItem` of a thread in order, an assistant reply with its typed
//! cards, the PM items with their folded outcome, the waiting and queued escape hatches. It is
//! its own scroll view and opens at the newest message.

use nsh_core::prelude::*;

use crate::model::assistant::{
    Action, AssistantMessage, Body, LABELS, Mode, OwnerMessage, PmMessage, QueueNote, RollDivider,
    ThreadItem, WaitingBlock, thread,
};
use crate::model::board::Mark;
use crate::model::{Form, Hue, Rich};
use crate::state::AppState;
use crate::ui::kit::button::ButtonKind;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::mark;
use crate::ui::kit::text::{plain, rich};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color, text_size};

use super::cards;
use super::parts::{
    act, avatar, button_row, link_words, meta_size, model_button, mono_run, note_size, spacer,
    strong_run, tag, words,
};

/// One blink of the streaming caret.
const BLINK_MS: f32 = 1000.0;
/// Paragraphs stop near 70 characters (`.tm p { max-width: 70ch }`).
const MEASURE: f32 = 640.0;
/// The avatar column and its gap, which the waiting and queued blocks line up under.
const AVATAR_COLUMN: f32 = 36.0;

/// The transcript of thread `id`: a scroll view filling its container, rows `inset` points in on
/// either side (desktop page 24, drawer 16; phone 14). It opens at its end and follows each new
/// message and each streamed step.
pub fn list(app: AppState, id: &'static str, inset: f32) -> AnyElement {
    let form = app.form;
    let scroller = element_ref();
    let tools_open = signal(false);
    super::drive_stream(app);
    follow(app, id, scroller);
    let content = dynamic(move || {
        let env = app.env();
        let items = app.assistant.with(|state| thread(state, id, env));
        let mut out = Vec::with_capacity(items.len() * 2);
        for item in &items {
            if form == Form::Terminal && !out.is_empty() {
                out.push(term::blank());
            }
            out.push(item_view(app, item, inset, tools_open));
        }
        out
    });
    let region = format!("thread-{id}-items");
    let inner = view().child(content);
    let inner = match form {
        Form::Desktop => inner.pt(px(10.0)).pb(px(14.0)),
        Form::Phone => inner.pt(px(6.0)).pb(px(10.0)),
        Form::Terminal => inner.child(term::blank()),
    };
    scroll()
        .element_ref(&scroller)
        .test_id(&region)
        .label("Transcript")
        .flex_1()
        .min_h(px(0.0))
        .child(inner)
        .into_element()
}

/// Scrolls to the end once mounted (after the commit that mounts it), then again, animated, when
/// the thread gains a message or the streamed answer grows.
fn follow(app: AppState, id: &'static str, scroller: ElementRef) {
    let seen = stored(None::<(usize, u32)>);
    effect(move || {
        let now = app.assistant.with(|state| {
            let sent = state.sent.iter().filter(|m| m.thread == id).count();
            let stream = if state.mode == Mode::Answering {
                state.stream.to_bits()
            } else {
                0
            };
            (sent, stream)
        });
        let before = seen.replace(Some(now));
        if before != Some(now) {
            let animated = before.is_some();
            after_answer(move || scroller.scroll_to_end(animated));
        }
    });
}

fn item_view(app: AppState, item: &ThreadItem, inset: f32, tools: Signal<bool>) -> AnyElement {
    match item {
        ThreadItem::Roll(roll) => roll_view(app, roll, inset),
        ThreadItem::Owner(owner) => owner_view(app, owner, inset),
        ThreadItem::Assistant(message) => assistant_view(app, message, inset, tools),
        ThreadItem::Pm(pm) => pm_view(app, pm, inset),
        ThreadItem::Waiting(block) => under_avatar(app, inset, waiting(app, block)),
        ThreadItem::Queued(note) => under_avatar(app, inset, queued(app, note)),
        ThreadItem::Saved(card) => under_avatar(app, inset, cards::saved(app, card)),
        ThreadItem::Note(note) => under_avatar(
            app,
            inset,
            words(app, *note, 12.5, Token::Ink3).into_element(),
        ),
    }
}

/// A message (`.tm`): the avatar, then the header over the body. Desktop and phone pad it 8 pt
/// above and below; a terminal leads it with the author's glyph two Cells wide.
fn message(
    app: AppState,
    inset: f32,
    face: AnyElement,
    head: AnyElement,
    body: Vec<AnyElement>,
) -> AnyElement {
    if app.form == Form::Terminal {
        return view()
            .flex_row()
            .px(term::cells(1.0))
            .child(view().w(term::cells(2.0)).flex_shrink_0().child(face))
            .child(view().flex_1().min_w(px(0.0)).child(head).children(body))
            .into_element();
    }
    view()
        .flex_row()
        .items_start()
        .gap(px(10.0))
        .px(px(inset))
        .py(px(8.0))
        .child(face)
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(4.0))
                .child(head)
                .children(body),
        )
        .into_element()
}

/// A block lined up under a message's body (`.waitbox` margin 0 16 8 52).
fn under_avatar(app: AppState, inset: f32, block: AnyElement) -> AnyElement {
    if app.form == Form::Terminal {
        return view()
            .pl(term::cells(3.0))
            .pr(term::cells(1.0))
            .child(block)
            .into_element();
    }
    view()
        .pl(px(inset + AVATAR_COLUMN))
        .pr(px(inset))
        .pb(px(8.0))
        .child(block)
        .into_element()
}

/// A header line (`.tm .hd`): quiet small words with bold and mono runs, then chips and tags.
fn head(app: AppState, runs: Vec<Text>, tail: Vec<AnyElement>) -> AnyElement {
    let line = words(app, "", meta_size(app.form), Token::Ink3)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .children(runs);
    let row = view().flex_row().flex_wrap().items_center();
    let row = if app.form == Form::Terminal {
        row
    } else {
        row.gap_x(px(6.0)).gap_y(px(2.0))
    };
    let mut parts = vec![line.into_element()];
    for piece in tail {
        if app.form == Form::Terminal {
            parts.push(term::space(1.0));
        }
        parts.push(piece);
    }
    row.children(parts).into_element()
}

const DOT: &str = " · ";

/// `you · app:pawis-desktop · 21:31 · typed 21:20 offline`, with the PM target and item id
/// between the author and the device when it is a PM item.
fn owner_runs(app: AppState, owner: &OwnerMessage, pm: Option<&'static str>) -> Vec<Text> {
    let size = meta_size(app.form);
    let mut runs = vec![strong_run(owner.author)];
    if let Some(id) = pm {
        runs.push(text(" → "));
        runs.push(strong_run(LABELS.pm));
        runs.push(text(DOT));
        runs.push(mono_run(app, id, size));
    }
    runs.push(text(DOT));
    runs.push(mono_run(app, owner.provenance.clone(), size));
    runs.push(text(format!("{DOT}{}", owner.at)));
    if let Some(extra) = owner.extra {
        runs.push(text(format!("{DOT}{extra}")));
    }
    runs
}

fn owner_tail(app: AppState, owner: &OwnerMessage, hue: Hue) -> Vec<AnyElement> {
    let mut tail = Vec::new();
    if let Some(attachment) = owner.attachment {
        tail.push(chip(app, attachment, Hue::Neutral));
    }
    tail.push(tag(app, owner.state.label(), hue));
    tail
}

fn owner_view(app: AppState, owner: &OwnerMessage, inset: f32) -> AnyElement {
    message(
        app,
        inset,
        avatar(app, LABELS.owner_avatar, Hue::You),
        head(
            app,
            owner_runs(app, owner, None),
            owner_tail(app, owner, owner.state.hue()),
        ),
        vec![plain(app, owner.text.clone()).into_element()],
    )
}

/// A PM item: the owner's words to the PM, then the PM's outcome folded under them on the corral
/// ground (`.pmreply`), or the note of a queued item.
fn pm_view(app: AppState, pm: &PmMessage, inset: f32) -> AnyElement {
    let form = app.form;
    let mut body = vec![plain(app, pm.owner.text.clone()).into_element()];
    if let Some(reply) = &pm.reply {
        let size = meta_size(form);
        let hd = words(app, "", size, Token::Ink3)
            .child(strong_run(LABELS.pm))
            .child(text(format!(
                "{DOT}{}{DOT}{} {}",
                reply.at, LABELS.outcome_of, pm.id
            )));
        let said_size = if form == Form::Phone { 15.0 } else { 13.0 };
        let said = words(app, reply.text, said_size, Token::Ink);
        let reply_box = view()
            .gap(px(2.0))
            .bg(color(Token::CorralBg))
            .child(hd)
            .child(said);
        let reply_box = if form == Form::Terminal {
            reply_box.px(term::cells(1.0))
        } else {
            reply_box
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .mt(px(2.0))
        };
        body.push(reply_box.into_element());
    }
    if let Some(note) = pm.note {
        body.push(words(app, note, note_size(form), Token::Ink3).into_element());
    }
    message(
        app,
        inset,
        avatar(app, LABELS.owner_avatar, Hue::You),
        head(
            app,
            owner_runs(app, &pm.owner, Some(pm.id)),
            owner_tail(app, &pm.owner, pm.owner.state.pm_hue()),
        ),
        body,
    )
}

/// An assistant reply: tool activity, the body and its cards in order, the caret while it
/// streams, what it rests on, then Stop, the cost, the stopped note and the disclosure.
fn assistant_view(
    app: AppState,
    message_data: &AssistantMessage,
    inset: f32,
    tools: Signal<bool>,
) -> AnyElement {
    let mut runs = vec![strong_run(format!(
        "{} {}",
        LABELS.assistant, message_data.id
    ))];
    runs.push(text(format!("{DOT}{}{DOT}", message_data.at)));
    runs.push(text(message_data.model).text_color(color(Token::Ink3)));
    let tail = message_data
        .state
        .iter()
        .map(|state| chip(app, state.label.clone(), state.hue))
        .collect();
    let mut body = Vec::new();
    if let Some(summary) = message_data.tool_summary {
        body.push(tool_summary(app, summary, message_data, tools));
    }
    for block in &message_data.body {
        body.push(body_view(app, block));
    }
    if message_data.streaming {
        body.push(caret(app));
    }
    if let Some(label) = message_data.sources_label() {
        body.push(sources(app, label, message_data));
    }
    if let Some(foot) = foot(app, message_data) {
        body.push(foot);
    }
    message(
        app,
        inset,
        avatar(app, LABELS.assistant_avatar, Hue::Worker),
        head(app, runs, tail),
        body,
    )
}

fn body_view(app: AppState, block: &Body) -> AnyElement {
    match block {
        Body::Paragraph(r) => paragraph(app, r),
        Body::Ordered(items) => ordered(app, items),
        Body::Action(card) => cards::action(app, card),
        Body::AskUser(card) => cards::ask_user(app, card),
        Body::Merged(card) => cards::merged(app, card),
        Body::Permission(card) => cards::permission(app, card),
        Body::Filed(card) => cards::filed(app, card),
        Body::Starting { state, text: said } => cards::starting(app, state, *said),
        Body::Illustrative(label) => illus(app, *label),
    }
}

/// A paragraph of the answer, its references linked, at most ~70 characters wide on desktop.
fn paragraph(app: AppState, r: &Rich) -> AnyElement {
    let holder = view().child(rich(app, r));
    if app.form == Form::Desktop {
        holder.max_w(px(MEASURE)).into_element()
    } else {
        holder.into_element()
    }
}

/// A numbered list (`.aol`): each number in its own column, the entry wrapping beside it.
fn ordered(app: AppState, items: &[Rich]) -> AnyElement {
    let form = app.form;
    let column = if form == Form::Terminal {
        term::cells(3.0)
    } else {
        px(20.0)
    };
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .when(form == Form::Desktop, |list| list.max_w(px(MEASURE)))
        .children(items.iter().enumerate().map(|(at, entry)| {
            view()
                .flex_row()
                .items_start()
                .child(view().w(column).flex_shrink_0().child(words(
                    app,
                    format!("{}.", at + 1),
                    text_size(form),
                    Token::Ink,
                )))
                .child(view().flex_1().min_w(px(0.0)).child(rich(app, entry)))
                .into_element()
        }))
        .into_element()
}

/// The streaming caret (`.caret`): a 7 by 14 pt block of ink, blinking; `▌` in a terminal.
fn caret(app: AppState) -> AnyElement {
    if app.form == Form::Terminal {
        return text("▌")
            .text_color(color(Token::Ink))
            .accessibility_elements_hidden(true)
            .into_element();
    }
    let blink = motion().entering(
        MotionSpec::new(KeyframeTiming::new(BLINK_MS).infinite())
            .values(MotionChannel::Opacity, [1.0, 0.0, 1.0]),
    );
    view()
        .w(px(7.0))
        .h(px(14.0))
        .bg(color(Token::Ink))
        .accessibility_elements_hidden(true)
        .bind_motion(&blink)
        .into_element()
}

/// The tool activity (`.toolsum`): one pressable line that opens the rows of tool, command and
/// time under it.
fn tool_summary(
    app: AppState,
    summary: &'static str,
    message_data: &AssistantMessage,
    open: Signal<bool>,
) -> AnyElement {
    let form = app.form;
    let size = note_size(form);
    let terminal = form == Form::Terminal;
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .role(AccessibilityRole::Button)
        .label(summary)
        .accessibility_state(move || AccessibilityState {
            expanded: Some(open.get()),
            ..AccessibilityState::default()
        })
        .test_id("btn-tool-activity")
        .cursor_pointer()
        .on_press(move || open.update(|o| *o = !*o))
        .child(mark(app, Mark::Done, Hue::Neutral))
        .when(terminal, |row| row.child(term::space(1.0)))
        .child(
            words(app, summary, size, Token::Ink2)
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(1u32),
        );
    let line = if terminal {
        line
    } else {
        line.px(px(10.0))
            .py(px(5.0))
            .hover(|s| s.bg(color(Token::Hover)))
    };
    let tools = message_data.tools;
    let rows = dynamic(move || {
        if !open.get() {
            return Vec::new();
        }
        tools
            .iter()
            .map(|tool| {
                let row = view()
                    .flex_row()
                    .items_center()
                    .child(
                        view()
                            .w(if terminal { term::cells(6.0) } else { px(40.0) })
                            .flex_shrink_0()
                            .child(tool_mono(app, tool.kind, 11.0, Token::Ink3)),
                    )
                    .child(
                        tool_mono(app, tool.command, 11.5, Token::Ink)
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate(),
                    )
                    .when(terminal, |row| row.child(term::space(1.0)))
                    .child(words(app, tool.elapsed, size, Token::Ink3).flex_shrink_0());
                if terminal {
                    row.pl(term::cells(2.0)).into_element()
                } else {
                    row.gap(px(8.0))
                        .px(px(10.0))
                        .py(px(3.0))
                        .border_t_1()
                        .border_t_color(color(Token::Rule))
                        .into_element()
                }
            })
            .collect::<Vec<_>>()
    });
    let block = view().child(line).child(rows);
    if terminal {
        block.into_element()
    } else {
        block
            .rounded(px(6.0))
            .border_1()
            .border_color(color(Token::Rule))
            .overflow_hidden()
            .into_element()
    }
}

fn tool_mono(app: AppState, content: &'static str, size: f32, ink: Token) -> Text {
    let t = text(content).number_of_lines(1u32).text_color(color(ink));
    if app.form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_MONO).text_size(px(size))
    }
}

/// `Rests on` and the records the answer cites, ` · ` apart (`.rests`): a record opens its page,
/// the search shows its outcome.
fn sources(app: AppState, label: &'static str, message_data: &AssistantMessage) -> AnyElement {
    let size = note_size(app.form);
    let mut parts = vec![words(app, label, size, Token::Ink3).into_element()];
    for (at, source) in message_data.sources.iter().enumerate() {
        if at > 0 {
            parts.push(words(app, "·", size, Token::Ink3).into_element());
        }
        let page = source.page;
        let toast = source.toast;
        parts.push(
            link_words(
                app,
                source.label,
                size,
                &format!("source-{at}"),
                move || {
                    if let Some(page) = page {
                        app.open(page);
                    } else if let Some(toast) = toast {
                        app.toast(toast);
                    }
                },
            )
            .into_element(),
        );
    }
    let row = view().flex_row().flex_wrap().items_center();
    let row = if app.form == Form::Terminal {
        row
    } else {
        row.gap_x(px(5.0)).gap_y(px(2.0))
    };
    row.children(spaced(app, parts)).into_element()
}

/// The reply's foot (`.afoot`): Stop while it streams, the cost, the stopped note, the
/// disclosure at the trailing edge.
fn foot(app: AppState, message_data: &AssistantMessage) -> Option<AnyElement> {
    let size = note_size(app.form);
    let mut parts = Vec::new();
    if let Some(stop) = &message_data.stop {
        parts.push(model_button(app, stop, ButtonKind::Ghost));
    }
    if let Some(cost) = message_data.cost {
        parts.push(words(app, cost, size, Token::Ink3).into_element());
    }
    if let Some(note) = message_data.stopped_note {
        parts.push(chip(app, note, Hue::Neutral));
    }
    if let Some(label) = message_data.illustrative {
        parts.push(spacer());
        parts.push(illus(app, label));
    }
    if parts.is_empty() {
        return None;
    }
    let row = view().flex_row().flex_wrap().items_center();
    let row = if app.form == Form::Terminal {
        row
    } else {
        row.gap(px(10.0)).mt(px(2.0))
    };
    Some(row.children(spaced(app, parts)).into_element())
}

/// Terminal items one Cell apart; elsewhere the row's gap spaces them.
fn spaced(app: AppState, parts: Vec<AnyElement>) -> Vec<AnyElement> {
    if app.form != Form::Terminal {
        return parts;
    }
    let mut out = Vec::with_capacity(parts.len() * 2);
    for (at, part) in parts.into_iter().enumerate() {
        if at > 0 {
            out.push(term::space(1.0));
        }
        out.push(part);
    }
    out
}

/// The roll divider (`.sumrow`): the summary of the generation before, and its transcript.
fn roll_view(app: AppState, roll: &RollDivider, inset: f32) -> AnyElement {
    let form = app.form;
    let size = note_size(form);
    let action = roll.transcript.action.clone();
    let transcript = link_words(
        app,
        roll.transcript.label,
        size,
        &super::parts::test_id(&roll.transcript),
        move || act(app, action.clone()),
    )
    .flex_shrink_0();
    let row = view()
        .flex_row()
        .items_center()
        .child(mark(app, Mark::Na, Hue::Neutral))
        .when(form == Form::Terminal, |row| row.child(term::space(1.0)))
        .child(
            words(app, roll.text, size, Token::Ink3)
                .flex_1()
                .min_w(px(0.0)),
        )
        .when(form == Form::Terminal, |row| row.child(term::space(1.0)))
        .child(transcript);
    if form == Form::Terminal {
        return row.px(term::cells(1.0)).into_element();
    }
    view()
        .px(px(inset))
        .pb(px(6.0))
        .child(
            row.gap(px(8.0))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(6.0))
                .bg(color(Token::BgSunk)),
        )
        .into_element()
}

/// The waiting block (`.waitbox`): the stale mark, the reason in bold, why and what happens to
/// the message, the escape hatches at the trailing edge; the lifetime line under it.
fn waiting(app: AppState, block: &WaitingBlock) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 15.0 } else { 13.0 };
    let mut runs = vec![strong_run(block.state.label.clone())];
    if let Some(explanation) = block.explanation {
        runs.push(text(format!("{DOT}{explanation}")));
    }
    if !block.note.is_empty() {
        runs.push(text(format!(" {}", block.note)));
    }
    let sentence = words(app, "", size, Token::Ink)
        .flex_1()
        .min_w(px(160.0))
        .children(runs);
    let buttons = view().flex_row().flex_wrap().items_center().children(
        block.buttons.iter().enumerate().flat_map(|(at, button)| {
            let kind = if matches!(button.action, Action::SetMode(_)) {
                ButtonKind::Ghost
            } else {
                ButtonKind::Quiet
            };
            let gap = (form == Form::Terminal && at > 0).then(|| term::space(2.0));
            gap.into_iter().chain([model_button(app, button, kind)])
        }),
    );
    let lifetime = words(app, block.lifetime, meta_size(form), Token::Ink3);
    let box_row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .bg(color(Token::HeldBg))
        .child(mark(app, Mark::Stale, Hue::Held))
        .when(form == Form::Terminal, |row| row.child(term::space(1.0)))
        .child(sentence)
        .child(buttons.when(form != Form::Terminal, |b| b.gap(px(6.0))));
    let box_row = if form == Form::Terminal {
        box_row
    } else {
        box_row
            .gap(px(10.0))
            .px(px(12.0))
            .py(px(10.0))
            .rounded(px(8.0))
    };
    view()
        .gap(px(4.0))
        .child(box_row)
        .child(lifetime)
        .into_element()
}

/// A locally queued message's note and its escape hatches, under the message.
fn queued(app: AppState, note: &QueueNote) -> AnyElement {
    let form = app.form;
    view()
        .gap(px(6.0))
        .child(words(app, note.text, note_size(form), Token::Ink3))
        .child(view().flex_row().justify_end().child(button_row(
            app,
            &note.buttons,
            ButtonKind::Quiet,
            6.0,
        )))
        .into_element()
}
