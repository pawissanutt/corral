//! The default filing project is orthogonal to the record Ask is about.
use crate::model::{
    Id, NavState, Route,
    daemon::Feed,
    projects::{FilterScreen, Filters, ProjectFilter},
};
use corral_protocol::assistant::ContextRef;

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
