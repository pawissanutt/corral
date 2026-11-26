use corral_protocol::detail::{
    ArtifactNarrowing, ArtifactsPage, DetailBody, DetailGet, DetailRef, DetailReply, TaskDetail,
};
use corral_types::{Actor, Artifact, ArtifactKind, Task};

use super::*;
use crate::model::daemon::{DetailCache, Feed, detail_key};

fn artifact(task: &str, id: u32, kind: ArtifactKind, title: &str, minute: i64) -> Artifact {
    Artifact {
        task: task.into(),
        id,
        sha256: "ab".repeat(32),
        bytes: 2048,
        kind,
        title: title.into(),
        at: jiff::Timestamp::from_second(1_800_000_000 + minute * 60).unwrap(),
        by: Actor::Agent,
        project: None,
        run: None,
        replaces: None,
        replaced_by: None,
    }
}

/// A task the app holds the detail of, in `project`.
fn task(feed: &mut Feed, id: &str, project: &str) {
    let target = DetailRef::Task { id: id.into() };
    let detail = TaskDetail {
        pending_review_verdict: None,
        gate_attempt: None,
        task: Task {
            id: id.into(),
            title: format!("Task {id}"),
            project: Some(project.into()),
            ..Task::default()
        },
        runs: Vec::new(),
        checks: Vec::new(),
        artifacts: Vec::new(),
        events: Vec::new(),
    };
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Task(detail)),
        },
    );
}

/// The list `target` as corrald served it so far: these pages, and whether one more waits.
fn served(feed: &mut Feed, target: &DetailRef, pages: Vec<Vec<Artifact>>, more: bool) {
    let count = pages.len();
    let pages = pages
        .into_iter()
        .enumerate()
        .map(|(at, rows)| DetailReply {
            etag: "e".into(),
            as_of: "2027-01-15T08:00:00Z".into(),
            seq: 1,
            next: (at + 1 < count || more).then(|| format!("e:{}", at + 1)),
            body: Some(DetailBody::Artifacts(ArtifactsPage { rows })),
        })
        .collect();
    feed.details.insert(
        detail_key(target),
        DetailCache {
            transcript_seams: Default::default(),
            target: target.clone(),
            pages,
            loading: false,
            stale: false,
            error: None,
            merged: None,
        },
    );
}

fn titles(page: &Page) -> Vec<&str> {
    page.results
        .rows
        .iter()
        .map(|row| row.title.as_str())
        .collect()
}

/// corrald pages through the rows a request narrows: the field tokens have to travel as its
/// narrowing and the words as its filter, or the app pages through unrelated artifacts; a second
/// token of the same field still narrows what it serves.
#[test]
fn tokens_go_into_the_narrowing_and_words_into_the_filter() {
    let query = Query::parse("Kind:report coverage task:t-10 project:");
    assert_eq!(query.kinds, ["report"]);
    assert_eq!(query.tasks, ["T-10"]);
    assert_eq!(query.words, ["coverage"]);
    assert!(
        query.projects.is_empty(),
        "a field still being typed narrows nothing"
    );
    assert_eq!(
        query.request(false),
        DetailRef::Artifacts {
            filter: Some("coverage".into()),
            narrowing: Some(ArtifactNarrowing {
                kind: Some("report".into()),
                task: Some("T-10".into()),
                project: None,
                ..Default::default()
            }),
        }
    );
    assert_eq!(
        Query::parse("gate log project:si").request(false),
        DetailRef::Artifacts {
            filter: Some("gate log".into()),
            narrowing: Some(ArtifactNarrowing {
                project: Some("si".into()),
                ..ArtifactNarrowing::default()
            }),
        }
    );
    assert_eq!(
        Query::parse("  ").request(false),
        DetailRef::Artifacts {
            filter: None,
            narrowing: None,
        }
    );

    let mut feed = Feed::default();
    task(&mut feed, "T-6", "site");
    let state = State {
        query: "kind:report kind:re".into(),
        ..State::default()
    };
    let target = Query::parse(&state.query).request(false);
    served(
        &mut feed,
        &target,
        vec![vec![
            artifact("T-6", 1, ArtifactKind::Report, "copy.md", 3),
            artifact("T-6", 2, ArtifactKind::Reference, "notes.md", 1),
        ]],
        false,
    );
    assert_eq!(titles(&page(&state, &feed, Form::Terminal)), ["copy.md"]);
}

/// Only the page corrald served is listed: unless reaching its end asks for the next, the owner
/// never sees past the first hundred artifacts.
#[test]
fn walking_to_the_last_result_asks_for_the_next_page() {
    let mut feed = Feed::default();
    let mut state = State::default();
    let target = Query::parse("").request(false);
    served(
        &mut feed,
        &target,
        vec![vec![
            artifact("T-2", 1, ArtifactKind::Log, "gate log", 3),
            artifact("T-3", 1, ArtifactKind::Reference, "plan.md", 2),
            artifact("T-1", 1, ArtifactKind::Diff, "diff", 1),
        ]],
        true,
    );
    let shown = page(&state, &feed, Form::Desktop);
    assert_eq!(titles(&shown), ["gate log", "plan.md", "diff"]);
    assert_eq!(more(&state, &feed), None);
    state.act(Action::Walk(Walk::Down), &feed);
    assert_eq!(more(&state, &feed), None);
    state.act(Action::Walk(Walk::Down), &feed);
    assert_eq!(state.selected.as_deref(), Some("T-1:1"));
    assert_eq!(
        more(&state, &feed),
        Some(DetailGet {
            target: target.clone(),
            etag: None,
            page: Some("e:1".into()),
        })
    );
    served(
        &mut feed,
        &target,
        vec![
            vec![
                artifact("T-2", 1, ArtifactKind::Log, "gate log", 3),
                artifact("T-3", 1, ArtifactKind::Reference, "plan.md", 2),
                artifact("T-1", 1, ArtifactKind::Diff, "diff", 1),
            ],
            vec![artifact("T-4", 1, ArtifactKind::Summary, "summary.md", 0)],
        ],
        false,
    );
    assert_eq!(more(&state, &feed), None, "not on the last result any more");
    state.act(Action::Walk(Walk::PageDown), &feed);
    assert_eq!(state.selected.as_deref(), Some("T-4:1"));
    assert_eq!(more(&state, &feed), None, "corrald has no page after it");
}

#[test]
fn a_result_opens_its_task_and_a_task_opens_the_finder_on_itself() {
    let mut feed = Feed::default();
    task(&mut feed, "T-6", "site");
    let mut state = State::default();
    let target = Query::parse("").request(false);
    served(
        &mut feed,
        &target,
        vec![vec![artifact("T-6", 1, ArtifactKind::Report, "copy.md", 1)]],
        false,
    );
    state.act(Action::Press("T-6:1".into()), &feed);
    let shown = page(&state, &feed, Form::Desktop);
    let full = shown
        .full
        .expect("a press on the selected result opens it full size");
    assert_eq!(full.task.as_str(), "T-6");
    let effects = state.act(Action::OpenTask(full.task), &feed);
    assert!(matches!(
        effects.as_slice(),
        [Effect::OpenLive(Live::Task(id))] if id.as_str() == "T-6"
    ));
    assert_eq!(state.full, None);

    let effects = state.act(Action::Find(Id::from("T-12")), &feed);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Go(Destination::Artifacts)]
    ));
    assert_eq!(Query::parse(&state.query).tasks, ["T-12"]);
}

/// A project's library is narrowed by its name as typed, which corrald matches exactly, even
/// where the name reads as task shorthand (`t-42` is a valid project name, `T-42` a task); All
/// reaches the rows corrald hides; a library row has no task to open.
#[test]
fn a_library_is_found_by_its_project_and_offers_no_task() {
    let mut feed = Feed::default();
    for name in ["ClientDocs", "t-42"] {
        feed.projects.insert(
            name.into(),
            corral_protocol::rows::ProjectRow {
                id: name.into(),
                ..Default::default()
            },
        );
    }
    let narrowing = |state: &State, feed: &Feed| match state.request(feed) {
        DetailRef::Artifacts { narrowing, .. } => narrowing.unwrap_or_default(),
        _ => ArtifactNarrowing::default(),
    };
    let typed = |query: &str| State {
        query: query.into(),
        ..State::default()
    };
    assert_eq!(
        narrowing(&typed("task:t-42"), &feed).task.as_deref(),
        Some("t-42"),
        "a known project wins over task shorthand"
    );
    assert_eq!(
        narrowing(&typed("task:t-43"), &feed).task.as_deref(),
        Some("T-43"),
        "an unknown name is still task shorthand"
    );
    let mut state = typed("task:ClientDocs");
    assert_eq!(narrowing(&state, &feed).task.as_deref(), Some("ClientDocs"));
    assert!(!narrowing(&state, &feed).all);
    state.act(Action::All(true), &feed);
    assert!(narrowing(&state, &feed).all);
    let mut row = artifact("ClientDocs", 3, ArtifactKind::Reference, "notes.md", 1);
    row.project = Some("ClientDocs".into());
    let target = state.request(&feed);
    served(&mut feed, &target, vec![vec![row]], false);
    let shown = page(&state, &feed, Form::Desktop);
    let preview = shown.preview.expect("the library row is selected");
    assert_eq!(preview.reference, "ClientDocs:3");
    assert_eq!(preview.open_task, None);

    let mut state = typed("task:t-42");
    state.act(Action::All(true), &feed);
    let mut library = artifact("t-42", 1, ArtifactKind::Reference, "brief.md", 2);
    library.project = Some("t-42".into());
    let target = state.request(&feed);
    served(
        &mut feed,
        &target,
        vec![vec![
            library,
            artifact("T-42", 1, ArtifactKind::Report, "other.md", 1),
        ]],
        false,
    );
    assert_eq!(
        titles(&page(&state, &feed, Form::Desktop)),
        ["brief.md"],
        "the task T-42's artifact is not the project t-42's"
    );
}

/// The content corrald served for `artifact` so far, and whether more of it waits.
fn content_served(feed: &mut Feed, artifact: &Artifact, text: &str, truncated: bool) {
    served_as(feed, artifact, "text/plain", text, truncated);
}

/// [`content_served`] under the media type corrald named, which it derives from the title.
fn served_as(feed: &mut Feed, artifact: &Artifact, media: &str, text: &str, truncated: bool) {
    served_with(feed, artifact, media, None, text, truncated);
}

/// [`served_as`] with the catalog's format hint, recorded from the attached file's path.
fn served_with(
    feed: &mut Feed,
    artifact: &Artifact,
    media: &str,
    source_format: Option<&str>,
    text: &str,
    truncated: bool,
) {
    let target = content_target(&artifact.reference());
    let body = DetailBody::Artifact(corral_protocol::detail::ArtifactPage {
        task: artifact.task.clone(),
        key: artifact.id.to_string(),
        media_type: Some(media.into()),
        source_format: source_format.map(str::to_owned),
        encoding: corral_protocol::detail::ArtifactEncoding::Text,
        content: text.into(),
        truncated,
    });
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(body),
        },
    );
}

/// A head read past the preview bound must say it is partial; one long line must not reach the
/// preview whole; a small artifact still shows all of itself with no note.
#[test]
fn previews_stay_bounded_and_say_when_they_are_partial() {
    let mut feed = Feed::default();
    let mut large = artifact("T-1", 1, ArtifactKind::Report, "run.txt", 1);
    large.bytes = 50 * 1024 * 1024;
    let line = "x".repeat(70 * 1024);
    content_served(&mut feed, &large, &line, true);
    for full in [false, true] {
        let shown = content(&feed, &large, Form::Desktop, full);
        let Body::Text(text) = &shown.body else {
            panic!("a text head draws as text");
        };
        assert!(text.len() <= if full { line.len() } else { PREVIEW_BYTES });
        assert!(
            shown
                .cut
                .is_some_and(|cut| cut.starts_with("Partial preview"))
        );
    }

    // Full size, a partial head of many short lines stays within a line budget too.
    content_served(&mut feed, &large, &"x\n".repeat(30_000), true);
    let shown = content(&feed, &large, Form::Desktop, true);
    let Body::Text(text) = &shown.body else {
        panic!("a text head draws as text");
    };
    assert_eq!(text.lines().count(), FULL_PARTIAL_LINES);

    let small = artifact("T-1", 2, ArtifactKind::Report, "notes.txt", 2);
    content_served(&mut feed, &small, "one\ntwo\n", false);
    let shown = content(&feed, &small, Form::Desktop, false);
    assert_eq!(shown.body, Body::Text("one\ntwo\n".into()));
    assert_eq!(shown.cut, None);
}

/// The reported standalone report: titled in words, so corrald names no HTML media type, yet
/// its own start says it is a page; a `.html` title without that start is not one. It must offer Open in browser over the whole verified
/// artifact rather than read as plain text; text that merely mentions a tag must not, and a
/// page past the preview's bound is refused before anything is read.
#[test]
fn a_descriptively_titled_html_report_opens_in_the_browser() {
    let mut feed = Feed::default();
    let report = artifact(
        "T-7",
        1,
        ArtifactKind::Report,
        "Standalone coverage report for the review",
        1,
    );
    content_served(
        &mut feed,
        &report,
        "\u{feff}\n<!-- generated -->\n<!DOCTYPE html>\n<html><body><h1>Coverage</h1></body></html>\n",
        false,
    );
    let shown = preview(&feed, &report, Form::Desktop, false, None);
    assert!(shown.html, "its doctype makes it a page");
    assert!(matches!(shown.content.body, Body::Facts(_)));
    assert_eq!(
        (shown.save.reference.as_str(), shown.save.bytes),
        ("T-7:1", report.bytes),
        "the open reads and verifies the whole artifact, not the head"
    );
    assert_eq!(browser::refusal(&shown.save), None);

    let notes = artifact("T-7", 2, ArtifactKind::Report, "Notes on html", 2);
    content_served(&mut feed, &notes, "We should emit <html> later.\n", false);
    assert!(!preview(&feed, &notes, Form::Desktop, false, None).html);

    // A `.html` title, and the `text/html` corrald names from it, are not evidence of a page.
    let named = artifact("T-7", 3, ArtifactKind::Report, "notes.html", 3);
    served_as(
        &mut feed,
        &named,
        "text/html",
        "Plain notes, no markup.\n",
        false,
    );
    let shown = preview(&feed, &named, Form::Desktop, false, None);
    assert!(!shown.html, "a suffix alone offers no browser");
    assert!(matches!(shown.content.body, Body::Text(_)));

    let mut huge = report.clone();
    huge.bytes = browser::MAX_BYTES + 1;
    assert!(browser::refusal(&preview(&feed, &huge, Form::Desktop, false, None).save).is_some());
}

/// A Markdown artifact with a Mermaid fence and ordinary code stays rendered Markdown: its
/// headings and table survive, the Mermaid fence reads as a diagram and the Rust fence stays
/// code, not a diagram or an error.
#[test]
fn markdown_draws_its_mermaid_fence_and_keeps_ordinary_code() {
    use nsh_base::text_view::{Block, Parser};

    let mut feed = Feed::default();
    let plan = artifact("T-8", 1, ArtifactKind::Report, "plan.md", 1);
    let text = "# Plan\n\n| Step | Owner |\n|---|---|\n| gate | [T-8](https://example.invalid) |\n\n```mermaid\nflowchart TD\n  A[Brief] --> B{Gate}\n  B -->|pass| C[Land]\n```\n\n```rust\nfn main() { println!(\"A --> B\"); }\n```\n";
    content_served(&mut feed, &plan, text, false);
    let Body::Markdown(words) = content(&feed, &plan, Form::Desktop, true).body else {
        panic!("a .md artifact draws as Markdown");
    };
    let blocks = Parser::new().markdown(&words).blocks;
    assert!(matches!(blocks.first(), Some(Block::Heading { .. })));
    assert!(blocks.iter().any(|block| matches!(block, Block::Table(_))));
    let fences: Vec<_> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::CodeBlock(code) => Some(code),
            _ => None,
        })
        .collect();
    assert_eq!(fences.len(), 2);
    assert!(mermaid::is_mermaid(fences[0].language.as_deref()));
    let diagram = mermaid::parse(&fences[0].code).expect("the flowchart is drawn");
    assert_eq!((diagram.nodes.len(), diagram.edges.len()), (3, 2));
    assert!(!mermaid::is_mermaid(fences[1].language.as_deref()));
    assert!(fences[1].code.contains("println!"));
}

/// An agent's diagram report keeps a descriptive title (no `.md`), so corrald names no media
/// type; its own structure still makes it Markdown, its flowchart, sequence and ER fences read
/// by the one renderer. A citation and the finder reach the same full-size render. Plain prose
/// with a `#` comment line later on stays text.
#[test]
fn a_titled_markdown_artifact_renders_its_diagrams_from_every_route() {
    let mut feed = Feed::default();
    let report = artifact(
        "T-9",
        1,
        ArtifactKind::Report,
        "Worker submits → Gate → Review → Land",
        1,
    );
    let text = "## Pipeline\n\n```mermaid\nflowchart LR\n  W[Worker submits] --> G{Gate} --> R[Review] --> L[Land]\n```\n\n```mermaid\nsequenceDiagram\n  Worker->>corrald: submit\n  corrald-->>Worker: gate receipt\n```\n\n```mermaid\nerDiagram\n  TASK ||--o{ ARTIFACT : records\n```\n\n```rust\nfn main() {}\n```\n";
    served_as(&mut feed, &report, "", text, false);
    let full = content(&feed, &report, Form::Desktop, true);
    let Body::Markdown(words) = &full.body else {
        panic!(
            "a titled Markdown report renders as Markdown: {:?}",
            full.body
        );
    };
    let fences: Vec<_> = nsh_base::text_view::Parser::new()
        .markdown(words)
        .blocks
        .into_iter()
        .filter_map(|block| match block {
            nsh_base::text_view::Block::CodeBlock(code) => Some(code),
            _ => None,
        })
        .collect();
    let kinds: Vec<_> = fences
        .iter()
        .filter(|code| mermaid::is_mermaid(code.language.as_deref()))
        .map(|code| mermaid::parse(&code.code).map(|diagram| diagram.edges.len()))
        .collect();
    assert_eq!(kinds, [Ok(3), Ok(2), Ok(1)]);
    assert_eq!(fences[3].language.as_deref(), Some("rust"));

    let target = Query::parse("task:T-9").request(false);
    served(&mut feed, &target, vec![vec![report.clone()]], false);
    let mut state = State::default();
    state.act(
        Action::Cite {
            reference: "T-9:1".into(),
            section: None,
        },
        &feed,
    );
    let cited = page(&state, &feed, Form::Desktop)
        .full
        .expect("the citation opens it");
    assert_eq!(cited.content, full);
    state.act(Action::Close, &feed);
    state.act(Action::Open("T-9:1".into()), &feed);
    let found = page(&state, &feed, Form::Desktop)
        .full
        .expect("the finder opens it");
    assert_eq!(found.content, full);

    let prose = artifact("T-9", 2, ArtifactKind::Report, "Run notes", 2);
    served_as(
        &mut feed,
        &prose,
        "",
        "Ran the gate.\n# not a heading here\n",
        false,
    );
    assert!(matches!(
        content(&feed, &prose, Form::Desktop, true).body,
        Body::Text(_)
    ));

    // Only the title varies, with the media type corrald derives from it: the same Markdown
    // bytes render the same, whatever the title says; the same script stays text.
    let pipeline = "Build steps:\n\n```mermaid\nflowchart LR\n  A[Build] --> B[Gate]\n```\n";
    let script = "# Install\nset -e\n# Then build\nmake\n";
    for (id, (title, media)) in [
        ("Build pipeline", ""),
        ("Build pipeline v1.0rc1", ""),
        ("Build pipeline.txt", "text/plain"),
        ("Build pipeline.log", "text/plain"),
        ("Build pipeline.html", "text/html"),
        ("pipeline.md", "text/markdown"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = 10 + 2 * id as u32;
        let doc = artifact("T-9", id, ArtifactKind::Report, title, 3);
        served_as(&mut feed, &doc, media, pipeline, false);
        let Body::Markdown(words) = content(&feed, &doc, Form::Desktop, true).body else {
            panic!("{title:?} renders its Markdown");
        };
        assert_eq!(words, pipeline, "{title:?}");
        let setup = artifact("T-9", id + 1, ArtifactKind::Evidence, title, 3);
        served_as(&mut feed, &setup, media, script, false);
        let body = content(&feed, &setup, Form::Desktop, true).body;
        let expected_log = title.ends_with(".log");
        assert!(
            matches!(&body, Body::Text(_)) && !expected_log
                || matches!(&body, Body::Log(_)) && expected_log,
            "{title:?}: a script's `#` comments are not Markdown: {body:?}"
        );
    }
    // A Markdown file of a heading and plain prose, under a custom title: the catalog's format
    // hint, recorded from its `.md` path, renders it; the same bytes with no hint are read by
    // their structure alone, where a lone `#` line may be a script's comment, so they stay text.
    let prose = "# Review\n\nThe build passed.\n";
    for (id, (title, media)) in [("Review notes", ""), ("Review notes.txt", "text/plain")]
        .into_iter()
        .enumerate()
    {
        let id = 40 + 2 * id as u32;
        let file = artifact("T-9", id, ArtifactKind::Report, title, 5);
        served_with(&mut feed, &file, media, Some("markdown"), prose, false);
        let Body::Markdown(words) = content(&feed, &file, Form::Desktop, true).body else {
            panic!("{title:?}: a Markdown file renders its heading");
        };
        assert_eq!(words, prose);
        let bytes = artifact("T-9", id + 1, ArtifactKind::Report, title, 5);
        served_with(&mut feed, &bytes, media, None, prose, false);
        assert!(matches!(
            content(&feed, &bytes, Form::Desktop, true).body,
            Body::Text(_)
        ));
    }
    // A file recorded as another format is still Markdown by its structure (a closed fence).
    let other = artifact("T-9", 50, ArtifactKind::Report, "Build pipeline", 5);
    served_with(&mut feed, &other, "", Some("other"), pipeline, false);
    assert!(matches!(
        content(&feed, &other, Form::Desktop, true).body,
        Body::Markdown(_)
    ));
    // A recorded log stays a log even when a line looks like Markdown structure.
    let gate = artifact("T-9", 30, ArtifactKind::Gate, "Gate attempt", 4);
    served_as(&mut feed, &gate, "", "# gate\n- step: pass\n", false);
    assert!(matches!(
        content(&feed, &gate, Form::Desktop, true).body,
        Body::Log(_)
    ));
}
