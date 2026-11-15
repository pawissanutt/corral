//! Live records the app can open, by their owned daemon ids, and the selection every form shares.

use corral_protocol::assistant::ContextRef;
use corral_protocol::detail::DetailRef;

use crate::model::id::Id;
use crate::model::nav::{Destination, Form};

/// The kind of a live record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveKind {
    Question,
    Task,
    Plan,
    Session,
    Thread,
    Review,
}

impl LiveKind {
    /// The kind as `ContextRef.kind` and palette ids name it.
    pub const fn slug(self) -> &'static str {
        match self {
            LiveKind::Question => "question",
            LiveKind::Task => "task",
            LiveKind::Plan => "plan",
            LiveKind::Session => "session",
            LiveKind::Thread => "thread",
            LiveKind::Review => "review",
        }
    }
}

/// A daemon record to open: a question by key, a task, a plan, an agent session, an assistant
/// thread ("quick" or A-n), or the candidate of a task under review.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Live {
    Question(Id),
    Task(Id),
    Plan(Id),
    Session(Id),
    Thread(Id),
    Review(Id),
}

impl Live {
    pub fn kind(&self) -> LiveKind {
        match self {
            Live::Question(_) => LiveKind::Question,
            Live::Task(_) => LiveKind::Task,
            Live::Plan(_) => LiveKind::Plan,
            Live::Session(_) => LiveKind::Session,
            Live::Thread(_) => LiveKind::Thread,
            Live::Review(_) => LiveKind::Review,
        }
    }

    pub fn id(&self) -> &Id {
        match self {
            Live::Question(id)
            | Live::Task(id)
            | Live::Plan(id)
            | Live::Session(id)
            | Live::Thread(id)
            | Live::Review(id) => id,
        }
    }

    /// The destination the record belongs to.
    pub fn owner(&self) -> Destination {
        match self {
            Live::Question(_) => Destination::Inbox,
            Live::Task(_) => Destination::Now,
            Live::Plan(_) => Destination::Plans,
            Live::Session(_) | Live::Thread(_) => Destination::Agents,
            Live::Review(_) => Destination::Review,
        }
    }

    /// Whether opening it pushes a page on this form; otherwise it is selected in place.
    pub fn pushes(&self, form: Form) -> bool {
        form == Form::Phone
            || matches!(self, Live::Question(_) | Live::Session(_) | Live::Thread(_))
    }

    /// The bounded reference Ask attaches for "Ask about this".
    pub fn context(&self, rev: Option<u64>) -> ContextRef {
        ContextRef {
            kind: self.kind().slug().to_owned(),
            id: self.id().to_string(),
            rev,
        }
    }

    /// The detail that shows the record; threads are scoped subscriptions instead.
    pub fn detail(&self) -> Option<DetailRef> {
        let id = self.id().to_string();
        Some(match self {
            Live::Question(_) => DetailRef::Question { key: id },
            Live::Task(_) => DetailRef::Task { id },
            Live::Plan(_) => DetailRef::Plan { id },
            Live::Session(_) => DetailRef::Session { id },
            Live::Review(_) => DetailRef::Review { task: id },
            Live::Thread(_) => return None,
        })
    }
}

/// The live record selected in each domain, shared by every form and the palette.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    pub question: Option<Id>,
    pub task: Option<Id>,
    pub plan: Option<Id>,
    pub session: Option<Id>,
    pub thread: Option<Id>,
    pub review: Option<Id>,
}

impl Selection {
    pub fn get(&self, kind: LiveKind) -> Option<&Id> {
        match kind {
            LiveKind::Question => self.question.as_ref(),
            LiveKind::Task => self.task.as_ref(),
            LiveKind::Plan => self.plan.as_ref(),
            LiveKind::Session => self.session.as_ref(),
            LiveKind::Thread => self.thread.as_ref(),
            LiveKind::Review => self.review.as_ref(),
        }
    }

    /// Selects the record in its own domain, leaving the others' selections alone.
    pub fn set(&mut self, live: &Live) {
        let id = Some(live.id().clone());
        match live.kind() {
            LiveKind::Question => self.question = id,
            LiveKind::Task => self.task = id,
            LiveKind::Plan => self.plan = id,
            LiveKind::Session => self.session = id,
            LiveKind::Thread => self.thread = id,
            LiveKind::Review => self.review = id,
        }
    }
}

/// One canonical record above a destination, for real and synthetic feeds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Route {
    Live(Live),
}

impl Route {
    pub fn owner(&self) -> Destination {
        match self {
            Route::Live(live) => live.owner(),
        }
    }

    /// Task, plan and review pages stay on the stack they were opened from on phone.
    pub fn is_detail(&self) -> bool {
        matches!(
            self,
            Route::Live(Live::Task(_) | Live::Plan(_) | Live::Review(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_records_name_their_owner_detail_and_context() {
        let plan = Live::Plan(Id::from("T-885"));
        assert_eq!(plan.owner(), Destination::Plans);
        assert_eq!(plan.detail(), Some(DetailRef::Plan { id: "T-885".into() }));
        let context = plan.context(Some(3));
        assert_eq!(
            (context.kind.as_str(), context.id.as_str()),
            ("plan", "T-885")
        );
        assert_eq!(Live::Thread(Id::from("quick")).detail(), None);
        assert!(!plan.pushes(Form::Desktop));
        assert!(plan.pushes(Form::Phone));
        assert!(Live::Question(Id::from("T-1/R1/Q1")).pushes(Form::Terminal));
    }

    #[test]
    fn selecting_one_domain_keeps_the_others() {
        let mut selection = Selection::default();
        selection.set(&Live::Task(Id::from("T-1")));
        selection.set(&Live::Plan(Id::from("T-2")));
        assert_eq!(selection.get(LiveKind::Task).map(Id::as_str), Some("T-1"));
        assert_eq!(selection.get(LiveKind::Plan).map(Id::as_str), Some("T-2"));
    }
}
