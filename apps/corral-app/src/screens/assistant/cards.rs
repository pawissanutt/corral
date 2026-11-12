//! The typed cards an assistant reply carries: the owner action and its receipt, the ask_user
//! choice, the merge, the write permission, the filed Idea (and its edit state), the Idea saved
//! as written, and the host starting.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{
    ActionCard, ActionOutcome, AskUserCard, Chip, DiffLine, FILED_LABELS, FiledCard, LABELS,
    MergedCard, PermissionCard, SavedCard,
};
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::button::ButtonKind;
use crate::ui::kit::chip::{chip, illus, stamp_small};
use crate::ui::kit::marks::{dot, hue_fill, mark};
use crate::ui::kit::text::{label_value, rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::parts::{
    Edge, button_row, frame, meta_size, mono_line, mono_run, note_size, quote, spacer, strong_run,
    words,
};

/// The owner-action card (`.actcard`): heading, class chip, why and the cited input id over the
/// exact command, the confirmation when one was given, then the receipt.
pub fn action(app: AppState, card: &ActionCard) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let size = meta_size(form);
    let mut head = vec![
        words(app, card.heading, size, Token::Ink3).into_element(),
        chip(app, card.class.label(), card.class.hue()),
    ];
    if let Some(why) = card.why {
        head.push(words(app, why, size, Token::Ink3).into_element());
    }
    head.push(spacer());
    head.push(
        words(app, "", size, Token::Ink3)
            .child(mono_run(
                app,
                format!("{} {}", LABELS.cites, card.input),
                size,
            ))
            .into_element(),
    );
    let head = row(app, head, 8.0);
    let command = mono_line(
        app,
        card.command,
        if form == Form::Phone { 15.0 } else { 13.0 },
    );
    let mut parts = Vec::new();
    if terminal {
        parts.push(head.into_element());
        parts.push(command.into_element());
    } else {
        parts.push(view().px(px(12.0)).pt(px(7.0)).child(head).into_element());
        parts.push(
            view()
                .px(px(12.0))
                .pt(px(2.0))
                .pb(px(4.0))
                .child(command)
                .into_element(),
        );
    }
    if let Some(confirmed) = card.confirmed {
        let line = words(
            app,
            format!("{} {confirmed}", LABELS.confirmed_on),
            note_size(form),
            Token::Ink3,
        );
        parts.push(if terminal {
            line.into_element()
        } else {
            view().px(px(12.0)).pb(px(6.0)).child(line).into_element()
        });
    }
    if terminal {
        parts.push(term::hrule());
    }
    parts.push(receipt(app, &card.outcome));
    frame(app, Edge::Plain, false, parts)
}

/// The receipt row (`.rcpt`): the stamp and its text; refused on the failure ground, waiting on
/// the needs-you ground with Confirm and Decline at the trailing edge; a restriction has no stamp.
fn receipt(app: AppState, outcome: &ActionOutcome) -> AnyElement {
    let form = app.form;
    let size = note_size(form);
    let fill = match outcome {
        ActionOutcome::Refused(_) => Some(Hue::Fail),
        ActionOutcome::Waiting { .. } => Some(Hue::You),
        _ => None,
    };
    let mut parts = Vec::new();
    if let Some(stamp) = outcome.stamp() {
        parts.push(stamp_small(app, stamp, outcome.hue()));
    }
    match outcome {
        ActionOutcome::Executed(said)
        | ActionOutcome::Refused(said)
        | ActionOutcome::Declined(said) => {
            parts.push(
                mono_line(app, said.clone(), size)
                    .flex_1()
                    .min_w(px(0.0))
                    .into_element(),
            );
        }
        ActionOutcome::Waiting {
            explanation,
            buttons,
        } => {
            parts.push(
                view()
                    .flex_1()
                    .min_w(px(180.0))
                    .child(rich_small(app, explanation))
                    .into_element(),
            );
            parts.push(button_row(app, buttons, ButtonKind::Ghost, 6.0).into_element());
        }
        ActionOutcome::AskAndFileOnly(said) => {
            parts.push(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(rich_small(app, said))
                    .into_element(),
            );
        }
        ActionOutcome::Late { explanation, .. } => {
            parts.push(
                words(app, explanation.clone(), size, Token::Ink2)
                    .flex_1()
                    .min_w(px(0.0))
                    .into_element(),
            );
        }
    }
    let line = row(app, parts, 10.0).into_element();
    let holder = view()
        .when_some(fill, |v, hue| v.bg(hue_fill(hue)))
        .child(line);
    if form == Form::Terminal {
        return holder.into_element();
    }
    holder
        .px(px(12.0))
        .py(px(8.0))
        .border_t_1()
        .border_t_color(color(Token::Rule))
        .into_element()
}

/// The ask_user card (`.askuser`): who asks and when, the linked question, the two options with
/// their consequences (the recorded one outlined in ink), and how it was answered.
pub fn ask_user(app: AppState, card: &AskUserCard) -> AnyElement {
    let form = app.form;
    let options = card.options.iter().enumerate().map(|(at, option)| {
        let selected = option.selected;
        let enabled = option.enabled;
        let label = text(option.label)
            .font_weight(FontWeight::BOLD)
            .text_color(color(if selected { Token::Ink } else { Token::Ink2 }));
        let label = if form == Form::Terminal {
            label
        } else {
            label
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 15.0 } else { 13.0 }))
        };
        let consequence = words(app, option.consequence, note_size(form), Token::Ink3);
        let choice = view()
            .role(AccessibilityRole::Button)
            .label(format!("{}. {}", option.label, option.consequence))
            .accessibility_state(AccessibilityState {
                selected,
                disabled: !enabled,
                ..AccessibilityState::default()
            })
            .test_id(&format!("opt-{}", at + 1))
            .gap(px(3.0));
        if form == Form::Terminal {
            choice
                .child(
                    view()
                        .flex_row()
                        .child(
                            text(if selected { "● " } else { "○ " }).text_color(color(Token::Ink)),
                        )
                        .child(label),
                )
                .child(view().pl(term::cells(2.0)).child(consequence))
                .into_element()
        } else {
            choice
                .flex_1()
                .min_w(px(0.0))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .border(px(if selected { 1.5 } else { 1.0 }))
                .border_color(color(if selected { Token::Ink } else { Token::Rule2 }))
                .when(!enabled && !selected, |c| c.opacity(0.8))
                .child(label)
                .child(consequence)
                .into_element()
        }
    });
    let options = if form == Form::Terminal {
        view().children(options.collect::<Vec<_>>())
    } else {
        view()
            .flex_row()
            .items_stretch()
            .gap(px(8.0))
            .children(options.collect::<Vec<_>>())
    };
    frame(
        app,
        Edge::Asks,
        true,
        vec![
            asks_head(app, card.heading, None),
            rich(app, &card.question),
            options.into_element(),
            rich_small(app, &card.answer),
        ],
    )
}

/// The head of a card that asks (`.askuser .hd`): the needs-you dot, the heading, a chip at the
/// trailing edge.
fn asks_head(app: AppState, heading: &'static str, state: Option<&Chip>) -> AnyElement {
    let mut parts = vec![
        dot(app, Hue::You),
        words(app, heading, meta_size(app.form), Token::Ink3)
            .flex_1()
            .min_w(px(0.0))
            .into_element(),
    ];
    if let Some(state) = state {
        parts.push(chip(app, state.label.clone(), state.hue));
    }
    row(app, parts, 6.0).into_element()
}

/// The merge (`.filed`): the result chip and the task it went to, the owner's words quoted as
/// appended, and the attribution saying nothing new was filed.
pub fn merged(app: AppState, card: &MergedCard) -> AnyElement {
    let form = app.form;
    let head = row(
        app,
        vec![
            chip(app, card.chip.label.clone(), card.chip.hue),
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(rich_small(app, &card.destination))
                .into_element(),
        ],
        8.0,
    );
    frame(
        app,
        Edge::Plain,
        true,
        vec![
            head.into_element(),
            quote(app, card.quote_label, card.words),
            words(app, card.attribution, meta_size(form), Token::Ink3).into_element(),
        ],
    )
}

/// The write permission (`.permcard`): who asks, its state, the file, the preview lines, the
/// three grants and Deny, the presence note and the disclosure; the prompt's timer note under it.
pub fn permission(app: AppState, card: &PermissionCard) -> AnyElement {
    let form = app.form;
    let size = meta_size(form);
    let choices = card
        .choices
        .iter()
        .map(|choice| choice.button.clone())
        .collect::<Vec<_>>();
    let body = frame(
        app,
        Edge::Asks,
        true,
        vec![
            asks_head(app, card.heading, Some(&card.state)),
            mono_line(
                app,
                card.path,
                if form == Form::Phone { 13.0 } else { 12.0 },
            )
            .into_element(),
            diff(app, &card.lines),
            button_row(app, &choices, ButtonKind::Ghost, 6.0).into_element(),
            words(app, card.presence_note, size, Token::Ink3).into_element(),
            illus(app, card.illustrative),
        ],
    );
    view()
        .gap(px(6.0))
        .child(body)
        .child(words(app, card.timer_note, size, Token::Ink3))
        .into_element()
}

/// The preview lines (`.diff .ln.add`): line number, `+` in the ok colour, the code cut to fit,
/// on the added ground.
fn diff(app: AppState, lines: &[DiffLine]) -> AnyElement {
    let form = app.form;
    let rows = lines.iter().map(|line| {
        let number = diff_mono(form, line.line.to_string(), Token::Ink4);
        let marker = diff_mono(form, line.marker, Token::Ok);
        let code = diff_mono(form, line.text, Token::Ink)
            .flex_1()
            .min_w(px(0.0))
            .truncate();
        if form == Form::Terminal {
            return view()
                .flex_row()
                .child(view().w(term::cells(4.0)).flex_shrink_0().child(number))
                .child(view().w(term::cells(2.0)).flex_shrink_0().child(marker))
                .child(code)
                .into_element();
        }
        let number_column = if form == Form::Phone { 30.0 } else { 38.0 };
        view()
            .flex_row()
            .items_center()
            .bg(hue_fill(Hue::Ok))
            .child(
                view()
                    .w(px(number_column))
                    .flex_shrink_0()
                    .items_end()
                    .pr(px(8.0))
                    .child(number),
            )
            .child(view().w(px(14.0)).flex_shrink_0().child(marker))
            .child(code)
            .into_element()
    });
    let block = view().children(rows.collect::<Vec<_>>());
    if form == Form::Terminal {
        return block.into_element();
    }
    block
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .overflow_hidden()
        .into_element()
}

fn diff_mono(form: Form, content: impl Into<SharedString>, ink: Token) -> Text {
    let t = text(content.into())
        .number_of_lines(1u32)
        .text_color(color(ink));
    if form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_MONO)
            .text_size(px(11.0))
            .line_height(px(17.6))
    }
}

/// The filed Idea (`.filed`): the result chip and provenance, who filed it, the owner's words
/// quoted first and unchanged, then the fields the assistant wrote (editable in the edit state),
/// and Edit, Merge into, Discard (or Save and Cancel while editing).
pub fn filed(app: AppState, card: &FiledCard) -> AnyElement {
    let form = app.form;
    let size = meta_size(form);
    let idea = card.idea;
    let mut head = vec![
        chip(app, card.chip.label.clone(), card.chip.hue),
        view()
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .child(rich_small(app, &card.provenance))
            .into_element(),
    ];
    if card.attached {
        head.push(chip(app, LABELS.link_attached, Hue::Neutral));
    }
    let value_size = if form == Form::Phone { 15.0 } else { 12.5 };
    let title = if card.editing {
        field(app, idea.title, FILED_LABELS.title)
    } else {
        words(app, idea.title, value_size, Token::Ink)
            .font_weight(FontWeight::BOLD)
            .into_element()
    };
    let project = if card.editing {
        let pick = format!("{} ▾", card.project);
        let pick = words(app, pick, note_size(form), Token::Ink2);
        if form == Form::Terminal {
            pick.into_element()
        } else {
            view()
                .flex_row()
                .self_start()
                .items_center()
                .h(px(26.0))
                .px(px(10.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .child(pick)
                .into_element()
        }
    } else {
        words(app, card.project, value_size, Token::Ink).into_element()
    };
    let mut labels = idea
        .labels
        .iter()
        .map(|label| chip(app, *label, Hue::Neutral))
        .collect::<Vec<_>>();
    if let Some(related) = &card.related {
        labels.push(rich_small(app, related));
    }
    let criteria = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 3.0 }))
        .children(idea.criteria.iter().enumerate().map(|(at, criterion)| {
            let entry = if card.editing {
                field(app, *criterion, FILED_LABELS.criteria)
            } else {
                words(app, *criterion, value_size, Token::Ink).into_element()
            };
            view()
                .flex_row()
                .items_start()
                .child(
                    view()
                        .w(if form == Form::Terminal {
                            term::cells(3.0)
                        } else {
                            px(18.0)
                        })
                        .flex_shrink_0()
                        .child(words(app, format!("{}.", at + 1), value_size, Token::Ink)),
                )
                .child(view().flex_1().min_w(px(0.0)).child(entry))
                .into_element()
        }));
    let fields = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 5.0 }))
        .child(label_value(app, FILED_LABELS.title, title))
        .child(label_value(app, FILED_LABELS.project, project))
        .child(label_value(
            app,
            FILED_LABELS.labels,
            row(app, labels, 6.0).into_element(),
        ))
        .child(label_value(
            app,
            FILED_LABELS.criteria,
            criteria.into_element(),
        ))
        .child(label_value(
            app,
            FILED_LABELS.duplicates,
            words(app, idea.duplicates, value_size, Token::Ink3).into_element(),
        ));
    let mut actions = button_row(app, &card.buttons, ButtonKind::Ghost, 8.0);
    if let Some(note) = card.discard_note {
        actions = actions
            .when(form == Form::Terminal, |r| r.child(term::space(2.0)))
            .child(words(app, note, size, Token::Ink3));
    }
    frame(
        app,
        Edge::Plain,
        true,
        vec![
            row(app, head, 8.0).into_element(),
            words(app, card.attribution, size, Token::Ink3).into_element(),
            quote(app, card.quote_label, idea.words),
            fields.into_element(),
            actions
                .when(form != Form::Terminal, |r| r.mt(px(4.0)))
                .into_element(),
        ],
    )
}

/// An editable field of the filed card (`.einput`): the assistant's words on the sunken ground
/// in the focus line, kept by the field until Save or Cancel leaves the edit state.
fn field(app: AppState, value: &'static str, label: &'static str) -> AnyElement {
    let form = app.form;
    let entry = input()
        .default_value(value.to_owned())
        .label(label)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)));
    match form {
        // A terminal draws a field without a fill of its own as a raised band.
        Form::Terminal => entry.bg(AdaptiveColor::TRANSPARENT).into_element(),
        Form::Desktop | Form::Phone => entry
            .w_full()
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 15.0 } else { 12.5 }))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Focus))
            .rounded(px(4.0))
            .px(px(6.0))
            .py(px(3.0))
            .into_element(),
    }
}

/// The Idea filed as written (`.filed`): the saved chip and who filed it, the words quoted, the
/// title from their first line and how it was made, what can be added later.
pub fn saved(app: AppState, card: &SavedCard) -> AnyElement {
    let form = app.form;
    let size = meta_size(form);
    let value_size = if form == Form::Phone { 15.0 } else { 12.5 };
    let title = words(app, "", value_size, Token::Ink)
        .child(strong_run(card.title.clone()).font_weight(FontWeight::BOLD))
        .child(text(format!(" {}", card.title_note)).text_color(color(Token::Ink3)));
    frame(
        app,
        Edge::Plain,
        true,
        vec![
            row(
                app,
                vec![
                    chip(app, card.chip.label.clone(), card.chip.hue),
                    words(app, card.attribution, size, Token::Ink3)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .into_element(),
                ],
                8.0,
            )
            .into_element(),
            quote(app, card.quote_label, &card.words),
            label_value(app, FILED_LABELS.title, title.into_element()),
            words(app, card.note, note_size(form), Token::Ink3).into_element(),
        ],
    )
}

/// The host starting (`.hoststate`): the running mark, the state in bold and what it means, on
/// the corral ground.
pub fn starting(app: AppState, state: &Chip, explanation: &'static str) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone {
        15.0
    } else {
        text_size(form)
    };
    let sentence = words(app, "", size, Token::Ink)
        .flex_1()
        .min_w(px(0.0))
        .child(strong_run(state.label.clone()))
        .child(text(format!(" · {explanation}")));
    let line = view()
        .flex_row()
        .items_center()
        .bg(hue_fill(state.hue))
        .child(mark(app, Mark::Active, state.hue))
        .when(form == Form::Terminal, |r| r.child(term::space(1.0)))
        .child(sentence);
    if form == Form::Terminal {
        return line.into_element();
    }
    line.gap(px(10.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .into_element()
}

/// Pieces in a wrapping row, `gap` points apart (one Cell in a terminal).
fn row(app: AppState, parts: Vec<AnyElement>, gap: f32) -> View {
    let base = view().flex_row().flex_wrap().items_center();
    if app.form != Form::Terminal {
        return base.gap_x(px(gap)).gap_y(px(4.0)).children(parts);
    }
    let mut spaced = Vec::with_capacity(parts.len() * 2);
    for (at, part) in parts.into_iter().enumerate() {
        if at > 0 {
            spaced.push(term::space(1.0));
        }
        spaced.push(part);
    }
    base.children(spaced)
}
