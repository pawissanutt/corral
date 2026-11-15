//! The published trackers: an executing plan's pipeline, warning, member graph and blockers
//! with the selected member's story beside them.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::Id;
use crate::model::hue::Hue;
use crate::model::plans::{Blocker, Detail, ExecutingView, GraphInput};
use crate::state::AppState;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::page::{Width, columns, context, pane as scroll_pane};
use crate::ui::kit::table::steps;
use crate::ui::kit::text::{h3, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::detail::{header, inset, pipeline};
use super::{graph, story};

/// The narrowest detail that holds the tracker (about 420 pt of graph, pipeline and blockers)
/// beside the 340 pt member story.
const STORY_BESIDE: f32 = 760.0;

/// The selected member's story, tracked; the detail is cloned out before elements are built,
/// so no memo is borrowed while the story's own bindings run.
fn story_view(app: AppState, detail: Memo<Detail>) -> AnyElement {
    dynamic(move || match detail.get() {
        Detail::Executing(tracker) => tracker
            .story
            .as_ref()
            .map(|inspector| story::story(app, inspector)),
        _ => None,
    })
}

/// The story stacked under the tracker on a narrow desktop detail: the side column's sunken
/// ground and paddings, ruled off from the tracker; nothing while no member is selected.
fn stacked_story(app: AppState, detail: Memo<Detail>) -> AnyElement {
    dynamic(move || match detail.get() {
        Detail::Executing(tracker) => tracker.story.as_ref().map(|inspector| {
            view()
                .test_id("plan-side")
                .px(px(18.0))
                .pt(px(16.0))
                .pb(px(24.0))
                .bg(color(Token::BgSunk))
                .border_t(px(1.0))
                .border_color(color(Token::Rule))
                .child(story::story(app, inspector))
                .into_element()
        }),
        _ => None,
    })
}

/// The executing plan. Desktop: the tracker beside the 340 pt story column where the measured
/// detail is wide enough, otherwise one scrolling pane with the story stacked under the
/// tracker on the sunken ground, so the tracker is never squeezed away. Terminal: one
/// scrolling pane, the story under the tracker.
pub(super) fn executing_pane(app: AppState, detail: Memo<Detail>) -> AnyElement {
    match app.form {
        Form::Terminal => scroll_pane(
            app,
            "plan-detail",
            view()
                .child(executing_body(app, detail))
                .child(term::hrule())
                .child(story_view(app, detail))
                .into_element(),
        ),
        Form::Phone => columns(
            app,
            vec![
                (
                    Width::Fill,
                    scroll_pane(app, "plan-main", executing_body(app, detail)),
                ),
                (
                    Width::Fixed(340.0),
                    context(app, "plan-side", story_view(app, detail)),
                ),
            ],
            0,
        ),
        Form::Desktop => {
            let room = signal(None::<f32>);
            let stacked = computed(move || room.get().is_some_and(|width| width < STORY_BESIDE));
            view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
                .child(dynamic(move || {
                    let stacked = stacked.get();
                    untracked(move || {
                        if stacked {
                            return scroll_pane(
                                app,
                                "plan-main",
                                view()
                                    .child(executing_body(app, detail))
                                    .child(stacked_story(app, detail))
                                    .into_element(),
                            );
                        }
                        columns(
                            app,
                            vec![
                                (
                                    Width::Fill,
                                    scroll_pane(app, "plan-main", executing_body(app, detail)),
                                ),
                                (
                                    Width::Fixed(340.0),
                                    context(app, "plan-side", story_view(app, detail)),
                                ),
                            ],
                            0,
                        )
                    })
                }))
                .into_element()
        }
    }
}

/// The tracker's head and body: pipeline, the unmapped-criteria warning, the member graph and
/// what blocks final acceptance. The phone page shows it alone.
pub(super) fn executing_body(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let Detail::Executing(tracker) = detail.get_untracked() else {
        return view().into_element();
    };
    view()
        .min_w(px(0.0))
        .child(header(app, &tracker.header))
        .child(
            inset(app)
                .child(pipeline(app, &tracker.pipeline))
                .children(
                    tracker
                        .warning
                        .as_deref()
                        .map(|sentence| warning(app, &tracker, sentence)),
                )
                .child(flush_caption(app, tracker.members_heading))
                .child(graph::drawing(app, move || members(detail)))
                .child(h3(
                    app,
                    tracker.blockers_heading,
                    Some(tracker.blocker_count.to_string()),
                ))
                .child(steps(
                    app,
                    tracker
                        .blockers
                        .iter()
                        .map(|blocker| (blocker.mark, blocker.hue, blocker_text(app, blocker)))
                        .collect(),
                )),
        )
        .into_element()
}

/// The members the model resolved and the selected one, tracked: the graph redraws for a
/// member press or a board change without rebuilding the tracker around it.
fn members(detail: Memo<Detail>) -> (GraphInput, Option<Id>) {
    detail.with(|detail| match detail {
        Detail::Executing(tracker) => (tracker.members.clone(), tracker.selected.clone()),
        _ => (GraphInput::default(), None),
    })
}

/// A section caption that starts its block (`.h3` with `margin-top: 0`): the kit caption
/// without its space above.
fn flush_caption(app: AppState, label: &'static str) -> AnyElement {
    let caption = h3(app, label, None);
    match app.form {
        Form::Terminal => caption,
        Form::Desktop => view().mt(px(-18.0)).child(caption).into_element(),
        Form::Phone => view().mt(px(-14.0)).child(caption).into_element(),
    }
}

/// The warning bar (`.warnbar`): the stale mark and the sentence in the held colour on its
/// ground, padding 7 12, radius 6, 14 pt under it. A terminal writes the mark and sentence.
fn warning(app: AppState, tracker: &ExecutingView, sentence: &str) -> AnyElement {
    let ink = hue_ink(Hue::Held);
    let words = text(sentence.to_owned())
        .flex_1()
        .min_w(px(0.0))
        .text_color(ink);
    if app.form == Form::Terminal {
        return view()
            .child(
                view()
                    .flex_row()
                    .items_start()
                    .child(mark(app, tracker.warning_mark, Hue::Held))
                    .child(term::space(1.0))
                    .child(words),
            )
            .child(term::blank())
            .into_element();
    }
    let size = if app.form == Form::Phone { 13.5 } else { 12.5 };
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(7.0))
        .mb(px(14.0))
        .rounded(px(6.0))
        .bg(color(Token::HeldBg))
        .role(AccessibilityRole::Alert)
        .child(mark(app, tracker.warning_mark, Hue::Held))
        .child(
            words
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .line_height(px((size * 1.45).round())),
        )
        .into_element()
}

fn blocker_text(app: AppState, blocker: &Blocker) -> AnyElement {
    plain(app, blocker.text.clone())
        .text_color(match blocker.hue {
            Hue::Neutral => color(Token::Ink),
            hue => hue_ink(hue),
        })
        .into_element()
}
