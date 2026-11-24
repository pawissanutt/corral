//! Fixed checked lenses for the explicit preview, including a stale anchor and a missing state.

use corral_types::lenses::{
    Lens, LensAnchor, LensDiagnostic, LensEntry, LensImpact, LensLocation, LensMessage, LensNode,
    LensResult, LensStatus, LensTransition, StatesFrom,
};
use corral_types::{
    EntryKind, NewTask, Status, SystemLens, SystemLensSummary, SystemLenses, SystemStatus,
};

const HEAD: &str = "85b8f96031c4e2a7d9b0";
const READ_AT: &str = "3aafaafe81b35478a0e731955241f0d9caf1277a";
const DIR: &str = ".corral/lenses";
const TYPES: &str = "crates/corral-types/src/lib.rs";
const TASKS: &str = "crates/corral-engine/src/tasks.rs";
const DISPATCH: &str = "crates/corral-engine/src/dispatch.rs";
const REVIEW: &str = "crates/corral-engine/src/review.rs";
const LAND: &str = "crates/corral-engine/src/land.rs";
const PIPELINE: &str = "crates/corral-engine/src/pipeline.rs";
const CLI: &str = "crates/corral-next/src/main.rs";
const CLI_ARGS: &str = "crates/corral-next/src/args.rs";

const LIFECYCLE_ID: &str = "task-lifecycle";
const SUBMIT: (&str, &str, &str) = ("running", "review", "Submit work");

type Source = (&'static str, &'static str, Option<u64>);

fn source(path: &'static str, text: &'static str, line: u64) -> Source {
    (path, text, Some(line))
}

fn diagnostic(element: &str, message: impl Into<String>) -> LensDiagnostic {
    LensDiagnostic {
        element: element.into(),
        message: message.into(),
    }
}

fn edge_element(kind: &str, (from, to, label): (&str, &str, &str)) -> String {
    format!("{kind} {from:?} -> {to:?}: {label:?}")
}

#[derive(Default)]
struct Checks {
    locations: Vec<LensLocation>,
    failures: Vec<LensDiagnostic>,
    not_checked: Vec<LensDiagnostic>,
}

impl Checks {
    fn locate(&mut self, element: &str, path: &str, line: u64) {
        self.locations.push(LensLocation {
            element: element.into(),
            path: path.into(),
            line,
        });
    }

    fn anchor(&mut self, element: &str, (path, text, line): Source) -> LensAnchor {
        if let Some(line) = line {
            self.locate(element, path, line);
        } else {
            self.failures.push(diagnostic(
                element,
                format!("anchor text {text:?} not found in {path}"),
            ));
        }
        LensAnchor {
            path: path.into(),
            name: None,
            text: Some(text.into()),
        }
    }

    fn node(&mut self, kind: &str, id: &str, label: &str, at: Source) -> LensNode {
        LensNode {
            id: id.into(),
            label: label.into(),
            anchor: self.anchor(&format!("{kind} {id}"), at),
        }
    }

    fn result(self, lens: Lens, status: LensStatus, missing_states: Vec<String>) -> LensResult {
        LensResult {
            file: format!("{DIR}/{}.toml", lens.id()),
            lens: Some(lens),
            status,
            failures: self.failures,
            not_checked: self.not_checked,
            locations: self.locations,
            missing_states,
            extra_states: Vec::new(),
        }
    }
}

fn lifecycle() -> LensResult {
    let mut checks = Checks::default();
    checks.locate("states_from", TYPES, 23);
    let mut states = [
        ("idea", "Idea", r#"Self::Idea => "idea","#),
        ("backlog", "Backlog", r#"Self::Backlog => "backlog","#),
        (
            "ready",
            "Ready",
            "Self::Ready => \"ready\",\n            Self::Running",
        ),
        ("running", "Running", r#"Self::Running => "running","#),
        (
            "blocked",
            "Blocked",
            "Self::Blocked => \"blocked\",\n            Self::Review",
        ),
        (
            "review",
            "Review",
            "Self::Review => \"review\",\n            Self::Done",
        ),
        (
            "done",
            "Done",
            "Self::Done => \"done\",\n            Self::Dropped",
        ),
    ]
    .into_iter()
    .zip(54..)
    .map(|((id, label, text), line)| checks.node("state", id, label, source(TYPES, text, line)))
    .collect::<Vec<_>>();
    // The literal still locates this state even without a provider checking the definition name.
    if let Some(idea) = states.first_mut() {
        idea.anchor.name = Some("Status::Idea".into());
        checks.not_checked.push(diagnostic(
            "state idea",
            "name \"Status::Idea\" not checked: definitions not provided",
        ));
    }
    let planning = source(TASKS, "self.set_status(store, &mut task, status)?;", 202);
    let done = source(REVIEW, "set_lifecycle(&mut task, Status::Review);", 91);
    let dispatch = source(
        DISPATCH,
        "set_lifecycle(&mut task, Status::Running);\n            if actor == Actor::Owner",
        178,
    );
    let resume = source(
        DISPATCH,
        "if task.status == Status::Blocked {\n                set_lifecycle(&mut task, Status::Running);",
        290,
    );
    let transitions = [
        (
            ("idea", "backlog", "Queue"),
            Some("The task is in a planning status"),
            planning,
        ),
        (("backlog", "ready", "Mark ready"), None, planning),
        (
            ("ready", "running", "Dispatch"),
            Some("A worker run starts successfully"),
            dispatch,
        ),
        (
            ("running", "blocked", "Record a blocker"),
            None,
            source(TASKS, "set_lifecycle(task, Status::Blocked);", 360),
        ),
        (("blocked", "running", "Reply and resume"), None, resume),
        (SUBMIT, None, done),
        (
            ("blocked", "review", "Submit work"),
            Some("The worker has changes to submit"),
            done,
        ),
        (
            ("review", "running", "Request changes"),
            None,
            source(REVIEW, "set_lifecycle(task, Status::Running);", 560),
        ),
        (
            ("review", "done", "Land"),
            Some("The merge succeeds"),
            source(LAND, "set_lifecycle(&mut task, Status::Done);", 206),
        ),
    ]
    .into_iter()
    .map(|(edge @ (from, to, label), guard, at)| LensTransition {
        from: from.into(),
        to: to.into(),
        label: label.into(),
        guard: guard.map(str::to_owned),
        anchor: checks.anchor(&edge_element("transition", edge), at),
    })
    .collect();
    checks.result(
        Lens::State {
            id: LIFECYCLE_ID.into(),
            title: "Task lifecycle".into(),
            read_at: READ_AT.into(),
            states_from: StatesFrom {
                path: TYPES.into(),
                name: "Status".into(),
            },
            states,
            transitions,
        },
        LensStatus::Incomplete,
        vec!["Dropped".into()],
    )
}

fn submit_work() -> LensResult {
    let mut checks = Checks::default();
    checks.locate("entry", CLI_ARGS, 125);
    let mut participants = [
        (
            "cli",
            "corral-next",
            source(CLI, "Command::Done { id, summary }", 242),
        ),
        (
            "engine",
            "Submission engine",
            source(REVIEW, "pub fn done(", 33),
        ),
        (
            "store",
            "Task store",
            source(REVIEW, "let (run, submitted) =", 34),
        ),
        (
            "pipeline",
            "Review pipeline",
            source(PIPELINE, "fn kick_pipeline(", 164),
        ),
    ]
    .into_iter()
    .map(|(id, label, at)| checks.node("participant", id, label, at))
    .collect::<Vec<_>>();
    if let Some(engine) = participants.iter_mut().find(|node| node.id == "engine") {
        engine.anchor.name = Some("Engine::done".into());
        checks.not_checked.push(diagnostic(
            "participant engine",
            "name \"Engine::done\" not checked: definitions not provided",
        ));
    }
    let messages = [
        (
            ("cli", "engine", "Submit with Op::Done"),
            None,
            source(
                CLI,
                "apply(&engine, out, Op::Done { id, summary }, false)?",
                242,
            ),
        ),
        (
            ("engine", "store", "Load the task and worker run"),
            Some("A task already in Review is not submitted again."),
            source(REVIEW, "let mut task = store.require_task(id)?;", 36),
        ),
        (
            ("engine", "store", "Record the submitted worker run"),
            None,
            source(REVIEW, "store.update_run(&run)?;", 90),
        ),
        (
            ("engine", "store", "Save the transition to Review"),
            None,
            source(REVIEW, "set_lifecycle(&mut task, Status::Review);", 91),
        ),
        (
            (
                "engine",
                "pipeline",
                "Kick the pipeline after the transaction",
            ),
            Some("Only a new submission kicks the pipeline."),
            (REVIEW, "self.start_review_pipeline(id)?;", None),
        ),
    ]
    .into_iter()
    .map(|(edge @ (from, to, label), note, at)| LensMessage {
        from: from.into(),
        to: to.into(),
        label: label.into(),
        note: note.map(str::to_owned),
        anchor: checks.anchor(&edge_element("message", edge), at),
    })
    .collect();
    checks.result(
        Lens::Sequence {
            id: "submit-work".into(),
            title: "What happens when a worker submits work".into(),
            read_at: READ_AT.into(),
            entry: Some(LensEntry {
                kind: EntryKind::Command,
                name: "done".into(),
            }),
            participants,
            messages,
        },
        LensStatus::Stale,
        Vec::new(),
    )
}

fn known_project(project: &str) -> Result<(), String> {
    match project {
        "corral" | "site" => Ok(()),
        _ => Err(format!("not_found: project {project}")),
    }
}

pub(super) fn results(project: &str) -> Result<Vec<LensResult>, String> {
    known_project(project)?;
    Ok(if project == "corral" {
        vec![lifecycle(), submit_work()]
    } else {
        Vec::new()
    })
}

fn find(project: &str, id: &str) -> Result<LensResult, String> {
    results(project)?
        .into_iter()
        .find(|item| item.file == id || item.lens.as_ref().is_some_and(|lens| lens.id() == id))
        .ok_or_else(|| "not_found: lens not found".into())
}

pub(super) fn list(project: &str) -> Result<SystemLenses, String> {
    let rows = results(project)?
        .into_iter()
        .map(|item| SystemLensSummary {
            file: item.file,
            id: item.lens.as_ref().map(|lens| lens.id().into()),
            title: item.lens.as_ref().map(|lens| lens.title().into()),
            kind: item.lens.as_ref().map(Lens::kind),
            status: item.status,
        })
        .collect();
    Ok(SystemLenses {
        project: project.into(),
        head: Some(HEAD.into()),
        status: SystemStatus::Ready,
        error: None,
        omitted_rows: 0,
        rows,
    })
}

pub(super) fn lens(project: &str, id: &str, at: Option<&str>) -> Result<SystemLens, String> {
    Ok(SystemLens {
        project: project.into(),
        // The preview uses the same checked diagram when opened from a sample review's head.
        commit: Some(at.unwrap_or(HEAD).into()),
        status: SystemStatus::Ready,
        error: None,
        lens: Some(find(project, id)?),
    })
}

pub(super) fn impact(result: LensResult, files: &[(String, u64, u64)]) -> Option<LensImpact> {
    if files.is_empty() {
        return None;
    }
    let changed = |anchor: &str| files.iter().any(|(path, _, _)| path == anchor);
    let lens = result.lens?;
    let mut touched: Vec<_> = result
        .locations
        .into_iter()
        .filter(|location| changed(&location.path))
        .map(|location| location.element)
        .collect();
    // Checked locations omit stale anchors, but a change to their source still affects the lens.
    if let Lens::Sequence { messages, .. } = &lens {
        for message in messages
            .iter()
            .filter(|message| changed(&message.anchor.path))
        {
            let key = edge_element("message", (&message.from, &message.to, &message.label));
            if !touched.contains(&key) {
                touched.push(key);
            }
        }
    }
    if touched.is_empty() {
        if lens.id() != LIFECYCLE_ID {
            return None;
        }
        // Most sample diffs touch app files rather than the engine; keep one example visible there.
        touched.push(edge_element("transition", SUBMIT));
    }
    Some(LensImpact {
        path: result.file,
        id: lens.id().into(),
        title: lens.title().into(),
        touched,
        added: Vec::new(),
        removed: Vec::new(),
    })
}

fn task(project: &str, title: String, body: String, ready: bool) -> NewTask {
    NewTask {
        project: Some(project.into()),
        title,
        body,
        status: if ready {
            Status::Ready
        } else {
            Status::Backlog
        },
        ..NewTask::default()
    }
}

pub(super) fn draft_task(
    project: &str,
    kind: &str,
    about: &str,
    ready: bool,
) -> Result<NewTask, String> {
    let shape = match kind {
        "state" => "Point `states_from` to the source enum and include each of its variants.",
        "sequence" => "List messages in execution order and name the entry point when known.",
        _ => return Err("lens kind must be state or sequence".into()),
    };
    let about = about.trim();
    let subject = about
        .lines()
        .next()
        .ok_or("lens draft requires a nonempty subject")?;
    known_project(project)?;
    let title = format!("Draft {kind} lens: {subject}");
    let body = format!(
        "Draft a {kind} lens about the following behaviour:\n\n{about}\n\n\
         Add one `.toml` lens under `{DIR}` with a new ID and path. Anchor each element to the \
         source and record the inspected commit as `read_at`. {shape}\n\n\
         Run `corral-next lens check` at the task head before submitting only the lens change."
    );
    Ok(task(project, title, body, ready))
}

pub(super) fn repair_task(project: &str, id: &str, ready: bool) -> Result<NewTask, String> {
    let result = find(project, id).map_err(|_| format!("no lens {id:?} in project {project:?}"))?;
    let lens = result
        .lens
        .ok_or_else(|| format!("cannot identify lens {id:?}"))?;
    let id = lens.id();
    let title = format!("Repair lens: {} ({id})", lens.title());
    let mut body = format!(
        "Repair the existing {} lens `{id}` in `{}`. Keep its ID and verify its anchors and \
         elements against the code.\n\n## Failed checks at {HEAD} ({})\n",
        lens.kind().as_str(),
        result.file,
        result.status.as_str()
    );
    for failure in result.failures {
        body.push_str(&format!("- {}: {}\n", failure.element, failure.message));
    }
    for state in result.missing_states {
        body.push_str(&format!("- Missing state: {state}\n"));
    }
    for state in result.extra_states {
        body.push_str(&format!("- Extra state: {state}\n"));
    }
    if !result.not_checked.is_empty() {
        body.push_str("\n## Not checked\n");
        for skipped in result.not_checked {
            body.push_str(&format!("- {}: {}\n", skipped.element, skipped.message));
        }
    }
    body.push_str(
        "\nRun `corral-next lens check` at the task head before submitting only the repair.",
    );
    Ok(task(project, title, body, ready))
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_types::{SystemImpactCategory, SystemReviewRow};

    #[test]
    fn preview_review_marks_stale_message_without_resolved_location() {
        let review = super::super::system_review::review(
            "T-1",
            "corral",
            "base",
            "head",
            &[(REVIEW.into(), 1, 1)],
            None,
        )
        .unwrap();
        let sample = submit_work();
        let Lens::Sequence { messages, .. } = sample.lens.unwrap() else {
            panic!("expected a sequence lens");
        };
        let stale = messages.last().unwrap();
        let name = format!(
            "message {:?} -> {:?}: {:?}",
            stale.from, stale.to, stale.label
        );
        assert_eq!(sample.failures[0].element, name);
        assert!(
            !sample
                .locations
                .iter()
                .any(|location| location.element == name)
        );
        assert!(review.rows.iter().any(|row| matches!(row,
            SystemReviewRow::Lens { impact }
                if impact.path == sample.file && impact.touched.contains(&name)
        )));
        assert!(
            review
                .categories
                .iter()
                .any(|summary| summary.category == SystemImpactCategory::Lenses
                    && summary.provided
                    && summary.total == 2)
        );
    }
}
