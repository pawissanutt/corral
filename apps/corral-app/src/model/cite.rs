//! Citations in text the app shows: `T-12` names a task, `T-12:3` one of its artifacts and
//! `T-12:3#section` a section of it. They are found with corral-types' grammar
//! (`references_in`), never a parser of the app's, and resolved against the task details the
//! app reads: a task resolves when corrald serves it, an artifact when that task's detail lists
//! it. Until then a citation is being looked up; once corrald says no, it is not found, stays
//! plain text and opens nothing.

use corral_protocol::detail::DetailRef;
use corral_types::{Artifact, references_in};

use crate::model::artifacts::display_kind;
use crate::model::daemon::Feed;
use crate::model::work::{loaded_task, task_ref};
use crate::model::{Effect, Id, Live};

/// What an unresolved citation says when it is shown or pressed.
pub const NOT_FOUND: &str = "not found";
/// What a citation says while its task's detail is on its way.
pub const LOOKING: &str = "looking up…";
/// The heading over the artifacts a task's body cites.
pub const CITED: &str = "Cited";
/// The URL scheme a citation's inline link carries; it never leaves the app.
pub const SCHEME: &str = "cite:";

/// One citation as written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cite {
    /// As spelled in the text: `T-12:3#4-fact-model`.
    pub text: String,
    pub task: Id,
    /// `T-12:3` for an artifact citation.
    pub artifact: Option<String>,
    pub section: Option<String>,
}

/// The distinct citations of `text`, in order of first appearance.
pub fn cites(text: &str) -> Vec<Cite> {
    references_in(text)
        .into_iter()
        .map(|reference| Cite {
            text: reference.as_str().to_owned(),
            task: Id::from(reference.task()),
            artifact: reference.artifact().map(str::to_owned),
            section: reference.section().map(str::to_owned),
        })
        .collect()
}

/// The distinct citations of several texts, in order of first appearance.
pub fn cites_in<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<Cite> {
    let mut all: Vec<Cite> = Vec::new();
    for cite in texts.into_iter().flat_map(cites) {
        if !all.contains(&cite) {
            all.push(cite);
        }
    }
    all
}

/// The task details that resolve `cites`, each once.
pub fn targets(cites: &[Cite]) -> Vec<DetailRef> {
    let mut targets: Vec<DetailRef> = Vec::new();
    for cite in cites {
        let target = task_ref(&cite.task);
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

/// Where a citation stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Task { title: Option<String> },
    Artifact(Box<Artifact>),
    Looking,
    Missing,
}

/// The cited task's title as the app holds it, or whether corrald said it has none.
fn task(feed: &Feed, id: &str) -> Result<Option<String>, bool> {
    if let Some(row) = feed.board.task(id) {
        return Ok(Some(row.title().to_owned()));
    }
    if let Some(detail) = loaded_task(feed, id) {
        return Ok(Some(detail.task.title.clone()));
    }
    let missing = feed
        .detail(&task_ref(id))
        .is_some_and(|cache| cache.body().is_none() && cache.error.is_some());
    Err(missing)
}

/// The cited artifact among every artifact row the app holds: the task's detail first, then
/// the finder's served pages.
pub fn held(feed: &Feed, reference: &str, task: &str) -> Option<Artifact> {
    if let Some(detail) = loaded_task(feed, task) {
        return detail
            .artifacts
            .iter()
            .find(|row| row.reference() == reference)
            .cloned();
    }
    feed.details
        .values()
        .flat_map(|cache| cache.bodies())
        .find_map(|body| match body {
            corral_protocol::detail::DetailBody::Artifacts(page) => page
                .rows
                .iter()
                .find(|row| row.reference() == reference)
                .cloned(),
            _ => None,
        })
}

/// Where `cite` stands in what the app holds.
pub fn resolve(feed: &Feed, cite: &Cite) -> Found {
    let Some(reference) = cite.artifact.as_deref() else {
        return match task(feed, &cite.task) {
            Ok(title) => Found::Task { title },
            Err(true) => Found::Missing,
            Err(false) => Found::Looking,
        };
    };
    if let Some(artifact) = held(feed, reference, &cite.task) {
        return Found::Artifact(Box::new(artifact));
    }
    // The task's detail lists every artifact it has: once it is here, an absent row is absent.
    if loaded_task(feed, &cite.task).is_some() {
        return Found::Missing;
    }
    match task(feed, &cite.task) {
        Err(true) => Found::Missing,
        _ => Found::Looking,
    }
}

/// The spellings among `cites` that open something now: the ones an inline link is drawn for.
pub fn opening(feed: &Feed, cites: &[Cite]) -> Vec<String> {
    cites
        .iter()
        .filter(|cite| matches!(resolve(feed, cite), Found::Task { .. } | Found::Artifact(_)))
        .map(|cite| cite.text.clone())
        .collect()
}

/// What a press on `cite` does: open the task's page, or the artifact full size at its section;
/// a citation that does not resolve says so and opens nothing.
pub fn press(feed: &Feed, cite: &Cite) -> Vec<Effect> {
    match resolve(feed, cite) {
        Found::Task { .. } => vec![Effect::OpenLive(Live::Task(cite.task.clone()))],
        Found::Artifact(artifact) => vec![Effect::OpenArtifact {
            reference: artifact.reference(),
            section: cite.section.clone(),
        }],
        Found::Looking => vec![Effect::Toast(format!("{}: {LOOKING}", cite.text))],
        Found::Missing => vec![Effect::Toast(format!("{}: {NOT_FOUND}", cite.text))],
    }
}

/// [`press`] for a citation as spelled (an inline link's).
pub fn press_text(feed: &Feed, text: &str) -> Vec<Effect> {
    match cites(text).into_iter().find(|cite| cite.text == text) {
        Some(cite) => press(feed, &cite),
        None => Vec::new(),
    }
}

/// One citation as a row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub cite: Cite,
    /// The task's or the artifact's title, else where the citation stands.
    pub title: String,
    /// The artifact's kind; the task citation's `task`.
    pub kind: String,
    /// It opens something; otherwise the row only says why not.
    pub opens: bool,
    /// What a screen reader hears.
    pub label: String,
    pub test_id: String,
}

/// `cites` as rows show them.
pub fn lines(feed: &Feed, cites: &[Cite]) -> Vec<Line> {
    cites
        .iter()
        .map(|cite| {
            let (title, kind, opens) = match resolve(feed, cite) {
                Found::Task { title } => (title.unwrap_or_default(), "task".to_owned(), true),
                Found::Artifact(artifact) => {
                    let title = match &cite.section {
                        Some(section) => format!("{} § {section}", artifact.title),
                        None => artifact.title.clone(),
                    };
                    (title, display_kind(artifact.kind), true)
                }
                Found::Looking => (LOOKING.to_owned(), String::new(), false),
                Found::Missing => (NOT_FOUND.to_owned(), String::new(), false),
            };
            let label = if opens {
                format!("{} {kind} {title}: open it", cite.text)
            } else {
                format!("{}, {title}", cite.text)
            };
            Line {
                test_id: format!("cite-{}", cite.text),
                cite: cite.clone(),
                title,
                kind,
                opens,
                label,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::Feed;

    /// A citation whose task corrald does not serve must never open anything: an empty viewer
    /// or a task page with nothing behind it.
    #[test]
    fn a_citation_corrald_does_not_serve_opens_nothing() {
        use crate::model::daemon::{ClientError, DetailCache, detail_key};

        let mut feed = Feed::default();
        let target = task_ref("T-404");
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                transcript_seams: Default::default(),
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: Some(ClientError {
                    code: "not_found".into(),
                    message: "task T-404".into(),
                }),
                merged: None,
            },
        );
        let cite = cites("see T-404:1#goal").remove(0);
        assert_eq!(cite.artifact.as_deref(), Some("T-404:1"));
        assert_eq!(cite.section.as_deref(), Some("goal"));
        assert!(matches!(
            press(&feed, &cite).as_slice(),
            [Effect::Toast(said)] if said.contains(NOT_FOUND)
        ));
    }
}
