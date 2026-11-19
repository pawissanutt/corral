//! One child of a plan as every Plans part reads it: the child task the plan detail serves, and
//! its full row: the open board's when the board carries it (the freshest), else the row the
//! plan detail serves for it (`child_rows`, which also covers Done and Dropped children), with
//! its run, checks, group and rail.

use std::borrow::Cow;

use corral_protocol::detail::PlanDetail;
use corral_types::{Blocked, Status};

use crate::model::board::{Actor, Mark, Rail, daemon_actor};
use crate::model::daemon::{self, Feed, Group};
use crate::model::glance::StageRail;
use crate::model::hue::Hue;

pub(super) struct Member<'a> {
    pub child: &'a corral_types::Task,
    pub row: Option<Cow<'a, daemon::Task>>,
}

impl<'a> Member<'a> {
    pub fn of(feed: &'a Feed, detail: &'a PlanDetail, child: &'a corral_types::Task) -> Self {
        let row = feed.board.task(&child.id).map(Cow::Borrowed).or_else(|| {
            detail
                .child_rows
                .as_ref()?
                .iter()
                .find(|row| row.task.id == child.id)
                .map(|row| Cow::Owned(daemon::Task::new(row.clone())))
        });
        Member { child, row }
    }

    /// Every child of the plan, in the order corrald serves them.
    pub fn all(feed: &'a Feed, detail: &'a PlanDetail) -> Vec<Self> {
        detail
            .children
            .iter()
            .map(|child| Member::of(feed, detail, child))
            .collect()
    }

    /// The task as fresh as this device knows it: the served row's, else the plan's copy.
    pub fn task(&self) -> &corral_types::Task {
        self.row.as_deref().map_or(self.child, |row| row.task())
    }

    pub fn id(&self) -> &'a str {
        &self.child.id
    }

    pub fn title(&self) -> &str {
        &self.task().title
    }

    pub fn status(&self) -> Status {
        self.task().status
    }

    pub fn done(&self) -> bool {
        self.status() == Status::Done
    }

    pub fn dropped(&self) -> bool {
        self.status() == Status::Dropped
    }

    /// Still between the plan and its completion: neither Done nor Dropped.
    pub fn open(&self) -> bool {
        !self.done() && !self.dropped()
    }

    pub fn blocked(&self) -> Option<&Blocked> {
        self.task().blocked.as_ref()
    }

    /// Who moves the member next; none once it is closed.
    pub fn group(&self) -> Option<Group> {
        self.row
            .as_deref()
            .map(|row| row.group)
            .filter(|group| *group != Group::Other)
    }

    pub fn needs_you(&self) -> bool {
        self.group() == Some(Group::NeedsYou)
    }

    /// Run, gate, review, land: the served row's rail, else what the status alone says. A
    /// dropped member has no stage left, whatever its row recorded.
    pub fn marks(&self) -> Rail {
        if let Some(row) = self.row.as_deref().filter(|_| !self.dropped()) {
            return std::array::from_fn(|index| row.rail[index].mark);
        }
        match self.status() {
            Status::Done => [Mark::Done; 4],
            Status::Dropped => [Mark::Na; 4],
            _ => [Mark::Next; 4],
        }
    }

    pub fn actor(&self) -> Actor {
        match (self.group(), self.status()) {
            (Some(group), _) => daemon_actor(group),
            (None, Status::Done) => Actor::Done,
            (None, Status::Dropped) => Actor::Parked,
            (None, _) => Actor::Queued,
        }
    }

    /// One line about where the member stands.
    pub fn now(&self) -> String {
        self.row
            .as_deref()
            .map_or_else(|| self.status().as_str().to_owned(), |row| row.now())
    }

    pub fn rail(&self) -> StageRail {
        StageRail::of(self.marks(), self.actor(), &self.now())
    }

    /// The member's mark and hue in a plan's member bar and path.
    pub fn segment(&self) -> (Mark, Hue) {
        if self.done() {
            return (Mark::Done, Hue::Ok);
        }
        if self.dropped() {
            return (Mark::Na, Hue::Neutral);
        }
        if self.needs_you() {
            return (Mark::Active, Hue::You);
        }
        match self.group() {
            Some(Group::Producing) => (Mark::Active, Hue::Worker),
            Some(Group::Verifying) => (Mark::Active, Hue::Corral),
            _ if self.marks().contains(&Mark::Fail) => (Mark::Fail, Hue::Held),
            _ => (Mark::Next, Hue::Neutral),
        }
    }

    pub fn moving(&self) -> bool {
        matches!(self.segment(), (Mark::Active, Hue::Worker | Hue::Corral))
    }
}
