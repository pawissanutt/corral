//! The phone's Now: the tab's root (the changes summary over the grouped two-line rows) and the
//! task page pushed from a row (its story, actions, live tail and plan).

use nsh_core::prelude::*;

use crate::model::Hue;
use crate::model::work::{self, Action, GroupedList};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::chip;
use crate::ui::kit::page::{head, pane, search};
use crate::ui::kit::rows::group_header;
use crate::ui::kit::text::dim;
use crate::ui::tokens::{Token, color};

use super::context::tail;
use super::inspector::story;
use super::list::rows;
use super::parts::{act, action_id, dim_when, model_button, stale};
use crate::model::Form;
use crate::ui::kit::rows::task_columns;

const RING: f32 = 2.0;

/// The Now tab's root: the large title, then, scrolling under it, the summary chips (a press opens
/// Changes; a stale board adds its `as of` chip) and the grouped list.
pub(super) fn root(app: AppState) -> AnyElement {
    let now = computed(move || {
        let env = app.env();
        app.work.with(|state| work::phone_now(state, env))
    });
    let title = untracked(|| now.with(|now| now.title));
    let list: Memo<GroupedList> = computed(move || now.with(|now| now.list.clone()));
    let summary = dynamic(move || {
        now.with(|now| {
            let action = now.summary_action;
            let label = now
                .chips
                .iter()
                .map(|chip| chip.label)
                .collect::<Vec<_>>()
                .join(", ");
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(6.0))
                .px(px(18.0))
                .pt(px(4.0))
                .pb(px(10.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(label)
                .test_id(&action_id(action))
                .on_press(move || act(app, action))
                .focus_visible(|s| {
                    s.outline(px(RING))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(-RING))
                })
                .children(
                    now.chips
                        .iter()
                        .map(|summary| chip(app, summary.label, summary.hue))
                        .collect::<Vec<_>>(),
                )
                .children(now.status.as_of.map(|stamp| chip(app, stamp, Hue::Held)))
                .into_element()
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

/// The task page pushed from a Now row (`Page::Task`): titled with the task id, back to Now, the
/// header's trailing control asks about the task. Its body is the story, the Session, Ask and
/// Review buttons, the live tail and the plan graph; the shell scrolls it.
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
                    .find(|button| matches!(button.action, Some(Action::AskAbout(_))))
                    .cloned(),
            )
        })
    });
    let trailing = ask.and_then(|button| {
        let action = button.action?;
        Some(
            view()
                .min_w(px(44.0))
                .min_h(px(44.0))
                .items_center()
                .justify_center()
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(button.label.to_string())
                .test_id("btn-ask-head")
                .on_press(move || act(app, action))
                .focus_visible(|s| {
                    s.outline(px(RING))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(-RING))
                })
                .child(icon(Icon::Ask, 20.0, color(Token::Ink2)))
                .into_element(),
        )
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

fn task_body(app: AppState, page: &work::TaskPage) -> AnyElement {
    let actions = page
        .actions
        .iter()
        .enumerate()
        .map(|(index, button)| {
            let id = button.action.map_or_else(String::new, action_id);
            // The mock's first two share the row; Review keeps its own width.
            model_button(app, button, id, index < 2)
        })
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
