//! A published recipe's evidence at its pinned commit beside the same bindings at one explicitly
//! chosen comparison commit. Both commits are resolved and pinned for the read; the artifact,
//! its recipe and its subject are never changed, and no change is claimed without evidence.
use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, ensure};
use corral_engine::{Engine, Role};
use corral_facts::{
    CommittedFile, Revision, map_span, on_first_parent, renames, resolve_local_commit,
};
use corral_protocol::{
    detail::*,
    diagram::{DiagramSubject, EntityRequest, SourceRequest, SourceSpan, WorkflowRecipe},
};
use corral_types::{Artifact, ReadStatus};

use super::{
    NotFound,
    diagram::{self, PinProblem},
    review,
};
use crate::state::Shared;

/// How far back a pinned head may be on the observed head's first-parent line.
const HEAD_HISTORY: usize = 10_000;

pub(super) fn compare(
    shared: &Shared,
    engine: &Engine,
    reference: &str,
    sha256: &str,
    target: &CompareTarget,
    element: Option<&str>,
) -> Result<DiagramCompareDetail> {
    ensure!(
        element.is_none_or(|id| id.len() <= 64),
        NotFound("diagram element not found")
    );
    let (artifact, result, _) = diagram::load(engine, reference, Some(sha256))?;
    let DiagramResult::Ready { recipe } = result else {
        return Err(NotFound("valid diagram recipe not found").into());
    };
    let selected = element
        .map(|id| diagram::find_element(&recipe, id).context(NotFound("diagram element not found")))
        .transpose()?;
    let subject = recipe.subject.clone();
    let mut detail = DiagramCompareDetail {
        reference: artifact.reference(),
        sha256: artifact.sha256.clone(),
        subject: subject.clone(),
        target: CompareResolution::Pending,
        elements: Vec::new(),
        selected: None,
    };
    let (repo, provider_enabled) = match diagram::project_scope(engine, &artifact, &subject) {
        Ok(scope) => scope,
        Err(problem) => {
            detail.target = unavailable(problem.reason());
            return Ok(detail);
        }
    };
    let (resolution, target_rev) = resolve(shared, engine, &artifact, &repo, &subject, target)?;
    detail.target = resolution;
    let Some(target_rev) = target_rev else {
        return Ok(detail);
    };
    let base_rev = resolve_local_commit(&repo, &subject.commit).ok();
    let mut sides = Sides {
        repo: &repo,
        base: base_rev.as_ref(),
        target: &target_rev,
        renames: None,
        files: BTreeMap::new(),
        texts: BTreeMap::new(),
    };
    // The summary is its own read; a selected element's read does only that element's work.
    if selected.is_none() {
        detail.elements = summary(&mut sides, &recipe);
    }
    if let Some(element) = selected {
        let links = links(engine, &artifact, &detail.target)?;
        detail.selected = Some(inspect(
            shared,
            &mut sides,
            &subject,
            provider_enabled,
            element,
            links,
        ));
    }
    Ok(detail)
}

fn unavailable(reason: impl Into<String>) -> CompareResolution {
    CompareResolution::Unavailable {
        reason: reason.into(),
    }
}

/// The comparison commit, resolved once and returned for the client to pin. An observed head is
/// System's existing observation, not a Git lookup of a moving ref on this read.
fn resolve(
    shared: &Shared,
    engine: &Engine,
    artifact: &Artifact,
    repo: &Path,
    subject: &DiagramSubject,
    target: &CompareTarget,
) -> Result<(CompareResolution, Option<Revision>)> {
    let observed = || -> Result<String, Box<CompareResolution>> {
        let Ok(snapshot) = shared.system.map(shared, &subject.project) else {
            return Err(Box::new(unavailable("the project's head is not observed")));
        };
        match snapshot.head {
            Some(head) => Ok(head),
            None if snapshot.building => Err(Box::new(CompareResolution::Pending)),
            None => {
                Err(Box::new(unavailable(snapshot.error.unwrap_or_else(|| {
                    "the project's head is not observed".into()
                }))))
            }
        }
    };
    let (origin, commit) = match target {
        CompareTarget::ObservedHead => match observed() {
            Ok(head) => (CompareOrigin::ObservedHead, head),
            Err(resolution) => return Ok((*resolution, None)),
        },
        CompareTarget::Candidate => match latest_submission(engine, artifact, subject)? {
            Ok(candidate) => (CompareOrigin::Candidate, candidate.head),
            Err(reason) => return Ok((unavailable(reason), None)),
        },
        CompareTarget::Pinned { origin, commit } => (*origin, commit.clone()),
    };
    let candidate = match origin {
        CompareOrigin::ObservedHead => None,
        CompareOrigin::Candidate => match recorded(engine, artifact, subject, &commit)? {
            Some(candidate) => Some(candidate),
            None => {
                return Ok((
                    unavailable(
                        "the commit is not a recorded submission of this task's latest worker run",
                    ),
                    None,
                ));
            }
        },
    };
    let Ok(rev) = resolve_local_commit(repo, &commit) else {
        return Ok((
            unavailable("the comparison commit is not available locally as a Git commit"),
            None,
        ));
    };
    // A pinned head is only ever one the project's head stood at: the current observation or a
    // recent commit on its first-parent line, never a merged side branch or an arbitrary commit
    // labelled as the head. A head rewritten past it no longer vouches for it.
    if let CompareTarget::Pinned {
        origin: CompareOrigin::ObservedHead,
        ..
    } = target
    {
        let head = match observed() {
            Ok(head) => head,
            Err(resolution) => return Ok((*resolution, None)),
        };
        let reached = resolve_local_commit(repo, &head)
            .and_then(|head| on_first_parent(repo, &rev, &head, HEAD_HISTORY))
            .unwrap_or(false);
        if !reached {
            return Ok((
                unavailable("the pinned commit is not on the project's observed head"),
                None,
            ));
        }
    }
    let current_candidate = match &candidate {
        Some(candidate) => latest_submission(engine, artifact, subject)?
            .is_ok_and(|latest| latest.head == candidate.head),
        None => false,
    };
    Ok((
        CompareResolution::Resolved {
            origin,
            commit,
            candidate,
            current_candidate,
        },
        Some(rev),
    ))
}

/// The artifact's task's latest worker submission, which its Changes and Impact describe.
fn latest_submission(
    engine: &Engine,
    artifact: &Artifact,
    subject: &DiagramSubject,
) -> Result<Result<DiagramCandidate, &'static str>> {
    if engine.store.get_task(&artifact.task)?.is_none() {
        return Ok(Err("a project library artifact has no task candidate"));
    }
    let Some(run) = engine
        .store
        .latest_run(&artifact.task, Role::Worker)?
        .filter(|run| run.summary.is_some())
    else {
        return Ok(Err("the artifact's task has no submitted worker run"));
    };
    let Some(submitted) = engine.store.submitted_revision(&artifact.task, &run.id)? else {
        return Ok(Err("the artifact's task has no recorded submission"));
    };
    if submitted
        .project
        .as_ref()
        .is_some_and(|project| project != &subject.project)
    {
        return Ok(Err("the task's submission belongs to another project"));
    }
    Ok(Ok(DiagramCandidate {
        task: artifact.task.clone(),
        run: run.id,
        head: submitted.sha,
        base: submitted.base_sha,
    }))
}

/// A pinned candidate must still be a recorded submission of the task's latest worker run.
fn recorded(
    engine: &Engine,
    artifact: &Artifact,
    subject: &DiagramSubject,
    commit: &str,
) -> Result<Option<DiagramCandidate>> {
    if engine.store.get_task(&artifact.task)?.is_none() {
        return Ok(None);
    }
    let Some(run) = engine.store.latest_run(&artifact.task, Role::Worker)? else {
        return Ok(None);
    };
    Ok(engine
        .store
        .submitted_revision_at(&artifact.task, &run.id, commit, i64::MAX)?
        .filter(|submitted| {
            submitted.sha == commit
                && submitted
                    .project
                    .as_ref()
                    .is_none_or(|project| project == &subject.project)
        })
        .map(|submitted| DiagramCandidate {
            task: artifact.task.clone(),
            run: run.id,
            head: submitted.sha,
            base: submitted.base_sha,
        }))
}

/// The candidate's diff and Impact, only while they describe the comparison commit.
fn links(
    engine: &Engine,
    artifact: &Artifact,
    target: &CompareResolution,
) -> Result<Option<Links>> {
    let CompareResolution::Resolved {
        origin: CompareOrigin::Candidate,
        current_candidate: true,
        commit,
        candidate: Some(candidate),
    } = target
    else {
        return Ok(None);
    };
    // The patch must be the compared submission's own: the same run, still at that head.
    Ok(review::submitted_paths(engine, &artifact.task)?
        .filter(|(run, _)| run.id == candidate.run)
        .filter(|_| {
            engine
                .store
                .submitted_revision(&artifact.task, &candidate.run)
                .ok()
                .flatten()
                .is_some_and(|submitted| &submitted.sha == commit)
        })
        .map(|(_, paths)| Links {
            task: artifact.task.clone(),
            head: commit.clone(),
            paths,
        }))
}

struct Links {
    task: String,
    head: String,
    paths: std::collections::BTreeSet<String>,
}

struct Sides<'a> {
    repo: &'a Path,
    base: Option<&'a Revision>,
    target: &'a Revision,
    /// Git's renames from base to target, found once per read when first needed.
    renames: Option<Result<BTreeMap<String, String>, ()>>,
    /// Tree lookups of this read, by side (`true` for the target) and path.
    files: BTreeMap<(bool, String), Result<Option<CommittedFile>, ()>>,
    /// Blob texts of this read, by object ID.
    texts: BTreeMap<String, Option<std::rc::Rc<str>>>,
}

impl Sides<'_> {
    /// One blob read per object in a read, however many elements bind it.
    fn text(&mut self, file: &CommittedFile) -> Option<std::rc::Rc<str>> {
        let repo = self.repo;
        self.texts
            .entry(file.blob().to_owned())
            .or_insert_with(|| file.text(repo).map(std::rc::Rc::from))
            .clone()
    }

    /// One tree lookup per side and path in a read, however many elements bind it.
    fn file(&mut self, target: bool, path: &str) -> Result<Option<CommittedFile>, ()> {
        let rev = if target { Some(self.target) } else { self.base };
        let (repo, Some(rev)) = (self.repo, rev) else {
            return Err(());
        };
        self.files
            .entry((target, path.to_owned()))
            .or_insert_with(|| corral_facts::committed_file(repo, rev, path).map_err(|_| ()))
            .clone()
    }

    fn renamed(&mut self, path: &str) -> Result<Option<String>, ()> {
        let Some(base) = self.base else {
            return Err(());
        };
        let renames = self
            .renames
            .get_or_insert_with(|| renames(self.repo, base, self.target).map_err(|_| ()));
        renames
            .as_ref()
            .map(|map| map.get(path).cloned())
            .map_err(|_| ())
    }
}

fn summary(sides: &mut Sides<'_>, recipe: &WorkflowRecipe) -> Vec<ElementSourceChange> {
    let nodes = recipe
        .nodes
        .iter()
        .map(|node| (&node.id, node.bindings.as_ref()));
    let edges = recipe
        .edges
        .iter()
        .map(|edge| (&edge.id, edge.bindings.as_ref()));
    nodes
        .chain(edges)
        .filter_map(|(id, bindings)| {
            let request = bindings?.source.as_ref()?;
            Some(ElementSourceChange {
                id: id.clone(),
                change: source_change(sides, request).0,
            })
        })
        .collect()
}

fn unknown(reason: &str) -> SourceChange {
    SourceChange::Unknown {
        reason: reason.into(),
    }
}

/// How the bound text changed, and where to read it at the target.
fn source_change(
    sides: &mut Sides<'_>,
    request: &SourceRequest,
) -> (SourceChange, Option<SourceRequest>) {
    let same = Some(request.clone());
    if sides.base.is_none() {
        return (
            unknown("the pinned commit is not available locally as a Git commit"),
            same,
        );
    }
    let (repo, target_commit) = (sides.repo, sides.target.commit().to_owned());
    let path = request.path.as_str();
    let (base_file, target_file) = match (sides.file(false, path), sides.file(true, path)) {
        (Ok(base), Ok(target)) => (base, target),
        _ => return (unknown("could not read a committed tree"), same),
    };
    let (base_file, target_file, target_path) = match (base_file, target_file) {
        (None, None) => {
            return (
                unknown("the bound file is not a regular committed file at either commit"),
                same,
            );
        }
        (None, Some(_)) => return (SourceChange::Added, same),
        (Some(base_file), Some(target_file)) => (base_file, target_file, path.to_owned()),
        (Some(base_file), None) => match sides.renamed(path) {
            Err(()) => return (unknown("Git's rename detection failed"), same),
            Ok(None) => return (SourceChange::Removed, None),
            Ok(Some(renamed)) => match sides.file(true, &renamed) {
                Ok(Some(target_file)) => (base_file, target_file, renamed),
                _ => return (unknown("the renamed file could not be read"), same),
            },
        },
    };
    let renamed = target_path != path;
    let at = |span: Option<SourceSpan>| SourceRequest {
        path: target_path.clone(),
        span,
    };
    let Some(span) = &request.span else {
        let change = match (base_file.blob() == target_file.blob(), renamed) {
            (true, false) => SourceChange::Unchanged,
            (true, true) => SourceChange::Moved {
                path: target_path.clone(),
                first: None,
            },
            (false, _) => SourceChange::Changed {
                path: target_path.clone(),
            },
        };
        return (change, Some(at(None)));
    };
    if base_file.blob() == target_file.blob() {
        let change = if renamed {
            SourceChange::Moved {
                path: target_path.clone(),
                first: Some(u64::from(span.start)),
            }
        } else {
            SourceChange::Unchanged
        };
        return (change, Some(at(Some(span.clone()))));
    }
    let (Some(before), Some(after)) = (sides.text(&base_file), sides.text(&target_file)) else {
        return (
            unknown("the committed source is unreadable as bounded UTF-8 text"),
            same,
        );
    };
    let before: Vec<&str> = before.lines().collect();
    let after: Vec<&str> = after.lines().collect();
    let first = span.start as usize - 1;
    let last = (span.end as usize).min(before.len());
    if first >= last {
        return (
            unknown("the bound span is not in the file at the pinned commit"),
            same,
        );
    }
    let bound = &before[first..last];
    if after.get(first..last) == Some(bound) {
        let change = if renamed {
            SourceChange::Moved {
                path: target_path.clone(),
                first: Some(u64::from(span.start)),
            }
        } else {
            SourceChange::Unchanged
        };
        return (change, Some(at(Some(span.clone()))));
    }
    let changed = SourceChange::Changed {
        path: target_path.clone(),
    };
    // Git's line diff of the two blobs says where the bound lines went.
    let mapped = map_span(
        repo,
        &base_file,
        &target_file,
        &target_commit,
        first as u64 + 1,
        last as u64,
    );
    match mapped {
        Err(_) => (unknown("Git's line diff of the bound file failed"), None),
        Ok(Some((start, end))) => {
            let end = end.min(start + 199);
            let place = SourceSpan {
                start: start as u32,
                end: end as u32,
            };
            let copies = after
                .windows(bound.len())
                .filter(|lines| *lines == bound)
                .count();
            // The same text is a move only when it is the target's one copy of it.
            if copies == 1 && after.get(start as usize - 1..end as usize) == Some(bound) {
                let moved = SourceChange::Moved {
                    path: target_path.clone(),
                    first: Some(start),
                };
                (moved, Some(at(Some(place))))
            } else {
                (changed, Some(at(Some(place))))
            }
        }
        // Every bound line was deleted there: a single exact copy elsewhere is a move; none,
        // or several, is not claimed as one.
        Ok(None) => {
            let mut copies = after
                .windows(bound.len())
                .enumerate()
                .filter(|(_, lines)| *lines == bound)
                .map(|(at, _)| at);
            match (copies.next(), copies.next()) {
                (Some(at_line), None) => {
                    let start = at_line as u32 + 1;
                    let place = SourceSpan {
                        start,
                        end: start + (last - first) as u32 - 1,
                    };
                    let moved = SourceChange::Moved {
                        path: target_path.clone(),
                        first: Some(u64::from(start)),
                    };
                    (moved, Some(at(Some(place))))
                }
                _ => (changed, None),
            }
        }
    }
}

fn inspect(
    shared: &Shared,
    sides: &mut Sides<'_>,
    subject: &DiagramSubject,
    provider_enabled: bool,
    element: SelectedElement,
    links: Option<Links>,
) -> ElementCompare {
    let (source_request, entity_request) = diagram::requests(&element);
    let (source_request, entity_request) = (source_request.cloned(), entity_request.cloned());
    let target_subject = DiagramSubject {
        project: subject.project.clone(),
        commit: sides.target.commit().to_owned(),
    };
    let missing_base = || SourceResult::Missing {
        reason: PinProblem::Missing.reason().into(),
    };
    let (base_source, target_source, source_change) = match &source_request {
        None => (
            SourceResult::NotProvided,
            SourceResult::NotProvided,
            SourceChange::NotProvided,
        ),
        Some(request) => {
            let (change, at_target) = source_change(sides, request);
            let base = match sides.base {
                Some(base) => diagram::read_source(sides.repo, base, request),
                None => missing_base(),
            };
            let target = match (&change, at_target) {
                (SourceChange::Removed, _) => SourceResult::Missing {
                    reason: "the file is absent at the comparison commit".into(),
                },
                (_, None) => SourceResult::Missing {
                    reason: "the bound lines were deleted at the comparison commit".into(),
                },
                (_, Some(at)) => diagram::read_source(sides.repo, sides.target, &at),
            };
            (base, target, change)
        }
    };
    let (base_entity, target_entity) = match &entity_request {
        None => (EntityResult::NotProvided, EntityResult::NotProvided),
        Some(request) => {
            let base = match sides.base {
                Some(base) => diagram::read_entity(
                    shared,
                    sides.repo,
                    base,
                    subject,
                    request,
                    provider_enabled,
                ),
                None => EntityResult::Missing {
                    reason: PinProblem::Missing.reason().into(),
                    evidence: None,
                },
            };
            let request = renamed_entity(sides, request);
            let target = diagram::read_entity(
                shared,
                sides.repo,
                sides.target,
                &target_subject,
                &request,
                provider_enabled,
            );
            (base, target)
        }
    };
    let entity_change = entity_change(&base_entity, &target_entity);
    let mut paths: Vec<&str> = Vec::new();
    if let Some(request) = &source_request
        && !matches!(
            source_change,
            SourceChange::Unchanged | SourceChange::NotProvided | SourceChange::Unknown { .. }
        )
    {
        paths.push(&request.path);
        if let SourceChange::Moved { path, .. } | SourceChange::Changed { path } = &source_change {
            paths.push(path);
        }
    }
    let links = match links {
        Some(Links {
            task,
            head,
            paths: patched,
        }) => {
            let mut links: Vec<CompareLink> = paths
                .into_iter()
                .filter(|path| patched.contains(*path))
                .map(|path| CompareLink::Diff {
                    task: task.clone(),
                    head: head.clone(),
                    path: path.to_owned(),
                })
                .collect();
            links.dedup();
            links.push(CompareLink::Impact { task, head });
            links
        }
        None => Vec::new(),
    };
    ElementCompare {
        element,
        base_source,
        target_source,
        source_change,
        base_entity,
        target_entity,
        entity_change,
        links,
    }
}

/// An entity binding at its renamed file, when Git found the file renamed at the target.
fn renamed_entity(sides: &mut Sides<'_>, request: &EntityRequest) -> EntityRequest {
    let path = match request {
        EntityRequest::File { path } | EntityRequest::Definition { path, .. } => path,
    };
    if sides.file(true, path).is_ok_and(|file| file.is_none())
        && let Ok(Some(renamed)) = sides.renamed(path)
    {
        return match request {
            EntityRequest::File { .. } => EntityRequest::File { path: renamed },
            EntityRequest::Definition { name, .. } => EntityRequest::Definition {
                path: renamed,
                name: name.clone(),
            },
        };
    }
    request.clone()
}

fn insufficient(reason: impl Into<String>) -> EntityChange {
    EntityChange::Insufficient {
        reason: reason.into(),
    }
}

/// A relationship delta is claimed only when both sides resolved, read the file completely with
/// the same basis, and listed every row they know.
/// The facts basis an entity was read on: the provider, whether it failed, and what it provides.
fn basis(evidence: &EntityEvidence) -> Option<(String, bool, Vec<String>, Vec<String>)> {
    evidence.provider.as_ref().map(|provider| {
        let mut provided = provider.coverage.provided.clone();
        let mut not_provided = provider.coverage.not_provided.clone();
        provided.sort();
        not_provided.sort();
        (
            provider.extension.clone(),
            provider.error.is_some(),
            provided,
            not_provided,
        )
    })
}

/// Built-in coverage and any provider's own skips, omissions and dropped files all count: a row
/// missing from either side must not read as a removal.
fn complete(evidence: &EntityEvidence, omitted: usize) -> bool {
    evidence.read_status == ReadStatus::Complete
        && !evidence.provider_pending
        && evidence.coverage.failed == 0
        && evidence.coverage.skipped_non_utf8_paths == 0
        && omitted == 0
        && evidence.provider.as_ref().is_none_or(|provider| {
            provider.error.is_none()
                && provider.coverage.dropped_unknown_files == 0
                && provider
                    .coverage
                    .skipped
                    .values()
                    .chain(provider.coverage.omitted.values())
                    .all(|&total| total == 0)
        })
}

pub(super) fn entity_change(base: &EntityResult, target: &EntityResult) -> EntityChange {
    use EntityResult as E;
    match (base, target) {
        (E::NotProvided, _) => EntityChange::NotProvided,
        (E::Pending, _) | (_, E::Pending) => EntityChange::Pending,
        // Without evidence, `missing` is Git's own answer: no such committed file. A missing
        // definition counts only when both commits were read completely on the same basis.
        (
            E::Resolved {
                evidence: base_evidence,
                ..
            },
            E::Missing { reason, evidence },
        ) => match evidence {
            Some(target_evidence)
                if basis(base_evidence) != basis(target_evidence)
                    || !complete(base_evidence, 0)
                    || !complete(target_evidence, 0) =>
            {
                insufficient("the comparison commit was read with different or incomplete coverage")
            }
            _ => EntityChange::Removed {
                reason: reason.clone(),
            },
        },
        (
            E::Resolved {
                location: base_at,
                evidence: base_evidence,
                related: base_rows,
                omitted: base_omitted,
            },
            E::Resolved {
                location: target_at,
                evidence: target_evidence,
                related: target_rows,
                omitted: target_omitted,
            },
        ) => {
            if basis(base_evidence) != basis(target_evidence) {
                return insufficient("the two commits were read with different fact providers");
            }
            if !complete(base_evidence, *base_omitted)
                || !complete(target_evidence, *target_omitted)
            {
                return insufficient(
                    "coverage is incomplete at one commit, so a missing reference would not show a change",
                );
            }
            let location = (base_at != target_at).then(|| LocationChange {
                base: base_at.clone(),
                target: target_at.clone(),
            });
            let added: Vec<_> = target_rows
                .iter()
                .filter(|row| !base_rows.contains(row))
                .cloned()
                .collect();
            let removed: Vec<_> = base_rows
                .iter()
                .filter(|row| !target_rows.contains(row))
                .cloned()
                .collect();
            if location.is_none() && added.is_empty() && removed.is_empty() {
                EntityChange::Unchanged
            } else {
                EntityChange::Changed {
                    location,
                    added,
                    removed,
                }
            }
        }
        (E::Resolved { .. }, E::Unavailable { reason, .. } | E::Ambiguous { reason, .. }) => {
            insufficient(format!("at the comparison commit: {reason}"))
        }
        (
            E::Missing { reason, .. } | E::Unavailable { reason, .. } | E::Ambiguous { reason, .. },
            _,
        ) => insufficient(format!("at the pinned commit: {reason}")),
        (E::Resolved { .. }, E::NotProvided) => insufficient("no lookup at the comparison commit"),
    }
}
