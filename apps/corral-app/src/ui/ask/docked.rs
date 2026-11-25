//! A docked session uses its own compact composer, never Ask's draft or key.

use nsh_core::prelude::*;

use crate::model::dock;
use crate::model::status::SessionState;
use crate::model::{Id, Live};
use crate::screens::agents::{Compact, ask_session, ask_view};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::tokens::{Token, color};

pub(super) fn pane(app: AppState) -> AnyElement {
    let selected = computed(move || app.shown_dock());
    let tabs = computed(move || app.dock_tabs());
    let source = computed(move || app.dock_source());
    view()
        .test_id("dock-pane")
        .role(AccessibilityRole::Region)
        .label(dock::DOCK_LABEL)
        .flex_1()
        .min_h(px(0.0))
        .child(dynamic(move || {
            let selected = selected.get();
            let tabs = tabs.get();
            untracked(|| {
                view()
                    .flex_row()
                    .flex_wrap()
                    .flex_none()
                    .gap(px(4.0))
                    .p(px(4.0))
                    .children(
                        tabs.into_iter()
                            .map(|tab| {
                                let chosen = selected.as_ref() == Some(&tab.id);
                                let id = tab.id;
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(2.0))
                                    .pl(px(4.0))
                                    .child(crate::ui::kit::status::session_mark(app, tab.state))
                                    .when(tab.state == SessionState::Blocked, |tab| {
                                        tab.border_b(px(2.0)).border_b_color(
                                            crate::ui::kit::marks::hue_ink(crate::model::Hue::Held),
                                        )
                                    })
                                    .child(button(
                                        app,
                                        tab.label,
                                        if chosen {
                                            ButtonKind::Primary
                                        } else {
                                            ButtonKind::Quiet
                                        },
                                        format!("dock-tab-{id}"),
                                        move || app.select_dock(&id),
                                    ))
                            })
                            .collect::<Vec<_>>(),
                    )
            })
        }))
        .child(dynamic(move || {
            let _ = source.get();
            selected.get().map(|id| untracked(|| session(app, id)))
        }))
        .into_element()
}

fn session(app: AppState, id: Id) -> AnyElement {
    let page = ask_view(app, id.clone());
    let open = id.clone();
    let close = id.clone();
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(
            view()
                .flex_row()
                .justify_end()
                .flex_none()
                .border_b_1()
                .border_b_color(color(Token::Rule))
                .child(button(
                    app,
                    dock::OPEN_PAGE,
                    ButtonKind::Quiet,
                    "btn-open-page",
                    move || {
                        app.open_live(Live::Session(open.clone()));
                    },
                ))
                .child(button(
                    app,
                    dock::UNDOCK,
                    ButtonKind::Quiet,
                    "btn-undock",
                    move || app.undock(&close),
                )),
        )
        .child(ask_session(app, id, page, Compact::Dock))
        .into_element()
}
