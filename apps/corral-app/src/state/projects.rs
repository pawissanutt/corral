//! Device-local scopes and project intents. Storage never contains preview facts or credentials.
use super::AppState;
use crate::model::{
    DataState, Destination, Id, WorkspaceMode, assistant,
    projects::{FilterScreen, Filters, NowGroup, ProjectFilter},
};
use corral_protocol::assistant::ContextRef;
use nsh_core::{prelude::*, storage::Storage};

const FILTERS_KEY: &str = "corral.filters";
pub(super) fn load_filters() -> Filters {
    Storage::get_item(FILTERS_KEY)
        .and_then(|value| Filters::parse(&value))
        .unwrap_or_default()
}

impl AppState {
    pub(super) fn workspace_destination(&self, destination: Destination) -> Destination {
        if self.workspace_mode() == WorkspaceMode::AssistantOnly
            && !matches!(
                destination,
                Destination::Now | Destination::Agents | Destination::Usage | Destination::Settings
            )
        {
            Destination::Now
        } else {
            destination
        }
    }

    pub fn workspace_mode(&self) -> WorkspaceMode {
        self.feed.with(WorkspaceMode::of)
    }

    pub fn project_filter(&self, screen: FilterScreen) -> ProjectFilter {
        self.feed.with(|feed| {
            feed.store.as_deref().map_or(ProjectFilter::All, |store| {
                self.filters
                    .with(|filters| filters.get(store, screen, feed))
            })
        })
    }

    pub fn set_project_filter(&self, screen: FilterScreen, filter: ProjectFilter) {
        let store = self.feed.with_untracked(|feed| feed.store.clone());
        if let Some(store) = store {
            self.filters
                .update(|filters| filters.set(&store, screen, filter));
            self.project_context_removed.set(None);
        }
    }

    /// Explicit opens and selections must reveal their target before filtered builders read it.
    pub(super) fn reveal_live(&self, live: &crate::model::Live) {
        let scope = self.feed.with_untracked(|feed| {
            self.filters.with_untracked(|filters| {
                crate::model::nav::explicit_record_filter(feed, filters, live)
            })
        });
        if let Some((screen, filter)) = scope {
            self.set_project_filter(screen, filter);
        }
    }

    pub fn now_group(&self) -> NowGroup {
        self.feed.with(|feed| {
            feed.store.as_deref().map_or(NowGroup::State, |store| {
                self.filters.with(|filters| filters.now_group(store))
            })
        })
    }

    pub fn set_now_group(&self, group: NowGroup) {
        if let Some(store) = self.feed.with_untracked(|feed| feed.store.clone()) {
            self.filters
                .update(|filters| filters.set_now_group(&store, group));
        }
    }

    pub fn open_project(&self, id: Id) {
        self.change(|nav| nav.open_project(id));
        self.project_context_removed.set(None);
    }

    pub fn open_project_in_now(&self, id: Id) {
        self.set_project_filter(FilterScreen::Now, ProjectFilter::Project(id));
        self.go(Destination::Now);
    }

    pub fn ask_project(&self, id: Id) {
        self.project_context_removed.set(None);
        self.ask_about(Some(assistant::project_context(&id)), None, None);
    }

    /// Tracked default project; removing its chip suppresses it until context changes or Ask closes.
    pub fn composer_project(&self) -> Option<Id> {
        if let Some(project) = self.ask_context.with(|context| {
            context
                .as_ref()
                .filter(|context| context.kind == "project")
                .map(|context| Id::from(context.id.as_str()))
        }) {
            return Some(project);
        }
        let project = self.feed.with(|feed| {
            self.nav.with(|nav| {
                self.filters
                    .with(|filters| assistant::default_project(nav, filters, feed))
            })
        });
        project.filter(|id| {
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
        self.go(Destination::Now);
    }

    pub(super) fn follow_projects(self) {
        effect(move || {
            let encoded = self.filters.with(Filters::encode);
            if let Ok(encoded) = encoded {
                let _ = Storage::set_item(FILTERS_KEY, &encoded);
            }
        });
        let mode = computed(move || self.workspace_mode());
        effect(move || {
            if mode.get() == WorkspaceMode::AssistantOnly {
                untracked(|| {
                    if !matches!(
                        self.destination(),
                        Destination::Agents | Destination::Usage | Destination::Settings
                    ) {
                        self.go(Destination::Now);
                    }
                });
            }
        });
    }
}
