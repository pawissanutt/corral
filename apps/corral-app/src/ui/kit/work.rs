//! The work model's buttons (`work::Button`) in the kit's look, and the test id of a press by
//! what it does, so a control the model offers carries the same id wherever it is drawn.

use nsh_core::prelude::*;

use crate::model::Live;
use crate::model::work::{self, Button, Press};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::more::press;

fn kind(kind: work::ButtonKind) -> ButtonKind {
    match kind {
        work::ButtonKind::Primary => ButtonKind::Primary,
        work::ButtonKind::Ghost => ButtonKind::Ghost,
        work::ButtonKind::Quiet => ButtonKind::Quiet,
    }
}

/// The leading icon the mock gives a control: the speech bubble on session buttons.
fn glyph(action: Option<&Press>) -> Option<Icon> {
    matches!(
        action,
        Some(Press::Open(Live::Session(_)) | Press::Question { .. })
    )
    .then_some(Icon::Ask)
}

/// A model button in the kit's look: its label, weight and enabled state, pressing it runs its
/// press. A button without one is one the mock shows with no handler; it stays visible and
/// does nothing.
pub fn model_button(app: AppState, button: &Button, test_id: String, wide: bool) -> AnyElement {
    let action = button.action.clone();
    button_with(
        app,
        button.label.to_string(),
        kind(button.kind),
        ButtonOptions {
            icon: glyph(action.as_ref()),
            disabled: !button.enabled,
            wide,
            ..ButtonOptions::default()
        },
        test_id,
        move || {
            if let Some(action) = &action {
                press(app, action.clone());
            }
        },
    )
}

/// A model button whose test id says what it does.
pub fn pressable(app: AppState, button: &Button, wide: bool) -> AnyElement {
    let id = button.action.as_ref().map_or_else(String::new, press_id);
    model_button(app, button, id, wide)
}

/// The test id of any control by what it does.
pub fn press_id(press: &Press) -> String {
    match press {
        Press::Open(Live::Task(id)) => format!("task-{id}"),
        Press::Open(Live::Plan(id)) => format!("plan-{id}"),
        Press::Open(Live::Session(_)) => "btn-open-session".into(),
        Press::Open(Live::Review(_)) => "btn-review".into(),
        Press::Question { .. } => "btn-answer-question".into(),
        Press::Verb { verb, .. } => format!("btn-{}", verb.slug()),
        Press::SubmitVerb(_) => "btn-verb-send".into(),
        Press::CancelVerb => "btn-verb-cancel".into(),
        Press::CloseMenu => "btn-verbs-close".into(),
        Press::Menu(_) => "btn-task-actions".into(),
        Press::Control { control, .. } => format!("btn-{}", control.slug()),
        Press::Next { index, .. } | Press::Answer { index, .. } => format!("btn-next-{index}"),
        Press::Step { task, .. } => crate::model::next::next_id(task),
        Press::Seen(_) => "btn-mark-seen".into(),
        Press::Older(_) => "btn-older".into(),
        Press::RetryBoard => "btn-retry-board".into(),
        Press::FindArtifacts(_) => "btn-find-artifacts".into(),
        Press::Remove(_) => "btn-remove".into(),
    }
}
