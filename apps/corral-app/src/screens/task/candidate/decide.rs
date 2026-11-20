//! The decide bar pinned under the candidate (`.decide`, `.pdecide`): Approve and land, Request
//! changes and Ask from the model, Run gate, Start review and (while the gate is red) Land
//! anyway at the trailing edge; above them the lines of operations sent from here. Every
//! operation stays pressable: the engine decides.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::review::{Button, ButtonStyle, DecideBar};
use crate::model::text::flat;
use crate::ui::kit::page::bar;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::onto::onto;
use super::{Ctx, command, command_hinted, rich_at, status};

/// A terminal button's Cells beside its label, the Cells between two actions and the bar's own
/// Cells either side (`page::bar`, `button::button_with`).
const TERM_BUTTON: f32 = 2.0;
const TERM_GAP: f32 = 2.0;
const TERM_EDGES: f32 = 2.0;

/// How a terminal's decide line fits its row; only a button with a short form is shortened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermFit {
    /// Every button, whole.
    Whole,
    /// Request changes as `Changes`.
    Short,
    /// Short, and the secondary operations on the line below with the attribution.
    Below,
}

/// A terminal button's Cells: its words and its marks.
fn cells_of(button: &Button, short: bool) -> f32 {
    let words = match (short, button.short) {
        (true, Some(short)) => short,
        _ => button.label.as_str(),
    };
    words.chars().count() as f32 + TERM_BUTTON
}

/// The Cells of buttons in a row, the gaps between them included.
fn row_cells(widths: &[f32]) -> f32 {
    widths.iter().sum::<f32>() + TERM_GAP * widths.len().saturating_sub(1) as f32
}

impl TermFit {
    /// The fullest line that fits `cells`; whole until the bar is measured.
    fn of(decide: &DecideBar, cells: f32) -> TermFit {
        if cells <= 0.0 {
            return TermFit::Whole;
        }
        let row = |short: bool| {
            let widths: Vec<f32> = decide
                .buttons
                .iter()
                .map(|button| cells_of(button, short))
                .chain(
                    decide
                        .secondary
                        .iter()
                        .map(|button| cells_of(button, false)),
                )
                .collect();
            row_cells(&widths) + TERM_EDGES
        };
        if row(false) <= cells {
            TermFit::Whole
        } else if row(true) <= cells {
            TermFit::Short
        } else {
            TermFit::Below
        }
    }

    /// Whether the attribution fits whole on its line (after the secondary operations when
    /// they went below); a clipped one would say nothing but an ellipsis.
    fn attribution(self, decide: &DecideBar, cells: f32) -> bool {
        let Some(line) = &decide.attribution else {
            return false;
        };
        if cells <= 0.0 {
            return true;
        }
        let words = flat(line).chars().count() as f32;
        let before = match self {
            TermFit::Below if !decide.secondary.is_empty() => {
                let widths: Vec<f32> = decide
                    .secondary
                    .iter()
                    .map(|button| cells_of(button, false))
                    .collect();
                row_cells(&widths) + TERM_GAP
            }
            _ => 0.0,
        };
        before + words + TERM_EDGES <= cells
    }
}

/// The Onto line and the decide bar of the shown candidate; an empty element while none is
/// shown. A terminal measures its row to keep the line whole (`TermFit`).
pub(crate) fn decide(cx: Ctx) -> AnyElement {
    let page = cx.page;
    let terminal = cx.app.form == Form::Terminal;
    // The phone's Onto field stands in the page (`phone_secondary`), not in its pinned bar.
    let onto = (cx.app.form != Form::Phone).then(|| onto(&cx));
    let room = signal(0.0_f32);
    let model = computed(move || {
        let decide = page.with(|page| page.detail.as_ref().map(|detail| detail.decide.clone()))?;
        let cells = room.get() / term::cells(1.0).0;
        let (fit, attribution) = if terminal {
            let fit = TermFit::of(&decide, cells);
            (fit, fit.attribution(&decide, cells))
        } else {
            (TermFit::Whole, decide.attribution.is_some())
        };
        Some((decide, fit, attribution))
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
        .children(onto)
        .child(dynamic(move || {
            model
                .get()
                .map(|(decide, fit, attribution)| decide_bar(&cx, &decide, fit, attribution))
        }))
        .into_element()
}

/// The Onto field, then Run gate, Start review and Land anyway, which a phone keeps in the page
/// rather than in its bar.
pub(crate) fn phone_secondary(cx: &Ctx) -> AnyElement {
    let field = onto(cx);
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
    let buttons = dynamic(move || {
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
    });
    view().child(field).child(buttons).into_element()
}

fn decide_bar(cx: &Ctx, decide: &DecideBar, fit: TermFit, attributed: bool) -> AnyElement {
    let app = cx.app;
    let phone = app.form == Form::Phone;
    let desktop = app.form == Form::Desktop;
    // What approving does is the primary's hint, where the pointer or the focus asks; on
    // desktop the device the owner acts through joins it, so the bar holds its buttons alone.
    // Any other button's hint is its own.
    let mut primary_hint: Vec<String> = decide.explanation.iter().map(|line| flat(line)).collect();
    if desktop {
        primary_hint.extend(decide.attribution.iter().map(|line| flat(line)));
    }
    let primary_hint = (!primary_hint.is_empty()).then(|| primary_hint.join("\n"));
    let mut actions: Vec<AnyElement> = decide
        .buttons
        .iter()
        .map(|button| {
            let hint = match (&button.hint, button.style) {
                (Some(hint), _) => Some(hint.clone()),
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
    // A terminal keeps the attribution under its row of actions, whole or not at all; when the
    // row is still too tight, the secondary operations lead that line, wrapping rather than
    // clipping a button.
    let attribution = decide
        .attribution
        .as_ref()
        .filter(|_| app.form == Form::Terminal && attributed)
        .map(|line| {
            rich_at(app, line, 11.5, Token::Ink3, false)
                .number_of_lines(1u32)
                .into_element()
        });
    let (secondary, attribution) = if fit == TermFit::Below && !secondary.is_empty() {
        let mut line: Vec<AnyElement> = secondary
            .into_iter()
            .map(|button| view().flex_shrink_0().child(button).into_element())
            .collect();
        line.extend(attribution);
        let below = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x(term::cells(TERM_GAP))
            .children(line)
            .into_element();
        (Vec::new(), Some(below))
    } else {
        (secondary, attribution)
    };
    let above: Vec<AnyElement> = decide
        .status
        .iter()
        .map(|line| status::status_line(cx, line))
        .collect();
    if decide.buttons.is_empty() && above.is_empty() {
        return view().into_element();
    }
    let actions = match app.form {
        // One row that fills the bar: the decisions, then the room left, then the secondary
        // operations at the trailing edge. The room has no width of its own, so the row wraps
        // only when the buttons themselves do not fit, and then those go to the next line.
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
