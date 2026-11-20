//! The device-local project scope and project intents. Storage never contains preview facts
//! or credentials.
use super::AppState;
use crate::model::{
    DataState, Destination, Id, ProjectFilter, WorkspaceMode, assistant,
    nav::{self, Area},
    projects::{Scope, Scopes},
};
use corral_protocol::assistant::ContextRef;
use nsh_core::{prelude::*, storage::Storage};

const SCOPE_KEY: &str = "corral.scope";
/// The per-screen filters the one scope replaced; dropped so it does not outlive them.
const FORMER_FILTERS_KEY: &str = "corral.filters";

pub(super) fn load_scopes() -> Scopes {
    Storage::remove_item(FORMER_FILTERS_KEY);
    Storage::get_item(SCOPE_KEY)
        .and_then(|value| Scopes::parse(&value))
        .unwrap_or_default()
}

impl AppState {
    /// The page this workspace shows for `destination`: itself, or in an assistant-only
    /// workspace Usage for Activity's other pages and Home for the board's.
    pub(super) fn workspace_destination(&self, destination: Destination) -> Destination {
        if self.feed.with(|feed| nav::offers(feed, destination)) {
            destination
        } else if destination.area() == Area::Activity {
            Destination::Usage
        } else {
            Destination::Home
        }
    }

    pub fn workspace_mode(&self) -> WorkspaceMode {
        self.feed.with(WorkspaceMode::of)
    }

    /// Tracked: the project scope of the whole app, for the daemon store in view. Home, Tasks,
    /// Sessions and Activity all read it; there is no other project filter.
    pub fn project_scope(&self) -> Scope {
        self.feed.with(|feed| match feed.store.as_deref() {
            Some(store) => self.scopes.with(|scopes| scopes.get(store, feed)),
            None => Scope::All,
        })
    }

    /// Tracked: [`Self::scope`] as the filter the domains' `*_filtered` builders take.
    pub fn scope_filter(&self) -> ProjectFilter {
        self.project_scope().filter()
    }

    /// Scopes every page. A scope set before the daemon names its store (a project's address
    /// opened cold) waits for the store.
    pub fn set_scope(&self, scope: Scope) {
        match self.feed.with_untracked(|feed| feed.store.clone()) {
            Some(store) => {
                self.scopes.update(|scopes| scopes.set(&store, scope));
                self.scope_pending.set(None);
            }
            None => self.scope_pending.set(Some(scope)),
        }
        self.project_context_removed.set(None);
    }

    /// A project's page is Home scoped to it.
    pub fn open_project(&self, id: Id) {
        self.set_scope(Scope::Project(id));
        self.go(Destination::Home);
    }

    pub fn ask_project(&self, id: Id) {
        self.project_context_removed.set(None);
        self.ask_about(Some(assistant::project_context(&id)), None);
    }

    /// Tracked default project: the one in scope. Removing its chip suppresses it until the
    /// context changes or Ask closes.
    pub fn composer_project(&self) -> Option<Id> {
        if let Some(project) = self.ask_context.with(|context| {
            context
                .as_ref()
                .filter(|context| context.kind == "project")
                .map(|context| Id::from(context.id.as_str()))
        }) {
            return Some(project);
        }
        assistant::default_project(&self.project_scope()).filter(|id| {
            self.project_context_removed
                .with(|removed| removed.as_ref() != Some(id))
        })
    }

    pub fn remove_composer_project(&self) {
        self.project_context_removed.set(self.composer_project());
        if self.ask_context.with_untracked(|context| {
            context
                .as_ref()
                .is_some_and(|context| context.kind == "project")
        }) {
            self.ask_context.set(None);
        }
    }

    /// The explicit Ask project brief is rendered as the project chip, not duplicated as a record chip.
    pub fn composer_record_context(&self) -> Option<ContextRef> {
        self.ask_context
            .with(|context| context.clone().filter(|context| context.kind != "project"))
    }

    pub fn preview_no_projects(&self) {
        self.set_data(DataState::Fixture);
        let local = jiff::Zoned::now();
        self.feed
            .preview
            .set(crate::model::preview::feed_no_projects(
                local.timestamp().as_second().max(0) as u64,
                local.date().to_string(),
                local.strftime("%H:%M").to_string(),
                Some(local.time_zone().clone()),
            ));
        self.go(Destination::Home);
    }

    pub(super) fn follow_projects(self) {
        effect(move || {
            let encoded = self.scopes.with(Scopes::encode);
            if let Ok(encoded) = encoded {
                let _ = Storage::set_item(SCOPE_KEY, &encoded);
            }
        });
        // The scope asked for before the store was known lands once it is.
        let store = computed(move || self.feed.with(|feed| feed.store.clone()));
        effect(move || {
            if store.get().is_some()
                && let Some(scope) = untracked(|| self.scope_pending.get())
            {
                untracked(|| self.set_scope(scope));
            }
        });
        // A workspace that turns out to have no projects leaves the pages it does not have.
        let offered = computed(move || {
            let destination = self.destination();
            self.feed.with(|feed| nav::offers(feed, destination))
        });
        effect(move || {
            if !offered.get() {
                untracked(|| self.go(self.destination()));
            }
        });
    }
}
