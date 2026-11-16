//! The default filing project is orthogonal to the bounded record brief attached to Ask.
use super::{Action, Composer, Draft, State, live_composer};
use crate::model::{
    Env, Id, NavState, Route,
    daemon::Feed,
    projects::{FilterScreen, Filters, ProjectFilter},
};
use corral_protocol::assistant::{AssistantSend, ContextRef};

pub fn default_project(nav: &NavState, filters: &Filters, feed: &Feed) -> Option<Id> {
    if let Some(Route::Project(id)) = nav.pages.last() {
        return crate::model::projects::contains_project(feed, id).then(|| id.clone());
    }
    let screen = FilterScreen::of(nav.destination)?;
    match filters.get(feed.store.as_deref()?, screen, feed) {
        ProjectFilter::Project(id) => Some(id),
        _ => None,
    }
}

pub fn project_context(id: &Id) -> ContextRef {
    ContextRef {
        kind: "project".into(),
        id: id.to_string(),
        rev: None,
    }
}

/// The wire reference is added once, alongside any task/plan brief, never replacing it.
pub fn attach_default_project(message: &mut AssistantSend, project: Option<&Id>) {
    if !message
        .context
        .iter()
        .any(|context| context.kind == "project")
        && let Some(project) = project
    {
        message.context.push(project_context(project));
    }
}

pub fn composer_with_project(
    state: &State,
    feed: &Feed,
    draft: Draft<'_>,
    project: Option<&Id>,
    env: Env,
) -> Composer {
    let mut composer = live_composer(state, feed, draft, env);
    composer.project_chip = project.map(|id| {
        let mut chip = super::context_chip("project".into(), id.to_string());
        chip.note = "default project for filed ideas";
        chip.remove.action = Action::UnchipProject;
        chip.remove.label = "Remove project";
        chip
    });
    composer
}
