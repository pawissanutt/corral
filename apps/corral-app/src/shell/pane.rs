//! The desktop page and the side pane share one resizable group; a narrow window lays the
//! pane over the page instead.

use nsh_base::{h_resizable, resizable_panel};
use nsh_core::prelude::*;

use crate::model::dock;
use crate::model::pane::{PAGE_LEAST, Placement};
use crate::state::AppState;
use crate::ui::ask::drawer;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::tokens::{Token, color};

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

/// The pane over a narrow page. It stays mounted while it steps aside for a page a citation in
/// it opened, so it comes back with its tab, transcript, scroll and draft as they were.
pub(super) fn overlay(app: AppState) -> AnyElement {
    when(
        move || app.pane_held() && app.pane.placement() == Placement::Over,
        move || untracked(|| drawer::overlay(app)),
    )
    .into_element()
}

/// Over a narrow page while the pane stepped aside: the one press that brings it back, beside
/// the side pane's toggles and `Ctrl J`, which do the same.
pub(super) fn back_to_pane(app: AppState) -> AnyElement {
    when(
        move || app.pane_aside(),
        move || {
            untracked(|| {
                view()
                    .absolute()
                    .right(px(16.0))
                    .bottom(px(16.0))
                    .rounded(px(8.0))
                    .bg(color(Token::Surface))
                    .shadow_lg()
                    .shadow_color(color(Token::Shell))
                    .child(button_with(
                        app,
                        dock::BACK_TO_PANE,
                        ButtonKind::Ghost,
                        ButtonOptions {
                            icon: Some(Icon::Split),
                            key_hint: Some(dock::KEY),
                            ..ButtonOptions::default()
                        },
                        "pane-back",
                        move || app.bring_pane_back(),
                    ))
                    .into_element()
            })
        },
    )
    .into_element()
}
