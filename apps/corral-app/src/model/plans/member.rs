//! One child of a plan as every Plans part reads it: the child task the plan detail serves,
//! and its open board row when the board carries it (fresher, with its run, checks and group).

use corral_types::{Blocked, Status};

use crate::model::board::{Actor, Mark, Rail, daemon_actor};
use crate::model::daemon::{self, Feed, Group};
use crate::model::glance::StageRail;
use crate::model::hue::Hue;

pub(super) struct Member<'a> {
    pub child: &'a corral_types::Task,
    pub row: Option<&'a daemon::Task>,
}

impl<'a> Member<'a> {
    pub fn of(feed: &'a Feed, child: &'a corral_types::Task) -> Member<'a> {
        Member {
            row: feed.board.task(&child.id),
            child,
        }
    }

    /// The task as fresh as this device knows it: the board's row, else the plan's copy.
    pub fn task(&self) -> &'a corral_types::Task {
        self.row.map_or(self.child, |row| row.task())
    }

    pub fn id(&self) -> &'a str {
        &self.child.id
    }

    pub fn title(&self) -> &'a str {
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

    pub fn blocked(&self) -> Option<&'a Blocked> {
        self.task().blocked.as_ref()
    }

    pub fn group(&self) -> Option<Group> {
        self.row.map(|row| row.group)
    }

    pub fn needs_you(&self) -> bool {
        self.group() == Some(Group::NeedsYou)
    }

    /// Run, gate, review, land: the board's rail, else what the status alone says.
    pub fn marks(&self) -> Rail {
        if let Some(row) = self.row {
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
