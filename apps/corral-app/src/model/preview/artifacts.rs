//! The sample's artifact catalog as corrald serves it: the list newest first across pages,
//! narrowed by kind and project prefix and exact task and filtered by words that must all match
//! title, kind or reference, each row with its project and run; and each artifact's content by
//! reference.

use corral_protocol::detail::*;
use corral_types::{
    Actor, Artifact, ArtifactKind, CheckKind, CheckResult, Reference, Status, section_slug,
};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::details::{files, patch};

use super::{ago, record, records, stamp};
use crate::model::artifacts::shown_by_default;
use crate::model::daemon::{DetailCache, Feed, detail_key};

/// The sample pages at ten rows, so its two dozen artifacts walk past a page and a second;
/// corrald pages at a hundred rows or its reply budget.
const PAGE: usize = 10;

/// A small capture of the app (a sidebar, a head and rows), served as a screenshot's bytes.
const SCREENSHOT: &str = "iVBORw0KGgoAAAANSUhEUgAAAGAAAAA8CAIAAAAWtijjAAAApklEQVR42u3bIQqAMBgGUM8iBu9/ApOYrRZZELOgyWC2DRHc3IPvAnvh/7axVXXTPkiYp0JSAQIECBAgQIAA3XLsWyEBBAhQikBD332YmIWtS3glgAABApQhUPrtAwhQ1kBjOJMNIECAABUPpMUAAXIWAwQIECD3QVoMECAzCBAgQIDsg7QYIEBmECBAgADZB2kxQIB+P4M8JAcECFDBQH77AAIUlwuEzW4tvLY9qAAAAABJRU5ErkJggg==";

/// Serve the list `target` names into `feed.details`: every page up to the one `page` asks for
/// (corrald's opaque token; the sample's is `<etag>:<offset>`).
pub(super) fn list(feed: &mut Feed, target: &DetailRef, page: Option<&str>) -> Result<(), String> {
    let pages = match page {
        None => 1,
        Some(token) => {
            let offset: usize = token
                .rsplit_once(':')
                .and_then(|(_, offset)| offset.parse().ok())
                .ok_or_else(|| format!("invalid: artifact page token {token}"))?;
            offset / PAGE + 1
        }
    };
    serve(feed, target, pages);
    Ok(())
}

/// Serve the list again after the sample changed, keeping every page the owner walked to.
pub(super) fn again(feed: &mut Feed, target: &DetailRef) {
    let walked = feed
        .detail(target)
        .map_or(1, |cache| cache.pages.len().max(1));
    serve(feed, target, walked);
}

/// Whether the request keeps `row`.
fn kept(target: &DetailRef, row: &Artifact) -> bool {
    let DetailRef::Artifacts { filter, narrowing } = target else {
        return false;
    };
    let lower = |text: &str| text.to_lowercase();
    let kind = row.kind.as_str();
    let prefix = |value: Option<&str>, wanted: &Option<String>| {
        wanted.as_deref().is_none_or(|wanted| {
            value.is_some_and(|value| lower(value).starts_with(&lower(wanted)))
        })
    };
    // corrald hides submissions, records and replaced rows unless asked for all or a kind.
    let hidden = !shown_by_default(row)
        && narrowing
            .as_ref()
            .is_none_or(|narrowing| !narrowing.all && narrowing.kind.is_none());
    let narrowed = !hidden
        && narrowing.as_ref().is_none_or(|narrowing| {
            prefix(Some(kind), &narrowing.kind)
                && prefix(row.project.as_deref(), &narrowing.project)
                && narrowing
                    .task
                    .as_deref()
                    .is_none_or(|task| row.task == task)
        });
    let title = lower(&row.title);
    let reference = lower(&row.reference());
    narrowed
        && filter
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .map(lower)
            .all(|word| title.contains(&word) || kind.contains(&word) || reference.contains(&word))
}

fn serve(feed: &mut Feed, target: &DetailRef, pages: usize) {
    let mut rows: Vec<Artifact> = records(feed)
        .into_iter()
        .flat_map(|record| {
            let project = record.task.project;
            record.artifacts.into_iter().map(move |row| Artifact {
                project: project.clone(),
                ..row
            })
        })
        .chain(library(feed))
        .filter(|row| kept(target, row))
        .collect();
    rows.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| super::number(&b.task).cmp(&super::number(&a.task)))
            .then_with(|| b.id.cmp(&a.id))
    });
    let etag = format!("preview-{}", feed.changes_head);
    let chunks: Vec<&[Artifact]> = if rows.is_empty() {
        vec![&[]]
    } else {
        rows.chunks(PAGE).collect()
    };
    let shown = pages.min(chunks.len());
    let replies = chunks
        .iter()
        .take(shown)
        .enumerate()
        .map(|(at, chunk)| DetailReply {
            etag: etag.clone(),
            as_of: stamp(feed.now),
            seq: feed.changes_head,
            next: (at + 1 < chunks.len()).then(|| format!("{etag}:{}", (at + 1) * PAGE)),
            body: Some(DetailBody::Artifacts(ArtifactsPage {
                rows: chunk.to_vec(),
            })),
        })
        .collect();
    rows.truncate(shown * PAGE);
    feed.details.insert(
        detail_key(target),
        DetailCache {
            transcript_seams: Default::default(),
            target: target.clone(),
            pages: replies,
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Artifacts(ArtifactsPage { rows })),
        },
    );
}

/// The project `corral`'s library: what every task of it may refer to.
const LIBRARY: &str = "corral";
const LIBRARY_NOTES: &str = "# Architecture notes\n\nThe app reaches corrald only through its socket; every engine operation stays enabled and corrald decides.\n";

/// The sample's workflow recipe: the guide's submit-to-land example.
pub(super) const RECIPE: &str =
    include_str!("../../../../../docs/examples/submit-to-land.diagram.json");
/// The library ordinal of [`RECIPE`].
pub(super) const RECIPE_ID: u32 = 2;

/// The sample's project-owned library rows.
pub(super) fn library(feed: &Feed) -> Vec<Artifact> {
    let recipe = Artifact {
        task: LIBRARY.into(),
        id: RECIPE_ID,
        sha256: "5d17".repeat(16),
        bytes: RECIPE.len() as u64,
        kind: ArtifactKind::Report,
        title: "Submit to land".into(),
        at: ago(feed, 2 * super::sample::DAY),
        by: Actor::Agent,
        project: Some(LIBRARY.into()),
        run: None,
        replaces: None,
        replaced_by: None,
    };
    vec![
        recipe,
        Artifact {
            task: LIBRARY.into(),
            id: 1,
            sha256: "4c1b".repeat(16),
            bytes: LIBRARY_NOTES.len() as u64,
            kind: ArtifactKind::Reference,
            title: "architecture-notes.md".into(),
            at: ago(feed, 6 * super::sample::DAY),
            by: Actor::Owner,
            project: Some(LIBRARY.into()),
            run: None,
            replaces: None,
            replaced_by: None,
        },
    ]
}

/// The artifact `reference` names, its content made from the sample's own facts.
pub(super) fn content(feed: &Feed, reference: &str) -> Result<ArtifactPage, String> {
    // A cited section, as corrald serves one: the Markdown source of that section, else
    // not found.
    let cited = Reference::parse(reference);
    if let Some(slug) = cited.and_then(Reference::section) {
        let address = cited.and_then(Reference::artifact).unwrap_or(reference);
        let page = content(feed, address)?;
        let markdown = page.media_type.as_deref() == Some("text/markdown")
            && matches!(page.encoding, ArtifactEncoding::Text);
        let part = markdown
            .then(|| section(&page.content, slug))
            .flatten()
            .ok_or_else(|| format!("not_found: section {slug} of {address}"))?
            .to_owned();
        return Ok(ArtifactPage {
            content: part,
            ..page
        });
    }
    let (task, key) = reference
        .rsplit_once(':')
        .ok_or_else(|| format!("invalid: artifact reference {reference}"))?;
    // The ordinal is a number, as the engine reads it: `T-2:03` is `T-2:3`.
    let key = key.parse::<u32>().ok().filter(|key| *key > 0);
    if task == LIBRARY {
        library(feed)
            .iter()
            .find(|row| Some(row.id) == key)
            .ok_or_else(|| format!("not_found: artifact {reference}"))?;
        if key == Some(RECIPE_ID) {
            return Ok(ArtifactPage {
                task: task.into(),
                key: RECIPE_ID.to_string(),
                // Titled in words: corrald names no media type and records no format for it.
                media_type: None,
                source_format: None,
                encoding: ArtifactEncoding::Text,
                content: RECIPE.into(),
                truncated: false,
            });
        }
        return Ok(ArtifactPage {
            task: task.into(),
            key: key.unwrap_or_default().to_string(),
            media_type: Some("text/markdown".into()),
            // Attached from a `.md` file: the catalog records its format from the path.
            source_format: Some("markdown".into()),
            encoding: ArtifactEncoding::Text,
            content: LIBRARY_NOTES.into(),
            truncated: false,
        });
    }
    let record = record(feed, task).ok_or_else(|| format!("not_found: task {task}"))?;
    let row = record
        .artifacts
        .iter()
        .find(|row| Some(row.id) == key)
        .ok_or_else(|| format!("not_found: artifact {reference}"))?;
    let text = |media: &str, content: String| (media.to_owned(), ArtifactEncoding::Text, content);
    let (media_type, encoding, content) = match row.kind {
        ArtifactKind::Diff => text(
            "text/x-diff",
            files(task)
                .iter()
                .map(|(path, ..)| patch(task, path))
                .collect(),
        ),
        ArtifactKind::Gate => text("text/plain", gate_log(&record)),
        ArtifactKind::Evidence if row.title.ends_with(".png") => (
            "image/png".to_owned(),
            ArtifactEncoding::Base64,
            SCREENSHOT.to_owned(),
        ),
        ArtifactKind::Evidence => text("text/plain", evidence(&record, row)),
        _ if row.title.ends_with(".md") => text("text/markdown", markdown(feed, &record, row)),
        // An agent's diagram titled in words: corrald names no media type for it.
        ArtifactKind::Reference if !row.title.contains('.') => text("", DIAGRAMS.to_owned()),
        // corrald names the media type from the title, so a page titled in words is plain text.
        _ => text(
            if row.title.ends_with(".html") {
                "text/html"
            } else {
                "text/plain"
            },
            format!(
                "<!doctype html><title>{}</title><h1>{}</h1><p>Preview sample report.</p>",
                row.title, record.task.title
            ),
        ),
    };
    Ok(ArtifactPage {
        task: task.into(),
        key: key.unwrap_or_default().to_string(),
        media_type: (!media_type.is_empty()).then_some(media_type),
        // The sample attaches each paper from a file named as its title; corrald records a
        // `.md` file's format from that path, never from the title it shows.
        source_format: row.title.ends_with(".md").then(|| "markdown".to_owned()),
        encoding,
        content,
        truncated: false,
    })
}

/// Whether a citation resolves, as corrald answers it: the cited row, and the cited section's
/// size; any miss is not found, as `content` decides it.
pub(super) fn citation(feed: &Feed, reference: &str) -> Result<CitationDetail, String> {
    let page = content(feed, reference)?;
    let rows = if page.task == LIBRARY {
        library(feed)
    } else {
        record(feed, &page.task)
            .map(|record| record.artifacts)
            .unwrap_or_default()
    };
    let artifact = rows
        .into_iter()
        .find(|row| row.id.to_string() == page.key)
        .ok_or_else(|| format!("not_found: artifact {reference}"))?;
    let section_bytes = Reference::parse(reference)
        .and_then(Reference::section)
        .map(|_| page.content.len() as u64);
    Ok(CitationDetail {
        artifact,
        section_bytes,
    })
}

fn gate_log(record: &TaskDetail) -> String {
    record
        .checks
        .iter()
        .filter(|check| check.kind == CheckKind::Gate)
        .map(|check| {
            let result = match check.result {
                CheckResult::Pass => "passed",
                CheckResult::Fail => "FAILED",
            };
            format!(
                "$ just check\n   Compiling nsh-app-corral v0.1.0\n    Finished test profile in 41.2s\n     Running unittests src/lib.rs\ntest result: {result}. {}\n{} · {}\n",
                check.notes.as_deref().unwrap_or("212 passed; 0 failed"),
                result,
                check.at
            )
        })
        .collect()
}

fn evidence(record: &TaskDetail, row: &Artifact) -> String {
    format!(
        "evidence {} · {}\nrecorded {}\n\n{}\n",
        row.reference(),
        record.task.title,
        row.at,
        record
            .runs
            .iter()
            .map(|run| format!(
                "run {} · {} · {} · started {}",
                run.id,
                run.role.as_str(),
                run.profile,
                run.started
            ))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// How a report says its work went: a Mermaid flowchart beside an ordinary code fence.
/// A descriptively titled agent diagram: a sequence and an ER diagram beside highlighted code.
const DIAGRAMS: &str = "# How usage reaches the page\n\n```mermaid\nsequenceDiagram\n  participant App\n  App->>corrald: usage.series\n  corrald-->>App: daily buckets\n```\n\n```mermaid\nerDiagram\n  TASK ||--o{ RUN : starts\n  RUN ||--o{ USAGE : records\n  USAGE {\n    string model\n    int tokens\n  }\n```\n\n```rust\nlet total: u64 = buckets.iter().map(|b| b.tokens).sum();\n```\n";
const FLOW: &str = "## Flow\n\n```mermaid\nflowchart LR\n  Brief[Brief] --> Work[Work] --> Gate{Gate}\n  Gate -->|pass| Review[Review]\n  Gate -->|fail| Work\n```\n\n```sh\njust check\n```\n\n";

/// A summary, report or plan, written as the agents write them.
fn markdown(feed: &Feed, record: &TaskDetail, row: &Artifact) -> String {
    let task = &record.task;
    let mut out = format!(
        "# {} · {}\n\n**{}**\n\n",
        row.title.trim_end_matches(".md"),
        task.id,
        task.title
    );
    if !task.body.trim().is_empty() {
        out.push_str(task.body.trim());
        out.push_str("\n\n");
    }
    if row.kind == ArtifactKind::Reference && row.title == "plan.md" {
        out.push_str("## Members\n\n");
        for child in records(feed)
            .iter()
            .filter(|child| child.task.parent.as_deref() == Some(task.id.as_str()))
        {
            let done = matches!(child.task.status, Status::Done);
            out.push_str(&format!(
                "- [{}] {} {}\n",
                if done { "x" } else { " " },
                child.task.id,
                child.task.title
            ));
        }
        return out;
    }
    let changed = files(&task.id);
    if !changed.is_empty() {
        out.push_str("## Changed\n\n| File | Added | Deleted |\n|---|---|---|\n");
        for (path, added, deleted, _) in changed {
            out.push_str(&format!("| `{path}` | {added} | {deleted} |\n"));
        }
        out.push('\n');
    }
    if row.kind == ArtifactKind::Report {
        out.push_str(FLOW);
    }
    out.push_str("## Checks\n\n");
    if record.checks.is_empty() {
        out.push_str("- No check has run yet.\n");
    }
    for check in &record.checks {
        let result = match check.result {
            CheckResult::Pass => "passed",
            CheckResult::Fail => "failed",
        };
        out.push_str(&format!(
            "- {} {result}{}\n",
            check.kind.as_str(),
            check
                .notes
                .as_deref()
                .map(|notes| format!(": {notes}"))
                .unwrap_or_default()
        ));
    }
    out
}

/// A Markdown heading: its slug, level and the start of its source line.
struct Heading {
    slug: String,
    level: u8,
    start: usize,
}

/// The headings of `text` as the engine finds them when it serves a cited section.
fn headings(text: &str) -> Vec<Heading> {
    let mut events = Parser::new(text).into_offset_iter();
    let mut headings = Vec::new();
    while let Some((event, range)) = events.next() {
        let Event::Start(Tag::Heading { level, .. }) = event else {
            continue;
        };
        let mut words = String::new();
        for (event, _) in events.by_ref() {
            match event {
                Event::End(TagEnd::Heading(_)) => break,
                Event::Text(text) | Event::Code(text) => words.push_str(&text),
                Event::SoftBreak | Event::HardBreak => words.push(' '),
                _ => {}
            }
        }
        headings.push(Heading {
            slug: section_slug(&words),
            level: level as u8,
            start: text[..range.start].rfind(['\n', '\r']).map_or(0, |i| i + 1),
        });
    }
    headings
}

/// The Markdown section whose heading's slug is `slug`: its heading and everything up to the
/// next heading of its level or above, as the engine cuts it.
fn section<'a>(text: &'a str, slug: &str) -> Option<&'a str> {
    let headings = headings(text);
    let (at, hit) = headings
        .iter()
        .enumerate()
        .find(|(_, heading)| heading.slug == slug)?;
    let end = headings[at + 1..]
        .iter()
        .find(|heading| heading.level <= hit.level)
        .map_or(text.len(), |heading| heading.start);
    Some(&text[hit.start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cited section must end at the next heading of its level, not at a deeper one or at a
    /// heading-looking line inside code, and a setext heading is a heading.
    #[test]
    fn a_cited_section_runs_to_the_next_heading_of_its_level() {
        let text = "# Doc\n## Goal\nwhy\n### Detail\n```\n## not a heading\n```\n\n    # code\n\nAcceptance\n----------\nok\n";
        assert_eq!(
            section(text, "goal"),
            Some("## Goal\nwhy\n### Detail\n```\n## not a heading\n```\n\n    # code\n\n")
        );
        assert_eq!(
            section(text, "acceptance"),
            Some("Acceptance\n----------\nok\n")
        );
        assert_eq!(section(text, "not-a-heading"), None);
        assert_eq!(section(text, "code"), None);
    }
}
