//! A diagram comparison's link names the submission it read. Changes shows that submission or
//! says it no longer can; it never shows another revision under the link's name. The check is
//! made where Changes draws, against the very detail it draws from.

use corral_protocol::detail::{DetailBody, DetailRef};

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
    /// Changes has not been served a submission yet.
    Waiting,
    Same,
    /// Changes holds another submission; why it is not shown.
    Other(String),
}

/// The submission Changes draws in `view`: the review's run for the diff, the System review's
/// head for Impact.
pub fn served(feed: &Feed, task: &str, view: View) -> Option<String> {
    let target = match view {
        View::Diff => DetailRef::Review {
            task: task.to_owned(),
        },
        View::Impact => super::request(task, None),
    };
    match feed.detail(&target).and_then(DetailCache::body)? {
        DetailBody::Review(review) => review.run.as_ref()?.head_sha.clone(),
        DetailBody::SystemReview(review) => Some(review.head.clone()),
        _ => None,
    }
}

pub fn verdict(feed: &Feed, task: &str, view: View, expected: &Expected) -> Verdict {
    match served(feed, task, view) {
        None => Verdict::Waiting,
        Some(head) if head == expected.head => Verdict::Same,
        Some(head) => Verdict::Other(format!(
            "This link is for {task}'s submission {}, which the diagram comparison read. {task}'s Changes now hold submission {}, so they are not shown under this link. Compare again with Task candidate to read the current one.",
            short(&expected.head),
            short(&head)
        )),
    }
}

/// The note over Changes while they show the linked submission.
pub fn shown_note(expected: &Expected) -> String {
    format!(
        "Showing submission {}, as linked from a diagram comparison.",
        short(&expected.head)
    )
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}
