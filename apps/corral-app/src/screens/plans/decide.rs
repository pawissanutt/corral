//! The decide bar of a revision under review and the fields the owner types into: the
//! request-changes note and the resolutions of owner decisions. Fields read their text from the
//! Plans state, so typing never rebuilds the bar or the document around them.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::plans::{Action, Compose, Decide, DecideNote, DecisionRow, Detail, LiveAction};
use crate::model::text::Rich;
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::bar;
use crate::ui::kit::text::rich_small;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::{command, press};

/// The decide bar of the detail, if it has one, with the typed note blanked.
pub(super) fn shape(detail: &Detail) -> Option<Decide> {
    let mut decide = match detail {
        Detail::Awaiting(review) => review.decide.clone(),
        Detail::Diff(diff) => diff.decide.clone()?,
        _ => return None,
    };
    if let Some(compose) = decide.compose.as_mut() {
        compose.text.clear();
    }
    Some(decide)
}

/// A one-line field in the app's input treatment: transparent, ink text, quiet placeholder.
fn field(app: AppState, base: Input) -> AnyElement {
    let form = app.form;
    let field = base
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .bg(AdaptiveColor::TRANSPARENT);
    let field = match form {
        Form::Terminal => field.h(term::rows(1.0)),
        Form::Desktop => field.font_family(FONT_TEXT).text_size(px(13.0)).h(px(22.0)),
        Form::Phone => field.font_family(FONT_TEXT).text_size(px(15.5)).h(px(28.0)),
    };
    let frame = view().flex_row().items_center().min_w(px(0.0));
    match form {
        Form::Terminal => frame.child(term::quiet("› ")).child(field).into_element(),
        Form::Desktop | Form::Phone => frame
            .px(px(10.0))
            .py(px(if form == Form::Phone { 8.0 } else { 5.0 }))
            .rounded(px(if form == Form::Phone { 10.0 } else { 8.0 }))
            .border_1()
            .border_color(color(Token::Rule2))
            .bg(color(Token::Surface))
            .child(field)
            .into_element(),
    }
}

/// One owner decision: the decision as corrald states it, and the owner's resolution under it.
pub(super) fn decision(app: AppState, row: &DecisionRow) -> AnyElement {
    let (plan, decision) = (row.plan.clone(), row.decision.clone());
    let (shown_plan, shown_decision) = (plan.clone(), decision.clone());
    let input = input()
        .test_id("plan-decision-input")
        .label(format!("Resolution of: {}", row.decision))
        .placeholder("Your resolution")
        .readonly(row.locked)
        .value(move || {
            app.plans.with(|state| {
                state
                    .live
                    .resolution_of(&shown_plan, &shown_decision)
                    .to_owned()
            })
        })
        .on_change(move |typed: &str| {
            press(
                app,
                Action::Live(LiveAction::Resolve {
                    plan: plan.clone(),
                    decision: decision.clone(),
                    text: typed.to_owned(),
                }),
            )
        });
    let words = text(row.decision.clone()).text_color(color(Token::Ink));
    let words = if app.form == Form::Terminal {
        words
    } else {
        let size = text_size(app.form);
        words
            .font_family(FONT_TEXT)
            .text_size(px(size))
            .line_height(px((size * 1.45).round()))
    };
    view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .when(row.locked && app.form != Form::Terminal, |v| v.opacity(0.6))
        .child(words)
        .child(field(app, input))
        .into_element()
}

/// The request-changes field in the decide bar.
fn compose_field(app: AppState, compose: &Compose) -> AnyElement {
    let plan = compose.plan.clone();
    let shown = plan.clone();
    let input = input()
        .test_id("plan-changes-input")
        .label(compose.placeholder.clone())
        .placeholder(compose.placeholder.clone())
        .readonly(compose.locked)
        .value(move || {
            app.plans
                .with(|state| state.live.compose_of(&shown).cloned().unwrap_or_default())
        })
        .on_change(move |typed: &str| {
            press(
                app,
                Action::Live(LiveAction::Draft(plan.clone(), typed.to_owned())),
            )
        });
    let width = match app.form {
        Form::Desktop => view().w(px(380.0)).flex_shrink(1.0),
        Form::Phone => view().flex_1(),
        Form::Terminal => view().w(term::cells(40.0)),
    };
    width.min_w(px(0.0)).child(field(app, input)).into_element()
}

/// The bar's controls: the request-changes field with Send and Cancel while it is open, else
/// the decide buttons (on the phone the primary one is wide).
pub(super) fn actions(app: AppState, decide: &Decide, phone: bool) -> Vec<AnyElement> {
    match &decide.compose {
        Some(compose) => {
            let mut controls = vec![compose_field(app, compose)];
            controls.push(command(app, &compose.send, false));
            controls.push(command(app, &compose.cancel, false));
            controls.extend(
                decide
                    .buttons
                    .iter()
                    .map(|button| command(app, button, false)),
            );
            controls
        }
        None => decide
            .buttons
            .iter()
            .map(|button| command(app, button, phone && button.primary))
            .collect(),
    }
}

/// The width under which a desktop decide bar cannot hold its actions and a readable
/// attribution side by side: the actions take about 500 pt with the bar's padding, and the
/// attribution needs about 180 pt before it breaks one word per line.
const SIDE_BY_SIDE: f32 = 680.0;

/// The desktop and terminal decide bar (`.decide`): the actions, then the exact command and
/// attribution, or the refusal or command status in its tone. A desktop detail narrower than
/// `SIDE_BY_SIDE` (a small window beside the 380 pt list) states the line above the bar,
/// left-aligned, and wraps the actions, so neither is squeezed or clipped.
pub(super) fn decide_bar(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let current = computed(move || detail.with(shape));
    let room = signal(None::<f32>);
    let narrow = computed(move || {
        app.form == Form::Desktop && room.get().is_some_and(|width| width < SIDE_BY_SIDE)
    });
    view()
        .flex_none()
        .w_full()
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            let narrow = narrow.get();
            current.get().map(|decide| {
                let line = match &decide.note {
                    Some(line) => Some(note(app, line, !narrow)),
                    None if decide.attribution.is_empty() => None,
                    None => Some(attribution(app, &decide.attribution, !narrow)),
                };
                if !narrow {
                    return bar(app, actions(app, &decide, false), line);
                }
                let wrapped = view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(10.0))
                    .gap_y(px(8.0))
                    .children(actions(app, &decide, false))
                    .into_element();
                view()
                    .children(line.map(|line| {
                        view()
                            .px(px(24.0))
                            .pt(px(10.0))
                            .bg(color(Token::Bg))
                            .border_t(px(1.0))
                            .border_color(color(Token::Rule))
                            .child(line)
                            .into_element()
                    }))
                    .child(bar(app, vec![wrapped], None))
                    .into_element()
            })
        }))
        .into_element()
}

/// A refusal or command status in its tone: failure for refusals, held for waiting, ok when
/// done, quiet otherwise. `trailing`: flush right at the desktop bar's end.
pub(super) fn note(app: AppState, line: &DecideNote, trailing: bool) -> AnyElement {
    let ink = match line.hue {
        Hue::Neutral => color(Token::Ink3),
        hue => hue_ink(hue),
    };
    let words = text(line.text.clone()).text_color(ink);
    if app.form == Form::Terminal {
        return words.number_of_lines(1u32).into_element();
    }
    words
        .when(trailing && app.form == Form::Desktop, |words| {
            words.text_right()
        })
        .font_family(FONT_TEXT)
        .text_size(px(11.5))
        .line_height(px(15.5))
        .into_element()
}

/// The command a decision runs (`.decide .attr`): 11.5 pt quiet, its mono runs in the mono
/// font; flush right at the bar's end when `trailing`. The kit's rich text sets no alignment.
fn attribution(app: AppState, words: &Rich, trailing: bool) -> AnyElement {
    if app.form == Form::Terminal {
        return rich_small(app, words);
    }
    text("")
        .when(trailing, |line| line.text_right())
        .font_family(FONT_TEXT)
        .text_size(px(11.5))
        .line_height(px(15.5))
        .text_color(color(Token::Ink3))
        .children(words.iter().map(|run| {
            let piece = text(run.text.clone());
            if run.mono {
                piece.font_family(FONT_MONO).text_size(px(11.0))
            } else {
                piece
            }
        }))
        .into_element()
}
