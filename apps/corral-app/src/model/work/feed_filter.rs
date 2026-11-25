//! Activity › Feed's own filters: an event category, who acted and a search over the task id
//! and the words a row shows. They combine (every one set must match), narrow only the loaded
//! rows and never the project scope, Mark seen or Load older; a page loaded later is filtered
//! the same way because the page is rebuilt from every loaded event.

use corral_types::{Actor, Event, EventKind};

use super::failed;

/// What an event is about, the Feed's first filter and the mark a row leads with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Created, edited, moved, dropped or reopened.
    Task,
    /// Dispatched, resumed, replied, done or stopped.
    Run,
    /// Gate, review, verdict, sent back, ready and landed.
    Review,
    /// Comments and artifacts.
    Note,
    /// Blocked, errors and forced operations; a row with any alert, a failed gate too.
    Problem,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Task,
        Category::Run,
        Category::Review,
        Category::Note,
        Category::Problem,
    ];

    pub const fn of(kind: EventKind) -> Category {
        match kind {
            EventKind::Created
            | EventKind::Edited
            | EventKind::Moved
            | EventKind::Dropped
            | EventKind::Reopened => Category::Task,
            EventKind::Dispatched
            | EventKind::Resumed
            | EventKind::Replied
            | EventKind::Done
            | EventKind::Stopped => Category::Run,
            EventKind::Gate
            | EventKind::ReviewStarted
            | EventKind::Verdict
            | EventKind::SentBack
            | EventKind::Ready
            | EventKind::Landed => Category::Review,
            EventKind::Commented | EventKind::Artifact => Category::Note,
            EventKind::Blocked | EventKind::Error | EventKind::Forced => Category::Problem,
        }
    }

    /// The word a filter option and a row's mark carry.
    pub const fn label(self) -> &'static str {
        match self {
            Category::Task => "Tasks",
            Category::Run => "Runs",
            Category::Review => "Review",
            Category::Note => "Notes",
            Category::Problem => "Problems",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Category::Task => "task",
            Category::Run => "run",
            Category::Review => "review",
            Category::Note => "note",
            Category::Problem => "problem",
        }
    }
}

/// Who acted, as the Feed groups the engine's actors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Who {
    Owner,
    /// Workers, reviewers and the assistant.
    Agent,
    /// Corral's own automation and extensions.
    Automation,
}

impl Who {
    pub const ALL: [Who; 3] = [Who::Owner, Who::Agent, Who::Automation];

    pub const fn of(actor: Actor) -> Who {
        match actor {
            Actor::Owner => Who::Owner,
            Actor::Agent | Actor::Reviewer | Actor::Assistant => Who::Agent,
            Actor::Auto | Actor::Extension => Who::Automation,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Who::Owner => "Owner",
            Who::Agent => "Agents",
            Who::Automation => "Automation",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Who::Owner => "owner",
            Who::Agent => "agent",
            Who::Automation => "automation",
        }
    }
}

/// An event that should not be read past, named in words as well as colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Blocked,
    Failed,
    Forced,
}

impl Alert {
    pub fn of(event: &Event) -> Option<Alert> {
        match event.kind {
            EventKind::Blocked => Some(Alert::Blocked),
            EventKind::Forced => Some(Alert::Forced),
            EventKind::Error | EventKind::Gate if failed(event) => Some(Alert::Failed),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Alert::Blocked => "Blocked",
            Alert::Failed => "Failed",
            Alert::Forced => "Forced",
        }
    }
}

/// The Feed's filters on this device; the default shows every loaded event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FeedFilter {
    pub category: Option<Category>,
    pub who: Option<Who>,
    /// Words that must each appear in the task id or the row's visible words.
    pub search: String,
}

impl FeedFilter {
    pub fn active(&self) -> bool {
        self.category.is_some() || self.who.is_some() || !self.search.trim().is_empty()
    }

    /// Whether a row with these facts passes every filter set; `words` are what it shows.
    pub fn matches(&self, category: Category, who: Who, words: &[&str]) -> bool {
        if self.category.is_some_and(|wanted| wanted != category)
            || self.who.is_some_and(|wanted| wanted != who)
        {
            return false;
        }
        let haystack = words.join(" ").to_lowercase();
        self.search
            .split_whitespace()
            .all(|token| haystack.contains(&token.to_lowercase()))
    }
}
