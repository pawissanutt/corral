//! Tasks' Done and Dropped: one fold at the bottom of the list, read from corrald a served page
//! at a time (`DetailRef::Closed`), newest first, with Load older under it. corrald lists the
//! two together and counts them together, so they share the fold; each row carries its own
//! status mark. The board never carries closed tasks, so nothing here is projected from it; a
//! closed task the open list still happens to hold is listed once, with the served ones.

use corral_protocol::app::CLOSED_V1;
use corral_protocol::detail::{ClosedPage, DetailBody, DetailRef};

use crate::model::Id;
use crate::model::daemon::{Feed, Link};
use crate::model::list::Section;
use crate::model::projects::Scope;
use crate::model::status::folds;

use super::Filter;

/// The fold's section key (`section-closed`).
pub const KEY: &str = "closed";
pub const LABEL: &str = "Done and Dropped";
/// Load older's words.
pub const OLDER: &str = "Load older";

/// Whether the fold is offered: once a complete board arrived for an enrolled device, even an
/// empty one, since closed history is what an empty board has left.
pub fn offered(feed: &Feed) -> bool {
    feed.board.complete && feed.device.board_notice().is_none()
}

/// The served pages the fold shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Closed {
    /// The served `older` anchors of the pages loaded after the newest, oldest last.
    pub older: Vec<String>,
}

impl Closed {
    /// Every page the fold reads, newest first: the newest always (its total is the fold's
    /// count), the older ones while loaded.
    pub fn targets(&self) -> Vec<DetailRef> {
        std::iter::once(None)
            .chain(self.older.iter().cloned().map(Some))
            .map(|before| DetailRef::Closed { before })
            .collect()
    }

    /// Loads the page after the last one shown, when corrald named one.
    pub fn more(&mut self, feed: &Feed) {
        let last = self.targets().pop().and_then(|target| {
            page(feed, &target)
                .and_then(|page| page.older.clone())
                .filter(|anchor| !self.older.contains(anchor))
        });
        self.older.extend(last);
    }

    /// Back to the newest page alone, so the client may drop the older ones.
    pub fn fold(&mut self) {
        self.older.clear();
    }
}

fn page<'a>(feed: &'a Feed, target: &DetailRef) -> Option<&'a ClosedPage> {
    match feed.detail(target)?.body()? {
        DetailBody::Closed(page) => Some(page),
        _ => None,
    }
}

/// The closed tasks the loaded pages hold, newest first.
fn served<'a>(feed: &'a Feed, closed: &Closed) -> Vec<&'a corral_types::Task> {
    closed
        .targets()
        .iter()
        .filter_map(|target| page(feed, target))
        .flat_map(|page| page.rows.iter())
        .collect()
}

/// A closed task from any page the client holds, whichever list read it.
pub fn find<'a>(feed: &'a Feed, id: &str) -> Option<&'a corral_types::Task> {
    feed.details
        .values()
        .filter_map(|cache| match cache.body() {
            Some(DetailBody::Closed(page)) => Some(page),
            _ => None,
        })
        .flat_map(|page| page.rows.iter())
        .find(|task| task.id == id)
}

/// The fold: its rows while open, its count either way. The count is corrald's total while
/// every closed task could be listed (all projects, no filter words), else the rows that match.
/// None until something is known to count, unless corrald names an older page: a project or
/// filter words that match nothing loaded yet may match there, and the fold holds Load older.
pub fn section(
    feed: &Feed,
    scope: &Scope,
    filter: &Filter,
    closed: &Closed,
    open: bool,
) -> Option<Section> {
    let mut rows: Vec<Id> = Vec::new();
    let board = feed
        .board
        .tasks()
        .iter()
        .filter(|task| folds(task.status()))
        .map(|task| task.task());
    // A filed plan parent lists among the plans whatever its stored status.
    let summary = |id: &str| feed.board.task(id).is_some_and(super::plans::is_summary);
    for task in board.chain(served(feed, closed)) {
        let id = Id::from(task.id.as_str());
        if scope.matches(task.project.as_deref())
            && filter.matches(task)
            && !summary(&task.id)
            && !rows.contains(&id)
        {
            rows.push(id);
        }
    }
    let total = page(feed, &DetailRef::Closed { before: None })
        .and_then(|page| page.total)
        .and_then(|total| usize::try_from(total).ok());
    let count = match total {
        Some(total) if matches!(scope, Scope::All) && filter.is_empty() => total.max(rows.len()),
        _ => rows.len(),
    };
    let shown = rows.len();
    let older = closed
        .targets()
        .last()
        .and_then(|target| page(feed, target))
        .is_some_and(|page| page.older.is_some());
    let section = Section {
        key: KEY.to_owned(),
        label: LABEL.to_owned(),
        look: None,
        count,
        note: if open && shown == 0 && older {
            Some("none of the loaded match".to_owned())
        } else {
            (open && count > shown).then(|| format!("newest {shown} loaded"))
        },
        fold: Some(open),
        rows: if open { rows } else { Vec::new() },
    };
    (section.shown() || older).then_some(section)
}

/// What stands under the open fold: why nothing is listed, Load older, or Retry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Foot {
    pub note: Option<String>,
    /// Load older, while corrald names an older page.
    pub older: bool,
    /// Retry, after a page failed.
    pub retry: bool,
}

/// The open fold's foot; none while folded or with nothing to say.
pub fn foot(feed: &Feed, closed: &Closed, open: bool) -> Option<Foot> {
    if !open {
        return None;
    }
    let mut foot = Foot::default();
    if !feed.capable(CLOSED_V1) {
        foot.note = Some(if feed.link == Link::Down {
            "corrald is not reachable; closed tasks load once a session with it is open.".into()
        } else {
            "This corrald does not list closed tasks to the app (closed.v1 is not advertised)."
                .into()
        });
        return Some(foot);
    }
    let targets = closed.targets();
    let failed = targets
        .iter()
        .filter_map(|target| feed.detail(target))
        .find_map(|cache| cache.error.as_ref());
    let last = targets.last().and_then(|target| feed.detail(target));
    if let Some(error) = failed {
        foot.note = Some(format!("Closed tasks unavailable: {}", error.message));
        foot.retry = true;
    } else if last.is_none_or(|cache| cache.body().is_none()) {
        foot.note = Some("Loading closed tasks…".into());
    } else {
        foot.older = targets
            .last()
            .and_then(|target| page(feed, target))
            .is_some_and(|page| page.older.is_some());
        if !foot.older && served(feed, closed).is_empty() {
            foot.note = Some("No closed tasks.".into());
        }
    }
    (foot != Foot::default()).then_some(foot)
}

#[cfg(test)]
mod tests {
    use corral_types::Status;

    use super::*;
    use crate::model::daemon::{DetailCache, bare_task, detail_key};

    fn serve(feed: &mut Feed, before: Option<&str>, ids: &[&str], older: Option<&str>) {
        let target = DetailRef::Closed {
            before: before.map(str::to_owned),
        };
        let rows = ids
            .iter()
            .map(|id| {
                let mut task = bare_task(id, id, Status::Done, jiff::Timestamp::UNIX_EPOCH);
                task.project = Some("site".into());
                task
            })
            .collect();
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                transcript_seams: Default::default(),
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Closed(ClosedPage {
                    total: Some(40),
                    rows,
                    older: older.map(str::to_owned),
                })),
            },
        );
    }

    /// The fold once counted only the rows on screen, so 40 closed tasks read as 2; and Load
    /// older asked for the same page again once it was loaded.
    #[test]
    fn the_fold_counts_corralds_total_and_load_older_walks_the_anchors() {
        let mut feed = Feed::default();
        feed.capabilities.insert(CLOSED_V1.into());
        serve(&mut feed, None, &["T-9", "T-8"], Some("a1"));
        serve(&mut feed, Some("a1"), &["T-7"], None);
        let mut closed = Closed::default();
        let all =
            |closed: &Closed, open| section(&feed, &Scope::All, &Filter::default(), closed, open);
        let folded = all(&closed, false).expect("a fold with a count");
        assert_eq!((folded.count, folded.rows.len()), (40, 0));
        assert_eq!(all(&closed, true).map(|s| s.rows.len()), Some(2));
        assert!(foot(&feed, &closed, true).is_some_and(|foot| foot.older));
        closed.more(&feed);
        closed.more(&feed);
        assert_eq!(closed.older, ["a1"]);
        assert_eq!(all(&closed, true).map(|s| s.rows.len()), Some(3));
        assert_eq!(foot(&feed, &closed, true), None);
        // Under one project, or with filter words, the total says nothing about what matches:
        // the rows that match are counted.
        let other = Scope::Project("docs".into());
        assert_eq!(
            section(&feed, &other, &Filter::default(), &closed, true),
            None
        );
        let words = Filter::parse("t-7");
        let found = section(&feed, &Scope::All, &words, &closed, true);
        assert_eq!(found.map(|s| s.count), Some(1));
    }

    /// Filter words that matched nothing on the newest page hid the fold, and with it Load
    /// older, so an older matching task could never be reached.
    #[test]
    fn the_fold_stays_while_an_older_page_may_hold_a_match() {
        let mut feed = Feed::default();
        feed.capabilities.insert(CLOSED_V1.into());
        serve(&mut feed, None, &["T-9", "T-8"], Some("a1"));
        serve(&mut feed, Some("a1"), &["T-7"], None);
        let mut closed = Closed::default();
        let words = Filter::parse("t-7");
        let first = section(&feed, &Scope::All, &words, &closed, true);
        assert_eq!(first.map(|s| s.rows.len()), Some(0));
        assert!(foot(&feed, &closed, true).is_some_and(|foot| foot.older));
        closed.more(&feed);
        let found = section(&feed, &Scope::All, &words, &closed, true);
        assert_eq!(found.map(|s| s.rows.len()), Some(1));
    }
}
