//! The desktop page and the side pane share one resizable group; a narrow window lays the
//! pane over the page instead.

use nsh_base::{h_resizable, resizable_panel};
use nsh_core::prelude::*;

use crate::model::pane::{PAGE_LEAST, Placement};
use crate::state::AppState;
use crate::ui::ask::drawer;

pub(super) fn group(app: AppState, page: AnyElement) -> AnyElement {
    let placement = computed(move || app.pane.placement());
    let beside = move || app.pane_open() && matches!(placement.get(), Placement::Beside { .. });
    let focus = element_ref();
    focus.on_focus_in(move || app.pane.focused.set(true));
    focus.on_focus_out(move || app.pane.focused.set(false));
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(
            h_resizable()
                .handle_class("bg-border")
                .handle_accessibility_label("Resize side pane")
                .child(
                    resizable_panel()
                        .min_size(move || px(if beside() { PAGE_LEAST } else { 0.0 }))
                        .child(page),
                )
                .child(
                    resizable_panel()
                        .visible(beside)
                        .fixed(true)
                        .size(move || match placement.get() {
                            Placement::Beside { width, .. } => Some(px(width)),
                            Placement::Over => None,
                        })
                        .min_size(move || match placement.get() {
                            Placement::Beside { least, .. } => px(least),
                            Placement::Over => px(0.0),
                        })
                        .max_size(move || match placement.get() {
                            Placement::Beside { most, .. } => Some(px(most)),
                            Placement::Over => None,
                        })
                        .on_resize(move |width: &Points| app.pane.resize(width.0))
                        .child(when(beside, move || untracked(|| drawer::side(app, focus)))),
                ),
        )
        .into_element()
}

pub(super) fn overlay(app: AppState) -> AnyElement {
    when(
        move || app.pane_open() && app.pane.placement() == Placement::Over,
        move || untracked(|| drawer::overlay(app)),
    )
    .into_element()
}
