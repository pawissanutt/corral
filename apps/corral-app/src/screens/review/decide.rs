//! The decide bar pinned under the candidate (`.decide`, `.pdecide`): Approve, Request changes
//! and Ask from the model, a refused one dimmed with its reason as its hint, Retry and Discard
//! at the trailing edge; above them the lines of decisions sent from here and the
//! request-changes note while it is open.

use nsh_core::prelude::*;

use crate::model::review::{Button, ButtonStyle, DecideBar};
use crate::model::text::flat;
use crate::model::{Form, Hue, span};
use crate::ui::kit::page::bar;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::{Ctx, command, command_hinted, compose, rich_at};

/// A terminal button's Cells beside its label, the Cells between two actions and the bar's own
/// Cells either side (`page::bar`, `button::button_with`).
const TERM_BUTTON: f32 = 2.0;
const TERM_GAP: f32 = 2.0;
const TERM_EDGES: f32 = 2.0;

/// How a terminal's decide line fits its row. `Approve <sha7>` is never cut: the SHA is what
/// it binds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermFit {
    /// Every button, whole.
    Whole,
    /// Request changes as `Request`.
    Short,
    /// Short, and Retry and Discard on the line below with the attribution.
    Below,
}

impl TermFit {
    /// The fullest line that fits `cells`; whole until the bar is measured.
    fn of(decide: &DecideBar, cells: f32) -> TermFit {
        if cells <= 0.0 {
            return TermFit::Whole;
        }
        let size = |button: &Button, short: bool| {
            let words = match (short, button.short) {
                (true, Some(short)) => short,
                _ => button.label.as_str(),
            };
            words.chars().count() as f32 + TERM_BUTTON
        };
        let row = |short: bool| {
            let widths: Vec<f32> = decide
                .buttons
                .iter()
                .map(|button| size(button, short))
                .chain(decide.secondary.iter().map(|button| size(button, false)))
                .collect();
            widths.iter().sum::<f32>()
                + TERM_GAP * widths.len().saturating_sub(1) as f32
                + TERM_EDGES
        };
        if row(false) <= cells {
            TermFit::Whole
        } else if row(true) <= cells {
            TermFit::Short
        } else {
            TermFit::Below
        }
    }
}

/// The decide bar of the shown candidate; an empty element while none is shown. A terminal
/// measures its row to keep the line whole (`TermFit`).
pub(super) fn decide(cx: Ctx) -> AnyElement {
    let page = cx.page;
    let terminal = cx.app.form == Form::Terminal;
    let room = signal(0.0_f32);
    let model = computed(move || {
        let decide = page.with(|page| page.detail.as_ref().map(|detail| detail.decide.clone()))?;
        let fit = if terminal {
            TermFit::of(&decide, room.get() / term::cells(1.0).0)
        } else {
            TermFit::Whole
        };
        Some((decide, fit))
    });
    view()
        .flex_none()
        .w_full()
        .min_w(px(0.0))
        .when(terminal, |bar| {
            bar.on_layout(move |frame: &Bounds<Points>| {
                let width = frame.size.width.0;
                if room.with_untracked(|now| *now != width) {
                    room.set(width);
                }
            })
        })
        .child(dynamic(move || {
            model
                .get()
                .map(|(decide, fit)| decide_bar(&cx, &decide, fit))
        }))
        .into_element()
}

/// Retry and Discard, which a phone keeps in the page rather than in its bar.
pub(super) fn phone_secondary(cx: &Ctx) -> AnyElement {
    let page = cx.page;
    let secondary = computed(move || {
        page.with(|page| {
            page.detail
                .as_ref()
                .map(|detail| detail.decide.secondary.clone())
                .unwrap_or_default()
        })
    });
    let cx = cx.clone();
    dynamic(move || {
        let buttons = secondary.get();
        (!buttons.is_empty()).then(|| {
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .px(px(18.0))
                .pt(px(12.0))
                .children(
                    buttons
                        .iter()
                        .map(|button| command(&cx, button))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    })
}

fn decide_bar(cx: &Ctx, decide: &DecideBar, fit: TermFit) -> AnyElement {
    let app = cx.app;
    let phone = app.form == Form::Phone;
    let desktop = app.form == Form::Desktop;
    // The candidate-bound explanation is the primary's hint: it says what approving binds to
    // where the pointer or the focus asks, and a refusal says why it cannot instead. On desktop
    // the device the owner acts through joins it, so the bar holds its buttons alone.
    let mut primary_hint: Vec<String> = decide.explanation.iter().map(|line| flat(line)).collect();
    if desktop {
        primary_hint.extend(decide.attribution.iter().map(|line| flat(line)));
    }
    let primary_hint = (!primary_hint.is_empty()).then(|| primary_hint.join("\n"));
    let mut actions: Vec<AnyElement> = decide
        .buttons
        .iter()
        .map(|button| {
            let hint = match (&button.refusal, button.style) {
                (Some(refusal), _) => Some(refusal.clone()),
                (None, ButtonStyle::Primary) => primary_hint.clone(),
                (None, _) => None,
            };
            match (fit, button.short) {
                (TermFit::Short | TermFit::Below, Some(short)) => {
                    let shortened = Button {
                        label: short.into(),
                        ..button.clone()
                    };
                    command_hinted(cx, &shortened, hint)
                }
                _ => command_hinted(cx, button, hint),
            }
        })
        .collect();
    let secondary: Vec<AnyElement> = if phone {
        Vec::new()
    } else {
        decide
            .secondary
            .iter()
            .map(|button| command(cx, button))
            .collect()
    };
    // A phone has no hover: a refused primary says why on a line of its own.
    let refusal = decide
        .buttons
        .iter()
        .find(|button| button.style == ButtonStyle::Primary)
        .and_then(|button| button.refusal.clone())
        .filter(|_| phone)
        .map(|line| vec![span(line).hue(Hue::Held)]);
    // A terminal keeps the attribution under its row of actions, where the bar clips it; when
    // the row is still too tight, Retry and Discard lead that line.
    let attribution = decide
        .attribution
        .as_ref()
        .filter(|_| app.form == Form::Terminal)
        .map(|line| {
            rich_at(app, line, 11.5, Token::Ink3, false)
                .truncate()
                .into_element()
        });
    let (secondary, attribution) = if fit == TermFit::Below && !secondary.is_empty() {
        let mut line = Vec::with_capacity(secondary.len() * 2 + 1);
        for (at, button) in secondary.into_iter().enumerate() {
            if at > 0 {
                line.push(term::space(TERM_GAP));
            }
            line.push(view().flex_shrink_0().child(button).into_element());
        }
        if let Some(words) = attribution {
            line.push(term::space(TERM_GAP));
            line.push(view().flex_1().min_w(px(0.0)).child(words).into_element());
        }
        let below = view()
            .flex_row()
            .items_center()
            .children(line)
            .into_element();
        (Vec::new(), Some(below))
    } else {
        (secondary, attribution)
    };
    let above: Vec<AnyElement> = refusal
        .iter()
        .map(|line| rich_at(app, line, 13.0, Token::Ink3, false).into_element())
        .chain(
            decide
                .status
                .iter()
                .map(|line| compose::status_line(cx, line)),
        )
        .chain(
            decide
                .composer
                .as_ref()
                .map(|composer| compose::composer(cx, composer)),
        )
        .collect();
    if decide.buttons.is_empty() && above.is_empty() {
        return view().into_element();
    }
    let actions = match app.form {
        // One row that fills the bar: the decisions, then the room left, then Retry and Discard
        // at the trailing edge. The room has no width of its own, so the row wraps only when
        // the buttons themselves do not fit, and then Retry and Discard go to the next line.
        Form::Desktop => vec![
            view()
                .w_full()
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_x(px(10.0))
                .gap_y(px(8.0))
                .children(actions)
                .when(!secondary.is_empty(), |row| {
                    row.child(view().flex_1().min_w(px(0.0))).child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .children(secondary),
                    )
                })
                .into_element(),
        ],
        Form::Terminal => {
            actions.extend(secondary);
            actions
        }
        Form::Phone => actions,
    };
    let pad = match app.form {
        Form::Desktop => view().px(px(20.0)).pt(px(10.0)).gap(px(6.0)),
        Form::Phone => view().px(px(14.0)).pt(px(10.0)).gap(px(6.0)),
        Form::Terminal => view().px(term::cells(1.0)),
    };
    view()
        .flex_none()
        .when(!above.is_empty(), |stack| {
            stack.child(
                pad.border_t(px(1.0))
                    .border_color(color(Token::Rule))
                    .bg(color(Token::Bg))
                    .children(above),
            )
        })
        .child(bar(app, actions, attribution))
        .into_element()
}
