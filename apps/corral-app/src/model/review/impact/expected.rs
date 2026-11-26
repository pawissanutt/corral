//! A diagram comparison's link names the submission it read. Changes show that submission or say
//! they cannot; they never show another revision under the link's name. The check is made where
//! Changes draw, against every detail they draw from: the review (or System review) that names
//! the submission, each expanded file's patch page and each drilled Impact level.

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_types::SystemAt;

use super::View;
use crate::model::daemon::{DetailCache, Feed};

/// The submission a link expects, and the file its diff should open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expected {
    pub head: String,
    /// Expanded once the expected submission shows and the task is loaded.
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Changes have not been served a submission yet.
    Waiting,
    Same,
    /// Changes hold another submission, or none can be confirmed; why it is not shown.
    Other(String),
}

/// What a linked Changes view draws besides its root detail.
#[derive(Debug, Clone, Copy, Default)]
pub struct Drawn<'a> {
    /// Expanded files of the diff, each read as its own patch page.
    pub patches: &'a [String],
    /// Drilled Impact levels, each read as its own System review.
    pub levels: &'a [SystemAt],
}

/// The detail that names the submission Changes draw in `view`.
pub fn root(task: &str, view: View) -> DetailRef {
    match view {
        View::Diff => DetailRef::Review {
            task: task.to_owned(),
        },
        View::Impact => super::request(task, None),
    }
}

/// The submission a served detail names: a review's run, a System review's head, a patch page's.
fn head_of(body: &DetailBody) -> Option<Option<&str>> {
    match body {
        DetailBody::Review(review) => Some(review.run.as_ref().and_then(|r| r.head_sha.as_deref())),
        DetailBody::SystemReview(review) => Some(Some(review.head.as_str())),
        DetailBody::ReviewFile(page) => Some(page.head.as_deref()),
        _ => None,
    }
}

pub fn verdict(
    feed: &Feed,
    task: &str,
    view: View,
    expected: &Expected,
    drawn: Drawn<'_>,
) -> Verdict {
    let other = |what: &str, head: Option<&str>| {
        Verdict::Other(match head {
            Some(head) => format!(
                "This link is for {task}'s submission {}, which the diagram comparison read. {what} now hold submission {}, so they are not shown under this link. Compare again with Task candidate to read the current one.",
                short(&expected.head),
                short(head)
            ),
            None => format!(
                "This link is for {task}'s submission {}, but {what} name no submission, so they are not shown under this link.",
                short(&expected.head)
            ),
        })
    };
    let changes = format!("{task}'s Changes");
    let Some(cache) = feed.detail(&root(task, view)) else {
        return Verdict::Waiting;
    };
    if let Some(error) = &cache.error {
        return Verdict::Other(format!(
            "Corral could not confirm which submission {task}'s Changes hold ({}), so they are not shown under this link.",
            error.message
        ));
    }
    let Some(head) = cache.body().and_then(head_of) else {
        return Verdict::Waiting;
    };
    if head != Some(expected.head.as_str()) {
        return other(&changes, head);
    }
    // Each patch page and drilled level is read on its own and may answer for another
    // submission than the root; any that does withholds the whole view.
    let parts = drawn
        .patches
        .iter()
        .map(|path| {
            (
                format!("{task}'s patch for {path}"),
                DetailRef::ReviewFile {
                    task: task.to_owned(),
                    path: path.clone(),
                },
            )
        })
        .filter(|_| view == View::Diff)
        .chain(
            drawn
                .levels
                .iter()
                .map(|at| {
                    (
                        format!("{task}'s Impact level"),
                        super::request(task, Some(at)),
                    )
                })
                .filter(|_| view == View::Impact),
        );
    for (what, target) in parts {
        let head = feed
            .detail(&target)
            .and_then(DetailCache::body)
            .and_then(head_of);
        if let Some(head) = head
            && head != Some(expected.head.as_str())
        {
            return other(&what, head);
        }
    }
    Verdict::Same
}

/// The worker's words for the linked submission: the run of the review the verdict checked.
pub fn summary(feed: &Feed, task: &str) -> Option<String> {
    match feed
        .detail(&root(task, View::Diff))
        .and_then(DetailCache::body)?
    {
        DetailBody::Review(review) => review.run.as_ref()?.summary.clone(),
        _ => None,
    }
}

/// The note over Changes while they show, or wait to show, the linked submission.
pub fn note(expected: &Expected, verdict: &Verdict) -> String {
    match verdict {
        Verdict::Waiting => format!(
            "Checking that Changes hold submission {}, as linked from a diagram comparison…",
            short(&expected.head)
        ),
        _ => format!(
            "Showing submission {}, as linked from a diagram comparison.",
            short(&expected.head)
        ),
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}
