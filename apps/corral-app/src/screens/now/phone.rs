//! The phone's Now: the tab's root (the changes summary over the grouped two-line rows) and the
//! task page pushed from a row (its story, actions, live tail and plan). In the Daemon state the
//! root lists the daemon's board under its real summary, and the live task page shows corrald's
//! detail of the task with its legal commands, its captured log tail and its real plan.

use nsh_core::prelude::*;

use crate::model::work::{self, GroupedList, NowTab, Press};
use crate::model::{Form, Hue, Id, Live};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::chip;
use crate::ui::kit::page::{head, pane, search};
use crate::ui::kit::rows::{group_header, task_columns};
use crate::ui::kit::text::dim;
use crate::ui::tokens::{Token, color};

use super::context::{live_graph, tail};
use super::inspector::{content, story};
use super::list::rows;
use super::parts::{act, action_id, dim_when, keep_loaded, press, pressable, quiet, stale, wanted};

const RING: f32 = 2.0;

/// The Now tab's root: the large title, then, scrolling under it, the summary chips (a press opens
/// Changes; a stale board adds its `as of` chip) and the grouped list.
pub(super) fn root(app: AppState) -> AnyElement {
    let now = computed(move || {
        if app.daemon() {
            let selection = app.selection();
            return app.feed.with(|feed| {
                app.work
                    .with(|state| work::daemon_phone_now(state, feed, &selection))
            });
        }
        let env = app.env();
        app.work.with(|state| work::phone_now(state, env))
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
        .child(rows(app, list, computed(|| task_columns(Form::Phone, 0.0))));
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

/// The fixture's task page pushed from a Now row (`Page::Task`): titled with the task id, back to
/// Now, the header's trailing control asks about the task. Its body is the story, the Session,
/// Ask and Review buttons, the live tail and the plan graph; the shell scrolls it.
pub fn task_phone(app: AppState) -> PhonePage {
    let page = computed(move || {
        let env = app.env();
        app.work.with(|state| work::task_page(state, env))
    });
    let (title, back, ask) = untracked(|| {
        page.with(|page| {
            (
                page.title.to_owned(),
                page.back_label,
                page.actions
                    .iter()
                    .find(|button| {
                        matches!(button.action, Some(Press::Work(work::Action::AskAbout(_))))
                    })
                    .cloned(),
            )
        })
    });
    let trailing = ask.and_then(|button| {
        let action = button.action?;
        Some(ask_head(app, button.label.to_string(), action))
    });
    let body = dim_when(view(), stale(app))
        .child(dynamic(move || page.with(|page| task_body(app, page))))
        .into_element();
    PhonePage {
        title,
        back,
        body,
        trailing,
        bar: None,
        ask: true,
        scroll: true,
    }
}

/// The task page pushed for a daemon task (`Live::Task`): titled with its id, back to Now, the
/// header asking about it. Its body is corrald's story of the task, the Session, Ask and Review
/// buttons, the captured log tail and the task's real plan graph; the task's detail stays
/// loaded while the page is open.
pub fn live_task_phone(app: AppState, id: Id) -> PhonePage {
    let target = {
        let id = id.clone();
        computed(move || {
            app.feed
                .with(|feed| feed.board.task(&id).and_then(|task| wanted(feed, task)))
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
            app.feed.with(|feed| live_task_view(&state, feed, &id))
        })
    };
    let plan = {
        let id = id.clone();
        computed(move || {
            app.feed.with(|feed| {
                feed.board
                    .task(&id)
                    .map(|task| work::task_plan_context(feed, task))
            })
        })
    };
    let body = dim_when(view(), stale(app))
        .child(dynamic(move || page.with(|page| live_task_body(app, page))))
        .child(dynamic(move || {
            plan.with(|plan| {
                plan.as_ref().map(|plan| {
                    view()
                        .child(group_header(
                            app,
                            plan.heading.clone(),
                            None,
                            Hue::Neutral,
                            None,
                        ))
                        .child(view().pb(px(20.0)).child(live_graph(app, plan)))
                        .into_element()
                })
            })
        }))
        .into_element();
    PhonePage {
        title: id.to_string(),
        back: "Now",
        body,
        trailing,
        bar: None,
        ask: true,
        scroll: true,
    }
}

/// The live task page's story, controls and tail, or why the task cannot be shown.
#[derive(Debug, Clone, PartialEq)]
enum LiveTaskView {
    Missing(String),
    Ready {
        story: work::Inspector,
        buttons: Vec<work::Button>,
        tail: Option<work::Tail>,
    },
}

fn live_task_view(state: &work::State, feed: &crate::model::daemon::Feed, id: &Id) -> LiveTaskView {
    let Some(task) = feed.board.task(id) else {
        return LiveTaskView::Missing(if feed.board.complete {
            format!("{id} is no longer on the board.")
        } else if let Some(line) = feed.device.board_notice() {
            line.to_owned()
        } else {
            work::NO_BOARD.to_owned()
        });
    };
    LiveTaskView::Ready {
        story: work::task_inspector(state, feed, task, NowTab::Story, Form::Phone),
        buttons: work::task_buttons(feed, task, true),
        tail: work::task_tail(feed, task).filter(|tail| !tail.lines.is_empty()),
    }
}

fn live_task_body(app: AppState, page: &LiveTaskView) -> AnyElement {
    let (story, buttons, captured) = match page {
        LiveTaskView::Missing(words) => return quiet(app, words.clone()),
        LiveTaskView::Ready {
            story,
            buttons,
            tail,
        } => (story, buttons, tail),
    };
    let actions = buttons
        .iter()
        .enumerate()
        // The mock's first two share the row; Review keeps its own width.
        .map(|(index, button)| pressable(app, button, index < 2))
        .collect::<Vec<_>>();
    view()
        .child(content(app, story))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .px(px(18.0))
                .pb(px(14.0))
                .children(actions),
        )
        .child(group_header(app, "Live", None, Hue::Neutral, None))
        .child(view().px(px(18.0)).pb(px(10.0)).child(match captured {
            Some(captured) => tail(app, captured, Some("btn-live-session".into())),
            None => dim(app, work::NO_TAIL).into_element(),
        }))
        .into_element()
}

fn task_body(app: AppState, page: &work::TaskPage) -> AnyElement {
    let actions = page
        .actions
        .iter()
        .enumerate()
        // The mock's first two share the row; Review keeps its own width.
        .map(|(index, button)| pressable(app, button, index < 2))
        .collect::<Vec<_>>();
    let plan = match page.plan.plan {
        Some(plan) => crate::screens::plans::graph(app, plan, page.plan.member),
        None => match page.plan.empty {
            Some(empty) => dim(app, empty).into_element(),
            None => view().into_element(),
        },
    };
    view()
        .child(
            view()
                .px(px(18.0))
                .py(px(14.0))
                .child(story(app, &page.story)),
        )
        .child(
            view()
                .flex_row()
                .gap(px(8.0))
                .px(px(18.0))
                .pb(px(14.0))
                .children(actions),
        )
        .child(group_header(
            app,
            page.live_label,
            None,
            Hue::Neutral,
            Some(page.live_illustrative),
        ))
        .child(view().px(px(18.0)).pb(px(10.0)).child(tail(
            app,
            &page.tail,
            Some("btn-live-session".into()),
        )))
        .child(group_header(
            app,
            page.plan.heading.clone(),
            None,
            Hue::Neutral,
            None,
        ))
        .child(view().px(px(18.0)).pb(px(20.0)).child(plan))
        .into_element()
}
