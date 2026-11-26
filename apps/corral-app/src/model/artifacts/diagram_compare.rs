//! Comparing a workflow recipe's evidence with a later commit (`artifact.diagram.compare.v1`).
//! The artifact stays pinned at its own commit. The owner chooses the project's observed head or
//! the task's candidate; corrald resolves it once, and every later read names that exact commit,
//! so a moving head never changes a comparison mid-read. What changed is corrald's answer for
//! both commits; the app claims nothing corrald did not.

use corral_protocol::app::ARTIFACT_DIAGRAM_COMPARE_V1;
use corral_protocol::detail::{
    CompareLink, CompareOrigin, CompareResolution, CompareTarget, DetailBody, DetailRef,
    DiagramCompareDetail, EntityChange, EntityLocation, SourceChange,
};

use super::diagram::{Graph, related, short, source};
use crate::model::daemon::Feed;
use crate::model::system::lens::Source;

pub const COMPARE_HEAD: &str = "Compare";
pub const COMPARE_NOTE: &str =
    "The artifact keeps its pinned commit; comparing reads both commits and changes nothing.";
pub const COMPARE_CHOICES: [(Option<CompareOrigin>, &str); 3] = [
    (None, "Off"),
    (Some(CompareOrigin::ObservedHead), "Project head"),
    (Some(CompareOrigin::Candidate), "Task candidate"),
];

/// A comparison chosen for one artifact, and the commit corrald resolved for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compare {
    pub reference: String,
    pub origin: CompareOrigin,
    /// Pinned from corrald's first answer; every later read names it.
    pub commit: Option<String>,
}

/// Whether corrald can compare.
pub fn capable(feed: &Feed) -> bool {
    feed.capable(ARTIFACT_DIAGRAM_COMPARE_V1)
}

/// The read for a comparison, pinned once corrald resolved it; for one element or the summary.
pub fn target(
    reference: &str,
    sha256: &str,
    compare: &Compare,
    element: Option<&str>,
) -> DetailRef {
    DetailRef::ArtifactDiagramCompare {
        reference: reference.to_owned(),
        sha256: sha256.to_owned(),
        target: match (&compare.commit, compare.origin) {
            (Some(commit), origin) => CompareTarget::Pinned {
                origin,
                commit: commit.clone(),
            },
            (None, CompareOrigin::ObservedHead) => CompareTarget::ObservedHead,
            (None, CompareOrigin::Candidate) => CompareTarget::Candidate,
        },
        element: element.map(str::to_owned),
    }
}

/// How an element's bound source changed, as its graph card and its row mark it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Changed,
    Moved,
    Removed,
    Added,
    /// Not compared: one side could not be read.
    Unknown,
}

impl Mark {
    pub fn word(self) -> &'static str {
        match self {
            Mark::Changed => "changed",
            Mark::Moved => "moved",
            Mark::Removed => "removed",
            Mark::Added => "added",
            Mark::Unknown => "not compared",
        }
    }

    /// The viewer's state name.
    pub fn state(self) -> &'static str {
        match self {
            Mark::Unknown => "unknown",
            mark => mark.word(),
        }
    }

    fn of(change: &SourceChange) -> Option<Mark> {
        match change {
            SourceChange::NotProvided | SourceChange::Unchanged => None,
            SourceChange::Unknown { .. } => Some(Mark::Unknown),
            SourceChange::Moved { .. } => Some(Mark::Moved),
            SourceChange::Changed { .. } => Some(Mark::Changed),
            SourceChange::Removed => Some(Mark::Removed),
            SourceChange::Added => Some(Mark::Added),
        }
    }
}

/// A link to the candidate's own changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Diff {
        task: String,
        head: String,
        path: String,
    },
    Impact {
        task: String,
        head: String,
    },
}

impl Link {
    /// Names the submission it was served for, so the owner can tell it from a later one.
    pub fn label(&self) -> String {
        match self {
            Link::Diff { task, head, path } => {
                format!("{task} diff of {} · {path}", short(head))
            }
            Link::Impact { task, head } => format!("{task} Impact of {}", short(head)),
        }
    }
}

/// The selected element at the comparison commit.
#[derive(Debug, Clone, PartialEq)]
pub enum Side {
    Loading,
    Failed(String),
    Ready {
        /// What happened to the bound source.
        change: String,
        /// The lines at the comparison commit, or why none show. The pinned commit's lines
        /// stay in the inspector's Source above, removed or not.
        source: Result<Source, String>,
        /// What changed in Corral's derived facts, or why it cannot say.
        derived: Vec<String>,
        links: Vec<Link>,
        /// Facts for a commit are still being prepared; the read is asked again.
        pending: bool,
    },
}

/// A comparison as the diagram shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub origin: CompareOrigin,
    /// Which commit, pinned for this view, or why there is none.
    pub status: String,
    /// Counts of what changed, when the comparison resolved.
    pub summary: Option<String>,
    /// A resolved commit to pin, while the comparison is not pinned yet.
    pub pin: Option<String>,
    /// Ask the summary again: corrald is still observing the head.
    pub pending: bool,
    pub marks: Vec<(String, Mark)>,
    pub side: Option<Side>,
    /// What to keep loaded: the summary, then the selected element's read.
    pub reads: Vec<DetailRef>,
}

fn body<'a>(
    feed: &'a Feed,
    target: &DetailRef,
) -> Option<Result<&'a DiagramCompareDetail, String>> {
    let cache = feed.detail(target)?;
    if let Some(error) = &cache.error {
        return Some(Err(error.message.clone()));
    }
    match cache.body() {
        Some(DetailBody::ArtifactDiagramCompare(detail)) => Some(Ok(detail)),
        _ => None,
    }
}

fn fresh(feed: &Feed, target: &DetailRef) -> bool {
    feed.detail(target)
        .is_some_and(|cache| !cache.loading && !cache.stale && cache.error.is_none())
}

/// The comparison of `graph` for its chosen target.
pub fn view(feed: &Feed, graph: &Graph, compare: &Compare) -> View {
    let summary_read = target(&graph.reference, &graph.sha256, compare, None);
    let mut shown = View {
        origin: compare.origin,
        status: "Resolving the comparison commit…".into(),
        summary: None,
        pin: None,
        pending: false,
        marks: Vec::new(),
        side: None,
        reads: vec![summary_read.clone()],
    };
    match body(feed, &summary_read) {
        None => {}
        Some(Err(why)) => shown.status = format!("corrald could not compare: {why}"),
        Some(Ok(detail)) => {
            shown.status = status(&detail.target);
            match &detail.target {
                CompareResolution::Pending => shown.pending = true,
                // Only an answer read for this choice pins: never a kept or revalidating one.
                CompareResolution::Resolved { commit, .. }
                    if compare.commit.is_none() && fresh(feed, &summary_read) =>
                {
                    shown.pin = Some(commit.clone());
                }
                _ => {}
            }
            shown.marks = detail
                .elements
                .iter()
                .filter_map(|element| Some((element.id.clone(), Mark::of(&element.change)?)))
                .collect();
            if matches!(detail.target, CompareResolution::Resolved { .. }) {
                shown.summary = Some(counts(&shown.marks, detail.elements.len()));
            }
        }
    }
    // An element is read only at a pinned commit, never at a head still moving.
    if compare.commit.is_some()
        && let Some(id) = graph.selected.as_deref()
    {
        let read = target(&graph.reference, &graph.sha256, compare, Some(id));
        shown.side = Some(match body(feed, &read) {
            None => Side::Loading,
            Some(Err(why)) => {
                Side::Failed(format!("corrald could not compare this element: {why}"))
            }
            Some(Ok(detail)) => side(detail),
        });
        shown.reads.push(read);
    }
    shown
}

fn status(target: &CompareResolution) -> String {
    match target {
        CompareResolution::Pending => "Waiting for corrald to observe the project head…".into(),
        CompareResolution::Unavailable { reason } => format!("Cannot compare: {reason}."),
        CompareResolution::Resolved {
            origin,
            commit,
            candidate,
            current_candidate,
        } => {
            let what = match (origin, candidate) {
                (CompareOrigin::Candidate, Some(c)) => {
                    format!("{} {} submission {}", c.task, c.run, short(&c.head))
                }
                _ => format!("the project head {}", short(commit)),
            };
            let stale = if *origin == CompareOrigin::Candidate && !current_candidate {
                " A newer submission exists; its diff and Impact describe that one, so they are not linked."
            } else {
                ""
            };
            format!("Compared with {what}, pinned for this view.{stale}")
        }
    }
}

fn counts(marks: &[(String, Mark)], bound: usize) -> String {
    let count = |mark| marks.iter().filter(|(_, m)| *m == mark).count();
    let parts: Vec<String> = [
        Mark::Changed,
        Mark::Moved,
        Mark::Removed,
        Mark::Added,
        Mark::Unknown,
    ]
    .into_iter()
    .filter_map(|mark| {
        let n = count(mark);
        (n > 0).then(|| format!("{n} {}", mark.word()))
    })
    .collect();
    if parts.is_empty() {
        format!("The source of all {bound} bound elements is unchanged.")
    } else {
        format!(
            "Of {bound} elements with bound source: {}.",
            parts.join(", ")
        )
    }
}

fn change_words(change: &SourceChange) -> String {
    match change {
        SourceChange::NotProvided => "The recipe binds no source to this element.".into(),
        SourceChange::Unknown { reason } => format!("Not compared: {reason}."),
        SourceChange::Unchanged => "The bound source is the same at both commits.".into(),
        SourceChange::Moved {
            path,
            first: Some(line),
        } => format!("The same lines are now at {path}:{line}."),
        SourceChange::Moved { path, first: None } => {
            format!("The same file is now at {path}.")
        }
        SourceChange::Changed { path } => format!(
            "The bound source differs in {path}; below are the lines Git's diff places it at, if any."
        ),
        SourceChange::Removed => {
            "The bound file is gone at the comparison commit; Source above still shows it at the pinned commit."
                .into()
        }
        SourceChange::Added => {
            "The bound file exists only at the comparison commit, not at the pinned one.".into()
        }
    }
}

fn lines(location: &EntityLocation) -> String {
    match location {
        EntityLocation::File => "the file".into(),
        EntityLocation::Definition { line, end_line } => format!("lines {line}–{end_line}"),
    }
}

fn derived(change: &EntityChange) -> (Vec<String>, bool) {
    let one = |line: String| (vec![line], false);
    match change {
        EntityChange::NotProvided => {
            one("The recipe binds no entity, so nothing was compared.".into())
        }
        EntityChange::Pending => (
            vec!["Corral is preparing facts for a commit; asking again shortly.".into()],
            true,
        ),
        EntityChange::Insufficient { reason } => one(format!(
            "Corral cannot say whether its derived facts changed: {reason}."
        )),
        EntityChange::Unchanged => one(
            "Corral's derived facts are the same at both commits, within complete coverage.".into(),
        ),
        EntityChange::Removed { reason } => {
            one(format!("Not found at the comparison commit: {reason}."))
        }
        EntityChange::Changed {
            location,
            added,
            removed,
        } => {
            let mut out = Vec::new();
            if let Some(moved) = location {
                out.push(format!(
                    "Definition moved from {} to {}.",
                    lines(&moved.base),
                    lines(&moved.target)
                ));
            }
            out.extend(added.iter().map(|row| format!("New: {}", related(row))));
            out.extend(removed.iter().map(|row| format!("Gone: {}", related(row))));
            (out, false)
        }
    }
}

fn side(detail: &DiagramCompareDetail) -> Side {
    let Some(selected) = &detail.selected else {
        return Side::Failed(status(&detail.target));
    };
    let (derived, pending) = derived(&selected.entity_change);
    Side::Ready {
        change: change_words(&selected.source_change),
        source: match selected.source_change {
            SourceChange::NotProvided => Err("No source is bound.".into()),
            _ => source(&selected.target_source),
        },
        derived,
        links: selected
            .links
            .iter()
            .map(|link| match link {
                CompareLink::Diff { task, head, path } => Link::Diff {
                    task: task.clone(),
                    head: head.clone(),
                    path: path.clone(),
                },
                CompareLink::Impact { task, head } => Link::Impact {
                    task: task.clone(),
                    head: head.clone(),
                },
            })
            .collect(),
        pending,
    }
}

/// The comparison's mark beside a row's words, when it has one.
pub fn mark_of(view: &View, id: &str) -> Option<Mark> {
    view.marks
        .iter()
        .find(|(at, _)| at == id)
        .map(|(_, mark)| *mark)
}

/// The message that tells the viewer each element's mark; none clears them.
pub fn marks_message(view: Option<&View>) -> String {
    let marks: serde_json::Map<String, serde_json::Value> = view
        .map(|view| {
            view.marks
                .iter()
                .map(|(id, mark)| (id.clone(), mark.state().into()))
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({"v": super::diagram::BRIDGE_VERSION, "marks": marks}).to_string()
}

/// Whether a read of the comparison is pending, so it is asked again.
pub fn retry(view: &View) -> Vec<DetailRef> {
    let mut again = Vec::new();
    if view.pending {
        again.extend(view.reads.first().cloned());
    }
    if matches!(view.side, Some(Side::Ready { pending: true, .. })) {
        again.extend(view.reads.get(1).cloned());
    }
    again
}
