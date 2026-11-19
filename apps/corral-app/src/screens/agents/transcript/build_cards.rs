//! The build tools' cards (`ui/build`): the `task` card (`task · 2 slices, one worktree`, one
//! row per slice, then one line per slice that came back), the `Build finished` card (slices,
//! branch figures, warnings, the gate attempts and review rounds, the exit's progress rail and
//! outcome, or that a later exit superseded it) and the one-row `board plan`. A card's header
//! opens the session's Agents tab.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::Action;
use crate::model::session::transcript::{
    BuildCard, ExitEntry, ExitLine, FiledEntry, FinishCard, GateRow, PlanCard, ReviewRow,
    SliceBack, SliceRow, TaskCard, Told, ToolState,
};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;
use super::parts::{disclosure, glyph, pressable, sections, small, tinted};

/// The widest a card is drawn (the terminal's 108 cells, in points).
const CARD_MAX: f32 = 760.0;
/// The id column of a slice row.
const NAME: f32 = 110.0;

pub(super) fn card(app: AppState, card: &BuildCard, id: &str) -> AnyElement {
    match card {
        BuildCard::Task(task) => task_card(app, task, id),
        BuildCard::Finish(finish) => finish_card(app, finish, id),
        BuildCard::Plan(plan) => plan_row(app, plan),
        BuildCard::Exit(exit) => exit_entry(app, exit, id),
        BuildCard::Filed(filed) => filed_entry(app, filed, id),
    }
}

/// `you chose <label>` muted then bold, pressable to show what the agent was told.
fn chose_row(app: AppState, label: &str, told: &Told, id: &str) -> AnyElement {
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .min_w(px(0.0))
        .child(small(app, "you chose ", Token::Ink3).flex_shrink_0())
        .child(
            words(app, label.to_owned(), Token::Ink)
                .font_weight(if app.form == Form::Terminal {
                    FontWeight::BOLD
                } else {
                    FontWeight::SEMIBOLD
                })
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate(),
        )
        .when(app.form == Form::Terminal, |row| {
            row.child(term::space(1.0))
        })
        .child(disclosure(app, told.open));
    pressable(
        app,
        row,
        Some(&told.action),
        format!("You chose {label}"),
        told.open,
        id,
    )
    .into_element()
}

/// What the host told the agent with an entry, while opened.
fn told_body(app: AppState, told: &Told) -> Option<AnyElement> {
    told.open.then(|| {
        view()
            .when(app.form == Form::Terminal, |block| {
                block.pl(term::cells(2.0))
            })
            .when(app.form != Form::Terminal, |block| {
                block.pl(px(18.0)).py(px(4.0))
            })
            .child(sections(app, &told.body))
            .into_element()
    })
}

/// The owner's exit choice (`you chose Keep iterating`) and the host's one-line result.
fn exit_entry(app: AppState, exit: &ExitEntry, id: &str) -> AnyElement {
    view()
        .min_w(px(0.0))
        .when(app.form != Form::Terminal, |block| {
            block.my(px(4.0)).px(px(4.0))
        })
        .child(chose_row(app, &exit.chose, &exit.told, id))
        .child(exit_line(app, &exit.outcome))
        .children(
            exit.reason
                .clone()
                .map(|reason| small(app, reason, Token::Ink3)),
        )
        .children(told_body(app, &exit.told))
        .into_element()
}

/// The owner's filing (`you chose File and start`) and the tasks filed.
fn filed_entry(app: AppState, filed: &FiledEntry, id: &str) -> AnyElement {
    view()
        .min_w(px(0.0))
        .when(app.form != Form::Terminal, |block| {
            block.my(px(4.0)).px(px(4.0))
        })
        .child(chose_row(app, filed.chose, &filed.told, id))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .min_w(px(0.0))
                .child(glyph(app, ToolState::Ok))
                .when(app.form == Form::Terminal, |row| {
                    row.child(term::space(1.0))
                })
                .child(
                    tinted(app, filed.filed.clone(), Hue::Ok)
                        .flex_1()
                        .min_w(px(0.0)),
                ),
        )
        .children(told_body(app, &filed.told))
        .into_element()
}

/// A mono word in ink, bold: an agent's id.
fn id_word(app: AppState, id: &str) -> Text {
    let word = text(id.to_owned())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        word.font_weight(FontWeight::BOLD)
    } else {
        word.font_family(FONT_MONO)
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
    }
}

fn words(app: AppState, said: impl Into<SharedString>, ink: Token) -> Text {
    let said = text(said.into()).text_color(color(ink));
    if app.form == Form::Terminal {
        said
    } else {
        said.font_family(FONT_TEXT).text_size(px(12.5))
    }
}

/// A card: the header (pressable when it opens a view), then its rows.
fn frame(
    app: AppState,
    head: View,
    action: Option<&Action>,
    id: &str,
    body: Vec<AnyElement>,
) -> AnyElement {
    let head = match action.cloned() {
        Some(action) => head
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .test_id(&format!("btn-card-{id}"))
            .on_press(move || run(app, action.clone())),
        None => head,
    };
    if app.form == Form::Terminal {
        return term::framed(view().px(term::cells(1.0)).children(body))
            .child(term::top_title(head))
            .into_element();
    }
    view()
        .mt(px(6.0))
        .mb(px(6.0))
        .max_w(px(CARD_MAX))
        .rounded(px(8.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(color(Token::Surface))
        .overflow_hidden()
        .child(
            head.px(px(12.0))
                .py(px(6.0))
                .bg(color(Token::Surface2))
                .hover(|s| s.bg(color(Token::Hover))),
        )
        .child(view().px(px(12.0)).py(px(8.0)).gap(px(3.0)).children(body))
        .into_element()
}

/// `state  id  title  profile · model  time`.
fn slice_row(app: AppState, row: &SliceRow) -> AnyElement {
    let gap = if app.form == Form::Terminal { 0.0 } else { 8.0 };
    let pad = |row: View| {
        if app.form == Form::Terminal {
            row.child(term::space(2.0))
        } else {
            row
        }
    };
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(gap))
        .min_w(px(0.0))
        .child(glyph(app, row.state))
        .when(app.form == Form::Terminal, |line| {
            line.child(term::space(1.0))
        })
        .child(
            view()
                .flex_shrink_0()
                .when(app.form != Form::Terminal, |cell| cell.w(px(NAME)))
                .when(app.form == Form::Terminal, |cell| cell.w(term::cells(11.0)))
                .child(id_word(app, &row.id).truncate()),
        );
    let line = pad(line).child(
        words(app, row.title.clone(), Token::Ink)
            .flex_1()
            .min_w(px(0.0))
            .truncate(),
    );
    let line = pad(line).when(app.form != Form::Phone, |line| {
        line.child(
            small(app, row.profile.clone(), Token::Ink3)
                .flex_shrink(2.0)
                .truncate(),
        )
    });
    pad(line)
        .child(tinted(app, row.time.clone(), row.hue).flex_shrink_0())
        .into_element()
}

/// `✓ host-slots finished · 2 files · cargo test 9 passed`.
fn back_line(app: AppState, back: &SliceBack) -> AnyElement {
    let hue = back.state.hue();
    view()
        .flex_row()
        .items_center()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .min_w(px(0.0))
        .child(glyph(app, back.state))
        .when(app.form == Form::Terminal, |line| {
            line.child(term::space(1.0))
        })
        .child(id_word(app, &back.id).text_color(hue_ink(hue)))
        .when(app.form == Form::Terminal, |line| {
            line.child(term::space(1.0))
        })
        .child(
            tinted(app, back.words.clone(), hue)
                .flex_1()
                .min_w(px(0.0))
                .truncate(),
        )
        .into_element()
}

fn task_card(app: AppState, task: &TaskCard, id: &str) -> AnyElement {
    let (lead, rest) = task
        .head
        .split_once(" · ")
        .unwrap_or((task.head.as_str(), ""));
    let head = view()
        .flex_row()
        .items_center()
        .child(words(app, lead.to_owned(), Token::Ink).font_weight(FontWeight::SEMIBOLD))
        .child(small(app, format!(" · {rest}"), Token::Ink3));
    let mut body: Vec<AnyElement> = task.rows.iter().map(|row| slice_row(app, row)).collect();
    if let Some(error) = &task.error {
        body.push(tinted(app, format!("✗ {error}"), Hue::Fail).into_element());
    }
    let card = frame(app, head, Some(&task.action), id, body);
    view()
        .min_w(px(0.0))
        .child(card)
        .children(task.back.iter().map(|back| back_line(app, back)))
        .into_element()
}

fn gate_row(app: AppState, gate: &GateRow) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(words(app, gate.attempt.clone(), Token::Ink).flex_shrink_0())
        .when(app.form == Form::Terminal, |row| {
            row.child(term::space(2.0))
        })
        .child(small(app, gate.tier.clone(), Token::Ink3).flex_shrink_0())
        .when(app.form == Form::Terminal, |row| {
            row.child(term::space(2.0))
        })
        .child(glyph(app, gate.state))
        .child(tinted(app, format!(" {}", gate.word), gate.state.hue()).flex_shrink_0())
        .children(gate.command.clone().map(|command| {
            words(app, format!("  {command}"), Token::Ink)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
        }))
        .child(view().flex_1().min_w(px(0.0)))
        .children(
            gate.time
                .clone()
                .map(|time| small(app, time, Token::Ink3).flex_shrink_0()),
        )
        .into_element()
}

fn review_row(app: AppState, review: &ReviewRow) -> AnyElement {
    let indent = |row: View| {
        if app.form == Form::Terminal {
            row.pl(term::cells(4.0))
        } else {
            row.pl(px(28.0))
        }
    };
    view()
        .min_w(px(0.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
                .child(words(app, review.round.clone(), Token::Ink).flex_shrink_0())
                .when(app.form == Form::Terminal, |row| {
                    row.child(term::space(2.0))
                })
                .child(glyph(app, review.state))
                .child(
                    tinted(app, format!(" {}", review.words), review.state.hue())
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate(),
                ),
        )
        .children(review.findings.iter().map(|(severity, hue, said)| {
            indent(view().flex_row().min_w(px(0.0)))
                .child(tinted(app, format!("{severity}  "), *hue).flex_shrink_0())
                .child(words(app, said.clone(), Token::Ink).flex_1().min_w(px(0.0)))
        }))
        .children(
            review
                .resolved
                .clone()
                .map(|resolved| indent(view()).child(small(app, resolved, Token::Ink3))),
        )
        .into_element()
}

fn exit_line(app: AppState, exit: &ExitLine) -> AnyElement {
    let hue = match exit.state {
        ToolState::Failed => Hue::Fail,
        ToolState::Ok => Hue::Ok,
        ToolState::Unknown => Hue::Held,
        _ => Hue::Neutral,
    };
    view()
        .min_w(px(0.0))
        .child(
            view()
                .flex_row()
                .items_start()
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(glyph(app, exit.state))
                .when(app.form == Form::Terminal, |row| {
                    row.child(term::space(1.0))
                })
                .child(
                    words(app, exit.words.clone(), Token::Ink)
                        .when(hue != Hue::Neutral, |said| said.text_color(hue_ink(hue)))
                        .flex_1()
                        .min_w(px(0.0)),
                )
                .children(
                    exit.facts
                        .clone()
                        .map(|facts| small(app, facts, Token::Ink3)),
                ),
        )
        .children(exit.step.clone().map(|step| small(app, step, Token::Ink3)))
        .into_element()
}

fn finish_card(app: AppState, finish: &FinishCard, id: &str) -> AnyElement {
    let head = view().flex_row().items_center().child(
        words(app, finish.title.clone(), Token::Ink)
            .font_weight(if app.form == Form::Terminal {
                FontWeight::BOLD
            } else {
                FontWeight::SEMIBOLD
            })
            .truncate(),
    );
    let mut body: Vec<AnyElement> = finish
        .slices
        .iter()
        .map(|row| slice_row(app, row))
        .collect();
    body.extend(
        finish
            .figures
            .clone()
            .map(|figures| words(app, figures, Token::Ink).into_element()),
    );
    body.extend(
        finish
            .flags
            .iter()
            .map(|flag| tinted(app, flag.clone(), Hue::Held).into_element()),
    );
    if !finish.verification.is_empty() {
        body.push(small(app, "verification", Token::Ink3).into_element());
        body.extend(finish.verification.iter().map(|gate| gate_row(app, gate)));
    }
    if !finish.reviews.is_empty() {
        body.push(small(app, "review", Token::Ink3).into_element());
        body.extend(finish.reviews.iter().map(|review| review_row(app, review)));
    }
    body.extend(finish.note.iter().flat_map(|note| {
        note.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| small(app, line.to_owned(), Token::Ink3).into_element())
            .collect::<Vec<_>>()
    }));
    if finish.superseded {
        body.push(small(app, "superseded by a later exit choice", Token::Ink3).into_element());
    } else {
        body.extend(
            finish
                .chose
                .clone()
                .map(|chose| small(app, chose, Token::Ink3).into_element()),
        );
        body.extend(finish.steps.iter().map(|step| {
            view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(glyph(app, step.state))
                .when(app.form == Form::Terminal, |row| {
                    row.child(term::space(1.0))
                })
                .child(words(app, step.words.clone(), Token::Ink))
                .into_element()
        }));
        body.extend(finish.exit.as_ref().map(|exit| exit_line(app, exit)));
    }
    if let Some(told) = &finish.told {
        let id = format!("{id}-told");
        let row = view()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(small(app, "what the agent was told", Token::Ink3))
            .child(disclosure(app, told.open));
        body.push(
            pressable(
                app,
                row,
                Some(&told.action),
                "What the agent was told".into(),
                told.open,
                &id,
            )
            .into_element(),
        );
        body.extend(told_body(app, told));
    }
    frame(app, head, Some(&finish.action), id, body)
}

/// `✓ board plan <title> · 4 tasks, 3 dependency edges`, then whether it waits for the owner.
fn plan_row(app: AppState, plan: &PlanCard) -> AnyElement {
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .min_w(px(0.0))
        .child(glyph(app, plan.state))
        .when(app.form == Form::Terminal, |row| {
            row.child(term::space(1.0))
        })
        .child(words(app, "board plan", Token::Ink).font_weight(FontWeight::SEMIBOLD))
        .when(!plan.title.is_empty(), |row| {
            row.when(app.form == Form::Terminal, |row| {
                row.child(term::space(1.0))
            })
            .child(
                words(app, plan.title.clone(), Token::Ink)
                    .flex_shrink(1.0)
                    .truncate(),
            )
        })
        .child(small(app, format!(" · {}", plan.size), Token::Ink3).flex_shrink_0());
    view()
        .min_w(px(0.0))
        .when(app.form != Form::Terminal, |block| {
            block.my(px(4.0)).px(px(4.0))
        })
        .child(line)
        .when(plan.waiting, |block| {
            block.child(tinted(app, "◆ waiting for your confirmation", Hue::You))
        })
        .children(plan.said.clone().map(|said| small(app, said, Token::Ink3)))
        .into_element()
}
