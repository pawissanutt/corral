use corral_protocol::detail::*;
use corral_protocol::session_host::{
    ChildAction, JobAction, SessionAction, SessionActionRequest, SessionChildAct, SessionJobAct,
    SessionModel,
};

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::id::Id;

const BUILD: &str = super::super::sessions::BUILD;

fn sample() -> Feed {
    super::super::feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None)
}

fn read(feed: &mut Feed, target: DetailRef) -> DetailBody {
    super::super::detail(feed, &target, None)
        .unwrap_or_else(|why| panic!("{target:?} was not served: {why}"));
    feed.detail(&target)
        .and_then(|cache| cache.body())
        .cloned()
        .unwrap_or_else(|| panic!("{target:?} has no body"))
}

fn run(feed: &mut Feed, key: &str, effect: Effect) -> Result<(), String> {
    super::super::effect(feed, &effect)?;
    match feed
        .operations
        .get(key)
        .and_then(|operation| operation.error.clone())
    {
        Some(error) => Err(error.message),
        None => Ok(()),
    }
}

/// Each part the app reads of the sample build was left "not read yet" when the preview did not
/// answer it: every one is served, and a job's output walks to its older page.
#[test]
fn every_part_of_the_sample_build_is_served() {
    let mut feed = sample();
    let id = BUILD.to_owned();
    let changes = |path: Option<&str>| match path {
        Some(path) => DetailRef::SessionChangeFile {
            id: id.clone(),
            comparison: ChangesComparison::Session,
            scope: None,
            path: path.into(),
        },
        None => DetailRef::SessionChanges {
            id: id.clone(),
            comparison: ChangesComparison::Session,
            scope: None,
        },
    };
    for target in [
        DetailRef::SessionCards { id: id.clone() },
        DetailRef::SessionModels { id: id.clone() },
        DetailRef::SessionChild {
            id: id.clone(),
            child: "docs".into(),
        },
        DetailRef::Session {
            id: format!("{id}/child/migration"),
        },
        DetailRef::SessionJobs { id: id.clone() },
        changes(None),
        changes(Some("line.txt")),
        DetailRef::SessionUsage { id: id.clone() },
        DetailRef::SessionContext { id: id.clone() },
        DetailRef::SessionPlan { id: id.clone() },
        DetailRef::SessionEvents { id: id.clone() },
        DetailRef::SessionCommands { id: id.clone() },
        DetailRef::BuildBoard { id: id.clone() },
        DetailRef::BuildRecord { id: id.clone() },
    ] {
        read(&mut feed, target);
    }
    let output = DetailRef::SessionJobOutput {
        id: id.clone(),
        job: "j1-1".into(),
    };
    read(&mut feed, output.clone());
    let older = feed
        .detail(&output)
        .and_then(|cache| cache.pages.first())
        .and_then(|newest| newest.next.clone())
        .expect("the running job's newest output names its older page");
    super::super::detail(&mut feed, &output, Some(&older)).unwrap();
    let Some(DetailBody::SessionJobOutput(page)) = feed.detail(&output).and_then(|c| c.body())
    else {
        panic!("the walked output has no body");
    };
    assert_eq!(page.offset, 0);
    assert_eq!(page.text.len() as u64, page.bytes);
}

/// Controls stayed "on their way" when the preview had no answer for them: each changes what
/// the build's parts serve as the host would, and Export is refused with the host's reason.
#[test]
fn the_sample_builds_actions_change_what_its_parts_serve() {
    let mut feed = sample();
    let id = BUILD.to_owned();
    run(
        &mut feed,
        "cancel",
        Effect::SessionChild {
            key: Id::new("cancel"),
            act: SessionChildAct {
                id: id.clone(),
                child: "migration".into(),
                action: ChildAction::Cancel,
            },
        },
    )
    .unwrap();
    let DetailBody::SessionChild(child) = read(
        &mut feed,
        DetailRef::SessionChild {
            id: id.clone(),
            child: "migration".into(),
        },
    ) else {
        panic!("the child detail is not a child");
    };
    assert_eq!(child.state, "cancelled");

    run(
        &mut feed,
        "stop",
        Effect::SessionJob {
            key: Id::new("stop"),
            act: SessionJobAct {
                id: id.clone(),
                job: "j1-1".into(),
                action: JobAction::Stop,
            },
        },
    )
    .unwrap();
    let DetailBody::SessionJobs(jobs) = read(&mut feed, DetailRef::SessionJobs { id: id.clone() })
    else {
        panic!("the jobs part is not jobs");
    };
    assert!(jobs.jobs.iter().all(|job| job.state != "running"));

    let model = "anthropic/claude-opus-4-8";
    run(
        &mut feed,
        "model",
        Effect::SessionModel {
            key: Id::new("model"),
            request: SessionModel {
                id: id.clone(),
                model: model.into(),
                effort: None,
            },
        },
    )
    .unwrap();
    assert_eq!(
        feed.assistants
            .get(BUILD)
            .and_then(|row| row.model.as_deref()),
        Some(model)
    );

    run(
        &mut feed,
        "compact",
        Effect::SessionAction {
            key: Id::new("compact"),
            request: SessionActionRequest {
                id: id.clone(),
                action: SessionAction::Compact { instructions: None },
            },
        },
    )
    .unwrap();
    let DetailBody::SessionPlan(plan) = read(&mut feed, DetailRef::SessionPlan { id: id.clone() })
    else {
        panic!("the plan part is not a plan");
    };
    assert!(plan.stale);

    let export = Effect::SessionAction {
        key: Id::new("export"),
        request: SessionActionRequest {
            id,
            action: SessionAction::Export,
        },
    };
    assert!(run(&mut feed, "export", export).is_err());
}
