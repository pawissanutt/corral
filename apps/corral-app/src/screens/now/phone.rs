//! The phone's Now: the tab's root and the task page pushed for a task.
//!
//! The root is the large title with its counts, then, scrolling under it, the project scope
//! (its chips scrolling sideways) with the Group toggle, the line saying what waits in other
//! projects, the `Since …` row with what changed (a press opens Changes in Now's scope), the
//! board's notice, the sections of two-line rows, Waiting and Closed. No loop band: the phone
//! has no room for five cells. An owner move's row opens its record; a task's row pushes its
//! page: the task's detail with its legal commands, its captured log tail and its plan, for a
//! board task or, off the board, a closed one from its detail alone.

use nsh_core::prelude::*;

use crate::model::work::{self, NowDigest, NowPage, NowPanel, Press};
use crate::model::{Destination, Hue, Id, Live};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::glance::{page_head, stat_in};
use crate::ui::kit::page::{pane, search};
use crate::ui::kit::project_filter::project_filter_grouped;
use crate::ui::kit::rows::group_header;
use crate::ui::kit::text::dim;
use crate::ui::tokens::{Token, color};

use super::context::{plan_body, tail};
use super::inspector::content;
use super::list::{board_status, list, scope_note};
use super::parts::{
    dim_when, go_scoped, keep_loaded, now_page, press, press_id, pressable, stale, wanted,
    wanted_id,
};
use super::rows::{PHONE_PAD, words};

const RING: f32 = 2.0;

/// The Now tab's root.
pub(super) fn root(app: AppState) -> AnyElement {
    let page = now_page(app);
    let title = untracked(|| page.with(|page| page.title));
    let stats = computed(move || page.with(|page| page.stats.clone()));
    let content = dim_when(view(), stale(app))
        .child(project_filter_grouped(app))
        .child(scope_note(app, page))
        .child(since(app, page))
        .child(board_status(app, page))
        .child(list(app, page));
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(dynamic(move || {
            stats.with(|stats| page_head(app, title, stats, vec![search(app)]))
        }))
        .child(pane(app, "now-phone", content.into_element()))
        .into_element()
}

/// The digest as one row (`Since …` and what landed, went stale and happened); a press opens
/// Changes in Now's scope. test id `btn-since`.
fn since(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let digest = computed(move || {
        page.with(|page| match &page.panel {
            NowPanel::Digest(digest) => Some(digest.clone()),
            _ => None,
        })
    });
    dynamic(move || {
        digest.with(|digest| digest.as_ref().map(|digest| since_row(app, digest, page)))
    })
}

fn since_row(app: AppState, digest: &NowDigest, page: Memo<NowPage>) -> AnyElement {
    let form = app.form;
    let spoken = std::iter::once(digest.since.as_str())
        .chain(digest.stats.iter().map(|stat| stat.label()))
        .collect::<Vec<_>>()
        .join(", ");
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .min_h(px(48.0))
        .px(px(PHONE_PAD))
        .py(px(10.0))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(spoken)
        .test_id("btn-since")
        .on_press(move || go_scoped(app, page, Destination::Changes))
        .active(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .child(
            words(form, digest.since.clone(), 14.0, color(Token::Ink2))
                .flex_1()
                .min_w(px(0.0)),
        )
        .children(
            digest
                .stats
                .iter()
                .map(|stat| stat_in(app, stat))
                .collect::<Vec<_>>(),
        )
        .child(icon(Icon::Chevron, 14.0, color(Token::Ink3)))
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
