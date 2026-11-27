//! Explicit sample data: render the same recipes as the reviewed repository, with a few
//! sample observations. Live mode never consults these examples.
use corral_types::lenses::{
    EvidenceAnchor, InboundReference, LensDiagnostic, LensExcerpt, LensImpact, LensLocation,
    LensResult, LensStatus, RepoRecipe,
};
use corral_types::{
    NewTask, ReferenceBasis, ReferenceKind, Status, SystemLens, SystemLensSummary, SystemLenses,
    SystemStatus,
};
const HEAD: &str = "85b8f96031c4e2a7d9b085b8f96031c4e2a7d9b0";
const DIR: &str = ".corral/lenses";
macro_rules! sample {
    ($name:literal) => {
        include_str!(concat!("../../../../../.corral/lenses/", $name, ".json"))
    };
}

fn known_project(project: &str) -> Result<(), String> {
    match project {
        "corral" | "site" => Ok(()),
        _ => Err(format!("not_found: project {project}")),
    }
}

pub(super) fn results(project: &str) -> Result<Vec<LensResult>, String> {
    known_project(project)?;
    if project != "corral" {
        return Ok(Vec::new());
    }
    [
        sample!("submit-to-land"),
        sample!("submit-work"),
        sample!("engine-dependencies"),
        sample!("task-records"),
        sample!("task-lifecycle"),
    ]
    .into_iter()
    .map(|src| {
        let recipe = RepoRecipe::from_json(src)?;
        let mut result = LensResult {
            file: format!("{DIR}/{}.json", recipe.id),
            lens: None,
            status: LensStatus::Current,
            failures: vec![],
            locations: vec![],
            not_checked: vec![],
            missing_states: vec![],
            extra_states: vec![],
            derived: vec![],
        };
        for b in &recipe.evidence {
            if let EvidenceAnchor::Text { path, .. } = &b.anchor {
                if recipe.id == "submit-work" && b.element == "advance" {
                    result.failures.push(LensDiagnostic {
                        element: b.element.clone(),
                        message: "Anchor text not found in this sample revision".into(),
                    });
                    result.status = LensStatus::Stale;
                } else {
                    result.locations.push(LensLocation {
                        element: b.element.clone(),
                        path: path.clone(),
                        line: 42,
                    });
                }
            } else {
                result.not_checked.push(LensDiagnostic {
                    element: b.element.clone(),
                    message: "Not checked in the sample".into(),
                });
            }
        }
        if recipe.id == "submit-to-land" {
            result.derived.push(InboundReference {
                element: "gate".into(),
                reference: corral_types::facts::Reference {
                    from: "crates/corral-engine/src/pipeline.rs".into(),
                    to: "crates/corral-engine/src/review.rs".into(),
                    kind: ReferenceKind::Use,
                    basis: ReferenceBasis::Syntax,
                    count: 1,
                },
            });
        }
        result.lens = Some(recipe);
        Ok(result)
    })
    .collect()
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
            layout: item.lens.as_ref().map(|l| l.layout),
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
    let lens = find(project, id)?;
    Ok(SystemLens {
        project: project.into(),
        // The preview uses the same checked diagram when opened from a sample review's head.
        commit: Some(at.unwrap_or(HEAD).into()),
        status: SystemStatus::Ready,
        error: None,
        excerpts: excerpts(&lens),
        lens: Some(lens),
    })
}

/// The sample has no files: an element's excerpt is its anchor text's lines at its located
/// line, and one anchored without text has none, as a source corrald could not read.
fn excerpts(result: &LensResult) -> Vec<LensExcerpt> {
    result
        .locations
        .iter()
        .filter_map(|loc| {
            let binding = result.lens.as_ref()?.evidence.iter().find(|e| {
                e.element == loc.element
                    && matches!(&e.anchor,EvidenceAnchor::Text{path,..} if path==&loc.path)
            })?;
            let EvidenceAnchor::Text { text, .. } = &binding.anchor else {
                return None;
            };
            Some(LensExcerpt {
                element: loc.element.clone(),
                path: loc.path.clone(),
                line: loc.line,
                first: loc.line,
                lines: text.lines().map(str::to_owned).collect(),
            })
        })
        .collect()
}

pub(super) fn impact(result: LensResult, files: &[(String, u64, u64)]) -> Option<LensImpact> {
    let lens = result.lens?;
    let mut touched = Vec::new();
    for e in &lens.evidence {
        let path = match &e.anchor {
            EvidenceAnchor::Text { path, .. }
            | EvidenceAnchor::Path { path }
            | EvidenceAnchor::Symbol { path, .. } => path,
            _ => continue,
        };
        if files.iter().any(|(f, _, _)| f == path) && !touched.contains(&e.element) {
            touched.push(e.element.clone());
        }
    }
    if touched.is_empty() && !files.is_empty() && lens.id == "task-lifecycle" {
        touched.push("submit".into());
    }
    (!touched.is_empty()).then_some(LensImpact {
        path: result.file,
        id: lens.id,
        title: lens.title,
        touched,
        added: vec![],
        removed: vec![],
        touched_base: vec![],
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
        "graph" => "Use stable IDs and directed links.",
        "ordered_lanes" => "List edges in execution order.",
        _ => return Err("lens kind must be graph or ordered_lanes".into()),
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
         Add one `.json` lens under `{DIR}` with a new ID and path. Anchor each element to the \
         source using typed evidence bindings. {shape}\n\n\
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
        lens.layout.as_str(),
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
    #[test]
    fn impact_includes_unlocated_evidence_in_sample() {
        let sample = find("corral", "submit-work").unwrap();
        let stale = &sample.failures[0].element;
        assert!(!sample.locations.iter().any(|l| &l.element == stale));
        assert!(
            impact(
                sample.clone(),
                &[
                    ("crates/corral-engine/src/review.rs".into(), 1, 1),
                    ("crates/corral-engine/src/pipeline.rs".into(), 1, 1)
                ]
            )
            .unwrap()
            .touched
            .contains(stale)
        );
    }
}
