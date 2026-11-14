//! The pinned decide bar.

use nsh_core::prelude::*;

use crate::model::review::{ButtonStyle, DecideBar};
use crate::model::{Form, Hue, span};
use crate::ui::kit::page::bar;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::{ANY_WIDTH, Ctx, command, compose, page_view, rich_at};

/// The width under which a desktop decide bar cannot hold its actions and a readable
/// attribution side by side: Approve, Request changes and Discard take about 420 pt with the
/// bar's padding, and the attribution needs about 220 pt before it breaks a word a line. The
/// same threshold as the Plans decide bar.
const SIDE_BY_SIDE: f32 = 680.0;

/// The decide bar (`.decide`, `.pdecide`): the request-changes note and the lines of sent
/// decisions above the model's buttons, then, on desktop and terminal, Retry and Discard, the
/// verdict-binding explanation, a refusal and the authenticated device at the trailing edge.
/// A desktop bar narrower than `SIDE_BY_SIDE` (a small window beside the candidate list) states
/// those lines above the bar, left-aligned, and wraps the actions, so neither is squeezed.
pub(super) fn decide(cx: Ctx) -> AnyElement {
    let model = {
        let cx = cx.clone();
        computed(move || page_view(&cx, ANY_WIDTH).decide)
    };
    let app = cx.app;
    let room = signal(None::<f32>);
    let narrow = computed(move || {
        app.form == Form::Desktop && room.get().is_some_and(|width| width < SIDE_BY_SIDE)
    });
    view()
        .flex_none()
        .w_full()
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
        .child(dynamic(move || decide_bar(&cx, &model.get(), narrow.get())))
        .into_element()
}

fn decide_bar(cx: &Ctx, decide: &DecideBar, narrow: bool) -> AnyElement {
    let app = cx.app;
    let phone = app.form == Form::Phone;
    let mut actions: Vec<AnyElement> = decide
        .buttons
        .iter()
        .map(|button| command(cx, button))
        .collect();
    if !phone {
        actions.extend(decide.secondary.iter().map(|button| command(cx, button)));
    }
    let refusal = decide
        .buttons
        .iter()
        .find(|button| button.style == ButtonStyle::Primary)
        .and_then(|button| button.refusal.clone())
        .filter(|_| !phone)
        .map(|line| vec![span(line).hue(Hue::Held)]);
    let lines: Vec<AnyElement> = refusal
        .iter()
        .chain(decide.explanation.iter())
        .chain(decide.attribution.iter())
        .map(|line| {
            rich_at(app, line, 11.5, Token::Ink3, false)
                .when(!narrow, |t| t.text_right())
                .when(app.form == Form::Terminal, |t| t.truncate())
                .into_element()
        })
        .collect();
    // The attribution gives way to the buttons: a terminal row sets it beside them unwrapped,
    // and a box does not shrink unless told to.
    let attribution = (!lines.is_empty()).then(|| {
        view()
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .when(!narrow, |lines| lines.items_end())
            .children(lines)
            .into_element()
    });
    let mut above: Vec<AnyElement> = decide
        .status
        .iter()
        .map(|line| compose::status_line(cx, line))
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
    let (actions, attribution) = if narrow {
        // Too narrow to stand side by side: the lines go above, the actions wrap.
        above.extend(attribution);
        let wrapped = view()
            .flex_1()
            .min_w(px(0.0))
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x(px(10.0))
            .gap_y(px(8.0))
            .children(actions)
            .into_element();
        (vec![wrapped], None)
    } else {
        (actions, attribution)
    };
    let pad = match app.form {
        Form::Desktop => view().px(px(24.0)).pt(px(10.0)).gap(px(6.0)),
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
