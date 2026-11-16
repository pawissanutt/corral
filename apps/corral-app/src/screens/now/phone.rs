//! The phone's Now: the tab's root (the board's summary over the grouped two-line rows, then
//! the Closed section) and the task page pushed for a task: the task's detail with its legal
//! commands, its captured log tail and its plan, for a board task or, off the board, a closed
//! one from its detail alone.

use nsh_core::prelude::*;

use crate::model::work::{self, GroupedList, Press};
use crate::model::{Form, Hue, Id, Live};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::chip;
use crate::ui::kit::page::{head, pane, search};
use crate::ui::kit::rows::{group_header, task_columns};
use crate::ui::kit::text::dim;
use crate::ui::tokens::{Token, color};

use super::closed::closed_section;
use super::context::{plan_body, tail};
use super::inspector::content;
use super::list::rows;
use super::parts::{
    act, action_id, dim_when, keep_loaded, press, press_id, pressable, stale, wanted, wanted_id,
};

const RING: f32 = 2.0;

/// The Now tab's root: the large title, then, scrolling under it, the summary chips (a press opens
/// Changes; a stale board adds its `as of` chip), the grouped list and the Closed section.
pub(super) fn root(app: AppState) -> AnyElement {
    let now = computed(move || {
        let selection = app.selection();
        app.feed.with(|feed| {
            app.work
                .with(|state| work::daemon_phone_now(state, feed, &selection))
        })
    });
    let title = untracked(|| now.with(|now| now.title));
    let list: Memo<GroupedList> = computed(move || now.with(|now| now.list.clone()));
    let summary = dynamic(move || {
        now.with(|now| {
            if now.chips.is_empty() && now.status.as_of.is_none() {
                return None;
            }
            let label = now
                .chips
                .iter()
                .map(|chip| chip.label.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let strip = view()
                .flex_row()
                .flex_wrap()
                .gap(px(6.0))
                .px(px(18.0))
                .pt(px(4.0))
                .pb(px(10.0))
                .children(
                    now.chips
                        .iter()
                        .map(|summary| chip(app, summary.label.clone(), summary.hue))
                        .collect::<Vec<_>>(),
                )
                .children(
                    now.status
                        .as_of
                        .clone()
                        .map(|stamp| chip(app, stamp, Hue::Held)),
                );
            let strip = match now.summary_action {
                Some(action) => strip
                    .cursor_pointer()
                    .role(AccessibilityRole::Button)
                    .label(label)
                    .test_id(&action_id(action))
                    .on_press(move || act(app, action))
                    .focus_visible(|s| {
                        s.outline(px(RING))
                            .outline_color(color(Token::Focus))
                            .outline_offset(px(-RING))
                    }),
                None => strip,
            };
            Some(strip.into_element())
        })
    });
    let content = dim_when(view(), stale(app))
        .child(summary)
        // The phone's two-line rows show every column.
        .child(rows(app, list, computed(|| task_columns(Form::Phone, 0.0))))
        .child(closed_section(app));
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(head(app, title, None, vec![search(app)]))
        .child(pane(app, "now-phone", content.into_element()))
        .into_element()
}

/// The header's trailing Ask control, asking about the page's task.
fn ask_head(app: AppState, label: String, action: Press) -> AnyElement {
    view()
        .min_w(px(44.0))
        .min_h(px(44.0))
        .items_center()
        .justify_center()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .test_id("btn-ask-head")
        .on_press(move || press(app, action.clone()))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .child(icon(Icon::Ask, 20.0, color(Token::Ink2)))
        .into_element()
}

/// The task page pushed for a task (`Live::Task`): titled with its id, back to Now, the header
/// asking about it. Its body is the task's story, the Session, Ask and Review buttons, the
/// captured log tail and the task's plan graph; the task's detail stays loaded while the page
/// is open, on the board or off it.
pub fn live_task_phone(app: AppState, id: Id) -> PhonePage {
    let target = {
        let id = id.clone();
        computed(move || {
            app.feed.with(|feed| match feed.board.task(&id) {
                Some(task) => wanted(feed, task),
                None => work::off_board_id(feed, &id).and_then(|id| wanted_id(feed, id, None)),
            })
        })
    };
    keep_loaded(app, target);
    let trailing = Some(ask_head(
        app,
        "Ask about this".into(),
        Press::Ask {
            about: Live::Task(id.clone()),
            rev: untracked(|| {
                app.feed
                    .with(|feed| feed.board.task(&id).map(|task| task.rev))
            }),
        },
    ));
    let page = {
        let id = id.clone();
        computed(move || {
            let state = app.work.get();
            app.feed.with(|feed| work::task_page(&state, feed, &id))
        })
    };
    let plan = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| work::page_plan(feed, &id)))
    };
    let body = dim_when(view(), stale(app))
        .child(dynamic(move || page.with(|page| live_task_body(app, page))))
        .child(dynamic(move || {
            plan.with(|plan| plan.as_ref().map(|plan| plan_section(app, plan)))
        }))
        .into_element();
    PhonePage {
        title: id.to_string(),
        body,
        trailing,
        bar: None,
        ask: false,
        scroll: true,
    }
}

/// The page's plan section, as the Live section above it: one heading (opening the plan when
/// there is one) over the plan's graph, or the note that the task is in none, at the sections'
/// own inset.
fn plan_section(app: AppState, plan: &work::LivePlan) -> AnyElement {
    let heading = group_header(app, plan.heading.clone(), None, Hue::Neutral, None);
    let heading = match plan.action.clone() {
        Some(action) => view()
            .cursor_pointer()
            .role(AccessibilityRole::Link)
            .label(plan.heading.clone())
            .test_id(&press_id(&action))
            .on_press(move || press(app, action.clone()))
            .child(heading)
            .into_element(),
        None => heading,
    };
    view()
        .child(heading)
        .child(view().px(px(18.0)).pb(px(20.0)).child(plan_body(app, plan)))
        .into_element()
}

fn live_task_body(app: AppState, page: &work::TaskPage) -> AnyElement {
    let (story, buttons, log) = match page {
        work::TaskPage::Waiting(inspector) => return content(app, inspector),
        work::TaskPage::Ready {
            story,
            buttons,
            log,
        } => (story, buttons, log),
    };
    let actions = body_buttons(app, buttons);
    let page = view().child(content(app, story)).child(
        view()
            .flex_row()
            .flex_wrap()
            .gap(px(8.0))
            .px(px(18.0))
            .pb(px(14.0))
            .children(actions),
    );
    let live = match log {
        work::TaskLog::Absent => None,
        work::TaskLog::Empty => Some(dim(app, work::NO_TAIL).into_element()),
        work::TaskLog::Lines(captured) => {
            Some(tail(app, captured, Some("btn-live-session".into())))
        }
    };
    page.children(live.map(|live| {
        view()
            .child(group_header(app, "Live", None, Hue::Neutral, None))
            .child(view().px(px(18.0)).pb(px(10.0)).child(live))
    }))
    .into_element()
}

/// The page's buttons under its story, without Ask: the header's control is the page's one
/// Ask affordance. The first two share the row, as in the mock; Review keeps its own width.
fn body_buttons(app: AppState, buttons: &[work::Button]) -> Vec<AnyElement> {
    buttons
        .iter()
        .filter(|button| !matches!(button.action, Some(Press::Ask { .. })))
        .enumerate()
        .map(|(index, button)| pressable(app, button, index < 2))
        .collect()
}
