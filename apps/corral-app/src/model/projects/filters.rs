//! The `corral.filters` payload is store → screen slug → project name. Null means All and
//! the empty string means Unassigned, avoiding collisions with real names such as "all".
use super::ProjectFilter;
use crate::model::{Destination, daemon::Feed};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterScreen {
    Now,
    Changes,
    Plans,
    Builds,
    Inbox,
    Review,
    Agents,
}
impl FilterScreen {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::Changes => "changes",
            Self::Plans => "plans",
            Self::Builds => "builds",
            Self::Inbox => "inbox",
            Self::Review => "review",
            Self::Agents => "agents",
        }
    }
    pub const fn of(destination: Destination) -> Option<Self> {
        match destination {
            Destination::Now => Some(Self::Now),
            Destination::Changes => Some(Self::Changes),
            Destination::Plans => Some(Self::Plans),
            Destination::Builds => Some(Self::Builds),
            Destination::Inbox => Some(Self::Inbox),
            Destination::Review => Some(Self::Review),
            Destination::Agents => Some(Self::Agents),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NowGroup {
    #[default]
    State,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NowScope<'a> {
    pub filter: &'a ProjectFilter,
    pub grouping: NowGroup,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoreFilters {
    now: ProjectFilter,
    changes: ProjectFilter,
    plans: ProjectFilter,
    builds: ProjectFilter,
    inbox: ProjectFilter,
    review: ProjectFilter,
    agents: ProjectFilter,
    #[serde(rename = "now.group")]
    now_group: NowGroup,
}
impl StoreFilters {
    fn get_mut(&mut self, screen: FilterScreen) -> &mut ProjectFilter {
        match screen {
            FilterScreen::Now => &mut self.now,
            FilterScreen::Changes => &mut self.changes,
            FilterScreen::Plans => &mut self.plans,
            FilterScreen::Builds => &mut self.builds,
            FilterScreen::Inbox => &mut self.inbox,
            FilterScreen::Review => &mut self.review,
            FilterScreen::Agents => &mut self.agents,
        }
    }
    fn get(&self, screen: FilterScreen) -> &ProjectFilter {
        match screen {
            FilterScreen::Now => &self.now,
            FilterScreen::Changes => &self.changes,
            FilterScreen::Plans => &self.plans,
            FilterScreen::Builds => &self.builds,
            FilterScreen::Inbox => &self.inbox,
            FilterScreen::Review => &self.review,
            FilterScreen::Agents => &self.agents,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Filters(BTreeMap<String, StoreFilters>);
impl Filters {
    pub fn parse(value: &str) -> Option<Self> {
        serde_json::from_str(value).ok()
    }
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
    pub fn get(&self, store: &str, screen: FilterScreen, feed: &Feed) -> ProjectFilter {
        self.0
            .get(store)
            .map(|filters| filters.get(screen).resolved(feed))
            .unwrap_or_default()
    }
    pub fn set(&mut self, store: &str, screen: FilterScreen, filter: ProjectFilter) {
        *self.0.entry(store.to_owned()).or_default().get_mut(screen) = filter;
    }
    pub fn now_group(&self, store: &str) -> NowGroup {
        self.0
            .get(store)
            .map(|filters| filters.now_group)
            .unwrap_or_default()
    }
    pub fn set_now_group(&mut self, store: &str, group: NowGroup) {
        self.0.entry(store.to_owned()).or_default().now_group = group;
    }
}
