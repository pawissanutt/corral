//! Pieces every Now, task and Changes view shares: running a work action, the inbox's
//! question projection, the model's buttons in the kit's look, and the stale board's dimming.

use nsh_core::prelude::*;

use crate::model::Env;
use crate::model::inbox::QuestionState;
use crate::model::work::{self, Action, Button};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};

/// Runs a work-domain action and the effects it returns.
pub(crate) fn act(app: AppState, action: Action) {
    app.act(app.work, move |state, env| state.act(action, env));
}

/// Whether question T-868/R1/Q1 is still open, as the inbox's state says (tracked): the work
/// builders take it for their question-projected rows.
pub(crate) fn question_open(app: AppState, env: Env) -> bool {
    app.inbox
        .with(|state| state.question_state(env) == QuestionState::Open)
}

fn kind(kind: work::ButtonKind) -> ButtonKind {
    match kind {
        work::ButtonKind::Primary => ButtonKind::Primary,
        work::ButtonKind::Ghost => ButtonKind::Ghost,
        work::ButtonKind::Quiet => ButtonKind::Quiet,
    }
}

/// The leading icon the mock gives a control: the speech bubble on session buttons.
fn glyph(action: Option<Action>) -> Option<Icon> {
    matches!(action, Some(Action::OpenSession(_))).then_some(Icon::Ask)
}

/// A model button in the kit's look: its label, weight and enabled state, pressing it runs its
/// action. A button without an action is one the mock shows with no handler; it stays visible and
/// does nothing.
pub(crate) fn model_button(
    app: AppState,
    button: &Button,
    test_id: String,
    wide: bool,
) -> AnyElement {
    let action = button.action;
    button_with(
        app,
        button.label.to_string(),
        kind(button.kind),
        ButtonOptions {
            icon: glyph(action),
            disabled: !button.enabled,
            wide,
            ..ButtonOptions::default()
        },
        test_id,
        move || {
            if let Some(action) = action {
                act(app, action);
            }
        },
    )
}

/// The test id of a control by what it does.
pub(crate) fn action_id(action: Action) -> String {
    match action {
        Action::Select(id) | Action::OpenTask(id) => format!("task-{id}"),
        Action::OpenPlan(id) => format!("plan-{id}"),
        Action::OpenSession(_) => "btn-open-session".into(),
        Action::AskAbout(_) => "btn-ask-about".into(),
        Action::OpenQuestion => "btn-answer".into(),
        Action::Go(destination) => format!("btn-{}", destination.slug()),
        Action::Back => "btn-back".into(),
        Action::Goals => "btn-goals".into(),
        Action::MoreQueued => "btn-more-queued".into(),
        Action::ParkedSummary => "btn-parked".into(),
        Action::SampleQuestion => "inbox-pm".into(),
        Action::MarkSeen => "btn-mark-seen".into(),
        Action::Connection => "btn-connection".into(),
        Action::RestartDaemon => "btn-restart-daemon".into(),
        Action::NowTab(tab) => format!("seg-inspector-{}", super::layout::inspector_panel(tab)),
        Action::CtxTab(tab) => format!("seg-context-{}", super::layout::context_panel(tab)),
        Action::LowTab(tab) => format!("seg-lower-{}", super::layout::lower_panel(tab)),
    }
}

/// Whether the board is a stale snapshot (daemon down or offline), tracked.
pub(crate) fn stale(app: AppState) -> Memo<bool> {
    computed(move || work::status(app.env()).dimmed)
}

/// The mock's `.stalefx`: a stale board is desaturated and a step fainter, so it reads as the
/// last known state rather than live.
pub(crate) fn dim_when<S: Styled>(s: S, stale: Memo<bool>) -> S {
    s.style_when(move || stale.get(), |s| s.saturate(0.35).opacity(0.78))
}
