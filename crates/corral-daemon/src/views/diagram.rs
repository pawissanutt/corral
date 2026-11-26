//! Evidence for a single selected element of a published, immutable recipe.
use std::{io::Read, path::Path};

use anyhow::{Context, Result, ensure};
use corral_engine::{Engine, Role};
use corral_facts::{CommittedExcerpt, Revision, committed_file, resolve_local_commit};
use corral_protocol::{
    detail::*,
    diagram::{self, DiagramSubject, EntityRequest, SourceRequest, WorkflowRecipe},
};
use corral_types::{Artifact, ReadStatus};
use sha2::{Digest, Sha256};

use super::{NotFound, citation, details, fingerprint, system};
use crate::state::Shared;

const RELATED_MAX: usize = 16;

pub(super) fn read(
    shared: &Shared,
    engine: &Engine,
    query: &DetailGet,
) -> Result<Option<DetailReply>> {
    let body = match &query.target {
        DetailRef::ArtifactDiagram { reference } => {
            no_page(query)?;
            let (artifact, result, _) = load(engine, reference, None)?;
            DetailBody::ArtifactDiagram(ArtifactDiagramDetail { artifact, result })
        }
        DetailRef::ArtifactDiagramElement {
            reference,
            sha256,
            element,
        } => {
            no_page(query)?;
            ensure!(element.len() <= 64, NotFound("diagram element not found"));
            let (artifact, result, attached_seq) = load(engine, reference, Some(sha256))?;
            let DiagramResult::Ready { recipe } = result else {
                return Err(NotFound("valid diagram recipe not found").into());
            };
            DetailBody::ArtifactDiagramElement(select(
                shared,
                engine,
                artifact,
                attached_seq,
                recipe,
                element,
            )?)
        }
        _ => return Ok(None),
    };
    let etag = fingerprint(&(&query.target, &body))?;
    details::reply(query, engine.store.latest_seq()?, etag, body, None).map(Some)
}

fn no_page(query: &DetailGet) -> Result<()> {
    ensure!(
        query.page.is_none(),
        "invalid: diagram details do not accept a page token"
    );
    Ok(())
}

fn load(
    engine: &Engine,
    reference: &str,
    digest: Option<&str>,
) -> Result<(Artifact, DiagramResult, Option<i64>)> {
    let (mut artifact, blob) = citation::cited(engine, reference)?;
    ensure!(
        artifact.reference() == reference,
        NotFound("artifact not found")
    );
    ensure!(
        digest.is_none_or(|digest| digest == artifact.sha256),
        NotFound("artifact digest no longer matches")
    );
    // Project-library rows carry their owner in the catalog; task rows get the
    // current project from the owning task, not from the recipe or replacement.
    let mut attached_seq = None;
    if artifact.project.is_none()
        && let Some(task) = engine.store.get_task(&artifact.task)?
    {
        artifact.project = task.project;
        if let Some((run, seq)) = engine.store.task_artifact_run(&artifact)? {
            artifact.run = Some(run);
            attached_seq = Some(seq);
        }
    }
    let mut bytes = Vec::new();
    if artifact.bytes <= diagram::MAX_RECIPE_BYTES as u64 {
        blob.take(diagram::MAX_RECIPE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .context("read bounded diagram artifact")?;
        ensure!(
            bytes.len() as u64 == artifact.bytes
                && format!("{:x}", Sha256::digest(&bytes)) == artifact.sha256,
            NotFound("artifact content does not match its catalog row")
        );
    }
    let result = if diagram::declares_schema(&bytes) {
        match diagram::validate(&bytes) {
            Ok(recipe) => DiagramResult::Ready { recipe },
            Err(diagnostic) => DiagramResult::Invalid { diagnostic },
        }
    } else {
        DiagramResult::NotRecipe
    };
    Ok((artifact, result, attached_seq))
}

fn select(
    shared: &Shared,
    engine: &Engine,
    artifact: Artifact,
    attached_seq: Option<i64>,
    recipe: WorkflowRecipe,
    id: &str,
) -> Result<DiagramElementDetail> {
    let element = recipe
        .nodes
        .into_iter()
        .find(|node| node.id == id)
        .map(|node| SelectedElement::Node { node })
        .or_else(|| {
            recipe
                .edges
                .into_iter()
                .find(|edge| edge.id == id)
                .map(|edge| SelectedElement::Edge { edge })
        })
        .context(NotFound("diagram element not found"))?;
    let bindings = match &element {
        SelectedElement::Node { node } => node.bindings.as_ref(),
        SelectedElement::Edge { edge } => edge.bindings.as_ref(),
    };
    let (source_request, entity_request) = bindings.map_or((None, None), |bindings| {
        (bindings.source.as_ref(), bindings.entity.as_ref())
    });
    let subject = recipe.subject;
    let (mut source, mut entity) = (SourceResult::NotProvided, EntityResult::NotProvided);
    let mut candidate = None;
    if source_request.is_some() || entity_request.is_some() || artifact.run.is_some() {
        match pin(engine, &artifact, &subject) {
            Ok((repo, rev, provider_enabled)) => {
                candidate = submission(engine, &artifact, &subject, attached_seq)?;
                if let Some(request) = source_request {
                    source = read_source(&repo, &rev, request);
                }
                if let Some(request) = entity_request {
                    entity = read_entity(shared, &repo, &rev, &subject, request, provider_enabled);
                }
            }
            Err(problem) => {
                let reason = problem.reason().to_owned();
                if source_request.is_some() {
                    source = match problem {
                        PinProblem::Missing => SourceResult::Missing {
                            reason: reason.clone(),
                        },
                        PinProblem::OutOfScope => SourceResult::Unavailable {
                            reason: reason.clone(),
                        },
                    };
                }
                if entity_request.is_some() {
                    entity = match problem {
                        PinProblem::Missing => EntityResult::Missing {
                            reason,
                            evidence: None,
                        },
                        PinProblem::OutOfScope => unavailable(reason, None),
                    };
                }
            }
        }
    }
    Ok(DiagramElementDetail {
        reference: artifact.reference(),
        sha256: artifact.sha256,
        subject,
        candidate,
        element,
        source,
        entity,
    })
}

enum PinProblem {
    OutOfScope,
    Missing,
}

impl PinProblem {
    fn reason(&self) -> &'static str {
        match self {
            Self::OutOfScope => "the subject must match the artifact's configured owning project",
            Self::Missing => "the exact pinned commit is not available locally as a Git commit",
        }
    }
}

fn pin(
    engine: &Engine,
    artifact: &Artifact,
    subject: &DiagramSubject,
) -> Result<(std::path::PathBuf, Revision, bool), PinProblem> {
    if artifact.project.as_deref() != Some(&subject.project) {
        return Err(PinProblem::OutOfScope);
    }
    let project = engine
        .config
        .projects
        .get(&subject.project)
        .ok_or(PinProblem::OutOfScope)?;
    let repo = engine.root.join(&project.path);
    let rev = resolve_local_commit(&repo, &subject.commit).map_err(|_| PinProblem::Missing)?;
    let provider_enabled = engine
        .config
        .extension_for_slot(&subject.project, corral_engine::config::Slot::Facts)
        .is_some();
    Ok((repo, rev, provider_enabled))
}

fn submission(
    engine: &Engine,
    artifact: &Artifact,
    subject: &DiagramSubject,
    attached_seq: Option<i64>,
) -> Result<Option<DiagramCandidate>> {
    let (Some(run), Some(attached_seq)) = (&artifact.run, attached_seq) else {
        return Ok(None);
    };
    if !engine
        .store
        .get_run(&artifact.task, run)?
        .is_some_and(|run| run.role == Role::Worker)
    {
        return Ok(None);
    }
    Ok(engine
        .store
        .submitted_revision_at(&artifact.task, run, &subject.commit, attached_seq)?
        .filter(|submitted| {
            submitted
                .project
                .as_ref()
                .is_none_or(|project| project == &subject.project)
        })
        .map(|submitted| DiagramCandidate {
            task: artifact.task.clone(),
            run: run.clone(),
            head: submitted.sha,
            base: submitted.base_sha,
        }))
}

fn read_source(repo: &Path, rev: &Revision, request: &SourceRequest) -> SourceResult {
    let span = request
        .span
        .as_ref()
        .map(|span| (u64::from(span.start), u64::from(span.end)));
    let result = committed_file(repo, rev, &request.path).and_then(|file| match file {
        Some(file) => file.excerpt(repo, span),
        None => Ok(CommittedExcerpt::Missing),
    });
    match result {
        Ok(CommittedExcerpt::Resolved { excerpt, truncated }) => {
            SourceResult::Resolved { excerpt, truncated }
        }
        Ok(CommittedExcerpt::Missing) => SourceResult::Missing {
            reason: "no matching regular committed file or source line".into(),
        },
        Ok(CommittedExcerpt::Unavailable) | Err(_) => SourceResult::Unavailable {
            reason: "committed source is unreadable as bounded UTF-8 text".into(),
        },
    }
}

fn read_entity(
    shared: &Shared,
    repo: &Path,
    rev: &Revision,
    subject: &DiagramSubject,
    request: &EntityRequest,
    provider_enabled: bool,
) -> EntityResult {
    let (path, name) = match request {
        EntityRequest::File { path } => (path, None),
        EntityRequest::Definition { path, name } => (path, Some(name)),
    };
    match committed_file(repo, rev, path) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return EntityResult::Missing {
                reason: "no matching regular committed file".into(),
                evidence: None,
            };
        }
        Err(_) => return unavailable("could not read the committed tree", None),
    }
    let snapshot = match shared
        .system
        .diagram_at(shared, &subject.project, &subject.commit)
    {
        Ok(snapshot) => snapshot,
        Err(error) if error.is::<crate::ops::WorkerBusy>() => {
            return unavailable("facts worker is busy; retry this selection", None);
        }
        Err(_) => return unavailable("system facts could not be requested", None),
    };
    if snapshot.building {
        return EntityResult::Pending;
    }
    let Some(tree) = snapshot
        .facts
        .as_deref()
        .filter(|_| snapshot.head.as_deref() == Some(rev.commit()))
    else {
        return unavailable("system facts for the pinned commit are unavailable", None);
    };
    let Some(file) = tree
        .files
        .binary_search_by(|file| file.path.cmp(path))
        .ok()
        .map(|index| &tree.files[index])
    else {
        return unavailable(
            "the committed file is not covered by the System snapshot",
            None,
        );
    };
    let evidence = EntityEvidence {
        coverage: tree.coverage.clone(),
        provider: system::provider(snapshot.provider.as_deref()),
        read_status: file.read_status,
        provider_pending: provider_enabled && snapshot.provider.is_none(),
    };
    if let Some(name) = name {
        return definition(file, name, evidence);
    }
    let mut omitted = 0;
    let mut related = Vec::new();
    for row in tree
        .file_references
        .iter()
        .filter(|row| row.to == *path && row.from != *path)
    {
        if related.len() == RELATED_MAX || !diagram::valid_path(&row.from) {
            omitted += 1;
        } else {
            related.push(DiagramRelated::FileReference {
                path: row.from.clone(),
                basis: row.basis,
                reference_kind: row.kind,
                count: row.count,
            });
        }
    }
    EntityResult::Resolved {
        location: EntityLocation::File,
        evidence,
        related,
        omitted,
    }
}

fn definition(file: &corral_facts::FileFact, name: &str, evidence: EntityEvidence) -> EntityResult {
    // File references never have symbol attribution, even for a resolved definition.
    if evidence.provider_pending {
        return EntityResult::Pending;
    }
    let Some(coverage) = evidence
        .provider
        .as_ref()
        .map(|provider| &provider.coverage)
        .filter(|coverage| coverage.provides("definitions"))
    else {
        return unavailable(
            "the facts provider did not supply definitions",
            Some(evidence),
        );
    };
    if file.read_status == ReadStatus::Failed && coverage.provides("files") {
        return unavailable(
            "the definition file was not read successfully",
            Some(evidence),
        );
    }
    let candidates = file.definitions.iter().filter(|def| def.name == name);
    if candidates.clone().any(|def| def.end_line > file.lines) {
        return unavailable(
            "provider definition is outside the committed file's lines",
            Some(evidence),
        );
    }
    let mut matches = candidates;
    if let Some(def) = matches.next() {
        if matches.any(|other| other != def) {
            return EntityResult::Ambiguous {
                reason: "multiple definitions match this name in the file".into(),
                evidence,
            };
        }
        return EntityResult::Resolved {
            location: EntityLocation::Definition {
                line: def.line,
                end_line: def.end_line,
            },
            evidence,
            related: Vec::new(),
            omitted: 0,
        };
    }
    if file.read_status != ReadStatus::Complete
        || coverage.dropped_unknown_files != 0
        || coverage
            .skipped
            .values()
            .chain(coverage.omitted.values())
            .any(|&total| total != 0)
    {
        return unavailable("definition coverage is incomplete", Some(evidence));
    }
    EntityResult::Missing {
        reason: "no matching definition in the covered file".into(),
        evidence: Some(evidence),
    }
}

fn unavailable(reason: impl Into<String>, evidence: Option<EntityEvidence>) -> EntityResult {
    EntityResult::Unavailable {
        reason: reason.into(),
        evidence,
    }
}
