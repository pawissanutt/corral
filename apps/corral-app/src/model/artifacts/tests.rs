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
        query.request(),
        DetailRef::Artifacts {
            filter: Some("coverage".into()),
            narrowing: Some(ArtifactNarrowing {
                kind: Some("report".into()),
                task: Some("T-10".into()),
                project: None,
            }),
        }
    );
    assert_eq!(
        Query::parse("gate log project:si").request(),
        DetailRef::Artifacts {
            filter: Some("gate log".into()),
            narrowing: Some(ArtifactNarrowing {
                project: Some("si".into()),
                ..ArtifactNarrowing::default()
            }),
        }
    );
    assert_eq!(
        Query::parse("  ").request(),
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
    let target = Query::parse(&state.query).request();
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
    let target = Query::parse("").request();
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
    let target = Query::parse("").request();
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
