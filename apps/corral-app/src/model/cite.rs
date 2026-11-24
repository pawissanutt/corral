//! Citations in text the app shows: `T-12` names a task, `T-12:3` one of its artifacts and
//! `T-12:3#section` a section of it. They are found with corral-types' grammar
//! (`references_in`), never a parser of the app's, and resolved against the task details the
//! app reads: a task resolves when corrald serves its detail, an artifact (or a section of one)
//! when corrald serves its content under the citation, which the engine resolves by its own
//! rules. Until then a citation is being looked up; once corrald answers `not_found` it is not
//! found, stays plain text and opens nothing; any other failure leaves it unknown.

use corral_protocol::detail::DetailRef;
use corral_types::{Artifact, references_in};

use crate::model::artifacts::{content_target, display_kind};
use crate::model::daemon::Feed;
use crate::model::work::{loaded_task, task_ref};
use crate::model::{Effect, Id, Live};

/// What an unresolved citation says when it is shown or pressed.
pub const NOT_FOUND: &str = "not found";
/// What a citation says while its task's detail is on its way.
pub const LOOKING: &str = "looking up…";
/// What a citation says while corrald cannot say whether it resolves (offline, an error).
pub const UNAVAILABLE: &str = "cannot be looked up now";
/// The heading over the artifacts a task's body cites.
pub const CITED: &str = "Cited";
/// The URL scheme a citation's inline link carries; it never leaves the app.
pub const SCHEME: &str = "cite:";

/// One citation as written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cite {
    /// As spelled in the text, the label it keeps: `T-12:03#4-fact-model`.
    pub text: String,
    pub task: Id,
    /// The artifact it names, canonically (`T-12:3`), as the engine reads the ordinal.
    pub artifact: Option<String>,
    pub section: Option<String>,
}

impl Cite {
    /// What corrald is asked for to resolve an artifact citation: the artifact, or its section.
    pub fn read(&self) -> Option<String> {
        let artifact = self.artifact.as_deref()?;
        Some(match &self.section {
            Some(section) => format!("{artifact}#{section}"),
            None => artifact.to_owned(),
        })
    }
}

/// `T-12:3` from the grammar's `T-12:03`: the engine reads the ordinal as a positive number.
/// One it cannot read stays as written, and corrald answers that it is not found.
fn canonical(address: &str) -> String {
    address
        .rsplit_once(':')
        .and_then(|(task, ordinal)| {
            let ordinal = ordinal.parse::<u32>().ok().filter(|ordinal| *ordinal > 0)?;
            Some(format!("{task}:{ordinal}"))
        })
        .unwrap_or_else(|| address.to_owned())
}

/// The distinct citations of `text`, in order of first appearance.
pub fn cites(text: &str) -> Vec<Cite> {
    references_in(text)
        .into_iter()
        .map(|reference| Cite {
            text: reference.as_str().to_owned(),
            task: Id::from(reference.task()),
            artifact: reference.artifact().map(canonical),
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

/// The details that resolve `cites`, each once: every cited task's, and for an artifact
/// citation the content corrald serves under it, which says whether it opens.
pub fn targets(cites: &[Cite]) -> Vec<DetailRef> {
    let mut targets: Vec<DetailRef> = Vec::new();
    for cite in cites {
        let content = cite.read().map(|read| content_target(&read));
        for target in std::iter::once(task_ref(&cite.task)).chain(content) {
            if !targets.contains(&target) {
                targets.push(target);
            }
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
    Unavailable,
    Missing,
}

/// The cited task's title as the app holds it, or where its lookup stands. The live board
/// outranks an older answer; otherwise only corrald's `not_found` makes it missing, and it does
/// even over an older cached detail.
fn task(feed: &Feed, id: &str) -> Result<Option<String>, Found> {
    if let Some(row) = feed.board.task(id) {
        return Ok(Some(row.title().to_owned()));
    }
    let error = feed
        .detail(&task_ref(id))
        .and_then(|cache| cache.error.as_ref());
    if error.is_some_and(|error| error.code == "not_found") {
        return Err(Found::Missing);
    }
    if let Some(detail) = loaded_task(feed, id) {
        return Ok(Some(detail.task.title.clone()));
    }
    Err(match error {
        Some(_) => Found::Unavailable,
        None => Found::Looking,
    })
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

/// Where `cite` stands. An artifact citation opens only once corrald has served its content
/// (or the cited section) under it: the engine, not the app, decides that it resolves.
pub fn resolve(feed: &Feed, cite: &Cite) -> Found {
    let task = task(feed, &cite.task);
    let (Some(address), Some(read)) = (cite.artifact.as_deref(), cite.read()) else {
        return match task {
            Ok(title) => Found::Task { title },
            Err(found) => found,
        };
    };
    if task == Err(Found::Missing) {
        return Found::Missing;
    }
    let content = feed.detail(&content_target(&read));
    let error = content.and_then(|cache| cache.error.as_ref());
    if error.is_some_and(|error| error.code == "not_found") {
        return Found::Missing;
    }
    // Any other failure leaves it unknown, even over content an earlier read cached.
    if error.is_some() || task == Err(Found::Unavailable) {
        return Found::Unavailable;
    }
    if !content.is_some_and(|cache| cache.body().is_some()) {
        return Found::Looking;
    }
    // corrald served it, so it exists: a detail that does not list it yet is behind.
    match held(feed, address, &cite.task) {
        Some(artifact) => Found::Artifact(Box::new(artifact)),
        None => match task {
            Err(found) => found,
            Ok(_) => Found::Looking,
        },
    }
}

/// Each cited task's served detail version, which says when what it holds may have changed.
pub fn versions(feed: &Feed, cites: &[Cite]) -> Vec<Option<String>> {
    targets(cites)
        .into_iter()
        .filter(|target| matches!(target, DetailRef::Task { .. }))
        .map(|target| {
            feed.detail(&target)
                .and_then(|cache| cache.pages.last())
                .map(|page| page.etag.clone())
        })
        .collect()
}

/// The content reads of `cites` corrald answered with an error, asked again once a cited
/// task's detail changes (an artifact attached since, a blob restored): a miss is not kept.
pub fn failed_reads(feed: &Feed, cites: &[Cite]) -> Vec<DetailRef> {
    targets(cites)
        .into_iter()
        .filter(|target| matches!(target, DetailRef::Artifact { .. }))
        .filter(|target| {
            feed.detail(target)
                .is_some_and(|cache| cache.error.is_some())
        })
        .collect()
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
        Found::Unavailable => vec![Effect::Toast(format!("{}: {UNAVAILABLE}", cite.text))],
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
                Found::Unavailable => (UNAVAILABLE.to_owned(), String::new(), false),
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

    fn failed(feed: &mut Feed, target: DetailRef, code: &str) {
        use crate::model::daemon::{ClientError, DetailCache, detail_key};

        feed.details.insert(
            detail_key(&target),
            DetailCache {
                transcript_seams: Default::default(),
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: Some(ClientError {
                    code: code.into(),
                    message: "corrald's answer".into(),
                }),
                merged: None,
            },
        );
    }

    /// Only corrald's `not_found` (for the task, or for the content served under the citation,
    /// which is where the engine decides a section or a blob is missing) makes a citation not
    /// found; any other failure leaves it unknown. Either way nothing opens. A zero-padded
    /// ordinal names the artifact the engine reads, under the label as written.
    #[test]
    fn only_corralds_not_found_makes_a_citation_not_found_and_neither_opens() {
        let mut feed = Feed::default();
        let [missing, failing, padded] =
            ["T-404", "T-5:2#goal", "T-6:03#goal"].map(|text| cites(text).remove(0));
        failed(&mut feed, task_ref("T-404"), "not_found");
        failed(&mut feed, content_target("T-5:2#goal"), "unavailable");
        failed(&mut feed, content_target("T-6:3#goal"), "not_found");
        assert_eq!(padded.text, "T-6:03#goal");
        assert_eq!(padded.read().as_deref(), Some("T-6:3#goal"));
        for (cite, said) in [
            (missing, NOT_FOUND),
            (failing, UNAVAILABLE),
            (padded, NOT_FOUND),
        ] {
            assert!(
                matches!(press(&feed, &cite).as_slice(), [Effect::Toast(toast)] if toast.ends_with(said)),
                "{}",
                cite.text
            );
        }
    }
}
