//! Pieces every Now, task and Changes view shares: running a work action or press, the inbox's
//! question projection, the model's buttons in the kit's look, the stale board's dimming, the
//! choice between the fixture's and the daemon's content, and keeping the selected task's
//! detail loaded.

use nsh_core::prelude::*;

use crate::model::inbox::QuestionState;
use crate::model::work::{self, Action, Button, Press};
use crate::model::{Env, Form, Live};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::text::dim;
use crate::ui::term::cells;

/// Runs a fixture or form-independent work action and the effects it returns.
pub(crate) fn act(app: AppState, action: Action) {
    app.act(app.work, move |state, env| state.act(action, env));
}

/// Runs any press: fixture actions through the work reducer, live presses into root effects
/// against the daemon's feed.
pub(crate) fn press(app: AppState, press: Press) {
    app.reduce(app.work, move |state, feed, _selection, env| {
        state.press(press, feed, env)
    });
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
fn glyph(action: Option<&Press>) -> Option<Icon> {
    matches!(
        action,
        Some(Press::Work(Action::OpenSession(_)) | Press::Open(Live::Session(_)))
    )
    .then_some(Icon::Ask)
}

/// A model button in the kit's look: its label, weight and enabled state, pressing it runs its
/// press. A button without one is one the mock shows with no handler; it stays visible and
/// does nothing.
pub(crate) fn model_button(
    app: AppState,
    button: &Button,
    test_id: String,
    wide: bool,
) -> AnyElement {
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
pub(crate) fn pressable(app: AppState, button: &Button, wide: bool) -> AnyElement {
    let id = button.action.as_ref().map_or_else(String::new, press_id);
    model_button(app, button, id, wide)
}

/// The test id of a fixture control by what it does.
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
        Action::ShowAll(group) => format!("btn-more-{}", group.key()),
        Action::NowTab(tab) => format!("seg-inspector-{}", super::layout::inspector_panel(tab)),
        Action::CtxTab(tab) => format!("seg-context-{}", super::layout::context_panel(tab)),
        Action::LowTab(tab) => format!("seg-lower-{}", super::layout::lower_panel(tab)),
        Action::Filter(filter) => format!("seg-changes-{}", filter.label().to_lowercase()),
    }
}

/// The test id of any control by what it does.
pub(crate) fn press_id(press: &Press) -> String {
    match press {
        Press::Work(action) => action_id(*action),
        Press::Select(id) | Press::Open(Live::Task(id)) => format!("task-{id}"),
        Press::Open(Live::Plan(id)) => format!("plan-{id}"),
        Press::Open(Live::Session(_)) => "btn-open-session".into(),
        Press::Open(Live::Question(_)) => "btn-answer".into(),
        Press::Open(Live::Review(_)) => "btn-review".into(),
        Press::Open(Live::Thread(id)) => format!("thread-{id}"),
        Press::Ask { .. } => "btn-ask-about".into(),
        Press::Verb { verb, .. } => format!("btn-{}", verb.slug()),
        Press::SubmitVerb(_) => "btn-verb-send".into(),
        Press::CancelVerb => "btn-verb-cancel".into(),
        Press::CloseMenu => "btn-verbs-close".into(),
        Press::Recover(_) => "btn-check-result".into(),
        Press::Goal(_) => "btn-goal".into(),
        Press::Seen(_) => "btn-mark-seen".into(),
        Press::Older(_) => "btn-older".into(),
        Press::RetryBoard => "btn-retry-board".into(),
    }
}

/// Whether the board is a stale snapshot (daemon down or offline), tracked.
pub(crate) fn stale(app: AppState) -> Memo<bool> {
    computed(move || {
        if app.daemon() {
            app.feed.with(|feed| work::daemon_status(feed).dimmed)
        } else {
            work::status(app.env()).dimmed
        }
    })
}

/// The mock's `.stalefx`: a stale board is desaturated and a step fainter, so it reads as the
/// last known state rather than live.
pub(crate) fn dim_when<S: Styled>(s: S, stale: Memo<bool>) -> S {
    s.style_when(move || stale.get(), |s| s.saturate(0.35).opacity(0.78))
}

/// `fixture()` in the Fixture state, `daemon()` in the Daemon state, built again when the data
/// state changes. Each builder tracks what it reads itself.
pub(crate) fn by_data(
    app: AppState,
    fixture: impl Fn() -> AnyElement + 'static,
    daemon: impl Fn() -> AnyElement + 'static,
) -> AnyElement {
    dynamic(move || {
        let served = app.daemon();
        untracked(|| if served { daemon() } else { fixture() })
    })
}

/// One quiet line where a panel has nothing to show: what the daemon does not serve, why there
/// is no board, or a detail's state.
pub(crate) fn quiet(app: AppState, words: impl Into<SharedString>) -> AnyElement {
    let line = view().child(dim(app, words.into()));
    match app.form {
        Form::Terminal => line.px(cells(1.0)),
        Form::Desktop => line.px(px(16.0)).py(px(10.0)),
        Form::Phone => line.px(px(18.0)).py(px(14.0)),
    }
    .into_element()
}

/// A task to keep loaded: its id, its board row's revision, and whether its detail is missing
/// or stale while the link is up.
pub(crate) type Wanted = Option<(crate::model::Id, u64, bool)>;

/// Keeps a task's detail loaded while `target` names it: requests it when absent or stale,
/// revalidates it when the board row's revision moves, and pins it in the client's cache while
/// it is shown, letting go when the target changes or the view goes.
pub(crate) fn keep_loaded(app: AppState, target: Memo<Wanted>) {
    let seen = stored(None::<(crate::model::Id, u64)>);
    let pinned = stored(None::<crate::model::Id>);
    effect(move || {
        let wanted = target.get();
        let shown = wanted.as_ref().map(|(id, _, _)| id.clone());
        if pinned.with(|pinned| *pinned != shown) {
            let was = pinned.replace(shown.clone());
            untracked(|| {
                if let Some(gone) = was {
                    app.watch_detail(work::task_ref(&gone), false);
                }
                if let Some(new) = &shown {
                    app.watch_detail(work::task_ref(new), true);
                }
            });
        }
        let Some((id, rev, needed)) = wanted else {
            return;
        };
        let reference = work::task_ref(&id);
        let previous = seen.replace(Some((id.clone(), rev)));
        let moved = previous.is_some_and(|(known, known_rev)| known == id && known_rev != rev);
        untracked(|| {
            if moved {
                app.refresh(reference);
            } else if needed {
                app.load(reference);
            }
        });
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if let Some(gone) = pinned.try_with(Option::take).flatten() {
            app.watch_detail(work::task_ref(&gone), false);
        }
    });
}

/// What to keep loaded for a daemon task, read from the feed (tracked).
pub(crate) fn wanted(
    feed: &crate::model::daemon::Feed,
    task: &crate::model::daemon::Task,
) -> Wanted {
    let needed = feed.link == crate::model::daemon::Link::Up
        && feed
            .detail(&work::task_ref(&task.id))
            .is_none_or(|cache| cache.stale && !cache.loading);
    Some((task.id.clone(), task.rev, needed))
}

/// The selected daemon task to keep loaded, tracked; `None` outside the Daemon state.
pub(crate) fn selected_task(app: AppState) -> Memo<Wanted> {
    let form = app.form;
    computed(move || {
        if !app.daemon() {
            return None;
        }
        let selection = app.selection();
        app.feed.with(|feed| {
            app.work.with(|state| {
                work::daemon_selection(state, &selection, feed, form)
                    .and_then(|task| wanted(feed, task))
            })
        })
    })
}
