//! Pieces every Now, task and Changes view shares: running a work action or press, the model's
//! buttons in the kit's look, the stale board's dimming, and keeping the selected task's detail
//! loaded.

use nsh_core::prelude::*;

use crate::model::work::{self, Action, Button, Press};
use crate::model::{Form, Live};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::text::dim;
use crate::ui::term::cells;

/// Runs a local work action (a tab, a filter, a group's rows) and the effects it returns.
pub(crate) fn act(app: AppState, action: Action) {
    app.act(app.work, move |state, _| state.act(action));
}

/// Runs any press: local actions through the work reducer, record presses into root effects
/// against the feed.
pub(crate) fn press(app: AppState, press: Press) {
    app.reduce(app.work, move |state, feed, _selection, env| {
        state.press(press, feed, env)
    });
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
    matches!(action, Some(Press::Open(Live::Session(_)))).then_some(Icon::Ask)
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

/// The test id of a local control by what it does.
pub(crate) fn action_id(action: Action) -> String {
    match action {
        Action::Go(destination) => format!("btn-{}", destination.slug()),
        Action::Back => "btn-back".into(),
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
        Press::Closed(press) => super::closed::press_id(press).into(),
    }
}

/// Whether the board is a stale snapshot (daemon down or offline), tracked.
pub(crate) fn stale(app: AppState) -> Memo<bool> {
    computed(move || app.feed.with(|feed| work::daemon_status(feed).dimmed))
}

/// The mock's `.stalefx`: a stale board is desaturated and a step fainter, so it reads as the
/// last known state rather than live.
pub(crate) fn dim_when<S: Styled>(s: S, stale: Memo<bool>) -> S {
    s.style_when(move || stale.get(), |s| s.saturate(0.35).opacity(0.78))
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

/// A task to keep loaded: its id, its board row's revision (none off the board), and whether
/// its detail is missing or stale while the link is up.
pub(crate) type Wanted = Option<(crate::model::Id, Option<u64>, bool)>;

/// Keeps a task's detail loaded while `target` names it: requests it when absent or stale,
/// revalidates it when the board row's revision moves, and pins it in the client's cache while
/// it is shown, letting go when the target changes or the view goes. Requests and pins go to
/// the source the view mounted on (`AppState::scope`).
pub(crate) fn keep_loaded(app: AppState, target: Memo<Wanted>) {
    let app = untracked(|| app.scope());
    let seen = stored(None::<(crate::model::Id, Option<u64>)>);
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
    wanted_id(feed, &task.id, Some(task.rev))
}

/// What to keep loaded for a task by id; `rev` is its board row's, absent off the board, so a
/// task leaving the board is revalidated once (tracked).
pub(crate) fn wanted_id(
    feed: &crate::model::daemon::Feed,
    id: &crate::model::Id,
    rev: Option<u64>,
) -> Wanted {
    let needed = feed.link == crate::model::daemon::Link::Up
        && feed
            .detail(&work::task_ref(id))
            .is_none_or(|cache| cache.stale && !cache.loading);
    Some((id.clone(), rev, needed))
}

/// The selected task to keep loaded, on the board or off it, tracked.
pub(crate) fn selected_task(app: AppState) -> Memo<Wanted> {
    let form = app.form;
    computed(move || {
        let selection = app.selection();
        app.feed.with(|feed| {
            app.work.with(
                |state| match work::daemon_selection(state, &selection, feed, form) {
                    Some(task) => wanted(feed, task),
                    None => {
                        work::off_board(&selection, feed).and_then(|id| wanted_id(feed, id, None))
                    }
                },
            )
        })
    })
}
