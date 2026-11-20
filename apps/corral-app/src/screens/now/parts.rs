//! Pieces every Now, task and Changes view shares: Now's page model in its scope, running a
//! work action or press, opening a destination in Now's scope, the model's buttons in the kit's
//! look, the stale board's dimming, and keeping the shown task's detail loaded.

use nsh_core::prelude::*;

use crate::model::projects::{NowGroup, ProjectFilter};
use crate::model::work::{self, Action, Button, NowPage, NowPanel, Press};
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

pub(crate) use crate::ui::kit::more::press;

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
        Action::NowTab(tab) => format!("seg-inspector-{}", tab.label().to_lowercase()),
        Action::ToggleWaiting => "btn-waiting".into(),
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
        Press::Open(Live::Review(_)) => "btn-review".into(),
        Press::Ask { .. } => "btn-ask-about".into(),
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
        Press::Closed(press) => super::closed::press_id(press).into(),
        Press::ClearSelection => "btn-clear-selection".into(),
        Press::FindArtifacts(_) => "btn-find-artifacts".into(),
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

/// A task to keep loaded: its id, whether the open-task list carries it, and whether its detail
/// is missing or stale while the link is up.
pub(crate) type Wanted = Option<(crate::model::Id, bool, bool)>;

/// Keeps a task's detail loaded while `target` names it: requests it when absent or stale,
/// revalidates it once when it leaves or joins the open-task list, and pins it in the client's
/// cache while it is shown, letting go when the target changes or the view goes. A served row's
/// change refreshes the pinned detail in the client itself. Requests and pins go to the source
/// the view mounted on (`AppState::scope`).
pub(crate) fn keep_loaded(app: AppState, target: Memo<Wanted>) {
    let app = untracked(|| app.scope());
    let seen = stored(None::<(crate::model::Id, bool)>);
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
        let Some((id, open, needed)) = wanted else {
            return;
        };
        let reference = work::task_ref(&id);
        let previous = seen.replace(Some((id.clone(), open)));
        let moved = previous.is_some_and(|(known, was_open)| known == id && was_open != open);
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

/// What to keep loaded for a task by id, on the open-task list or off it (tracked).
pub(crate) fn wanted_id(feed: &crate::model::daemon::Feed, id: &crate::model::Id) -> Wanted {
    let needed = feed.link == crate::model::daemon::Link::Up
        && feed
            .detail(&work::task_ref(id))
            .is_none_or(|cache| cache.stale && !cache.loading);
    Some((id.clone(), feed.board.task(id).is_some(), needed))
}

/// Now's own project scope and grouping, tracked: every Now builder reads the list it shows.
pub(crate) fn scope(app: AppState) -> (ProjectFilter, NowGroup) {
    (app.scope_filter(), app.now_group())
}

/// Now's page in its own scope, tracked: one model for the head, the list, the loop band and
/// the side panel.
pub(crate) fn now_page(app: AppState) -> Memo<NowPage> {
    let form = app.form;
    computed(move || {
        let selection = app.selection();
        let (filter, grouping) = scope(app);
        app.feed.with(|feed| {
            app.work
                .with(|state| work::now_page(state, feed, &selection, form, &filter, grouping))
        })
    })
}

/// The task the side panel shows, to keep loaded, on the board or off it (tracked); none while
/// the panel holds the digest.
pub(crate) fn shown_task(app: AppState, page: Memo<NowPage>) -> Memo<Wanted> {
    computed(move || {
        let shown = page.with(|page| matches!(page.panel, NowPanel::Task(_) | NowPanel::Closed(_)));
        if !shown {
            return None;
        }
        let id = app.selection().task?;
        app.feed.with(|feed| wanted_id(feed, &id))
    })
}
