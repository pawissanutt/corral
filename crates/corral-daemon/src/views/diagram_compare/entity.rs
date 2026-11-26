//! Whether Corral's derived facts about a bound entity changed between the two commits, and
//! whether the files those facts come from were read comparably at both.
use corral_protocol::detail::*;
use corral_types::ReadStatus;

pub(super) fn insufficient(reason: impl Into<String>) -> EntityChange {
    EntityChange::Insufficient {
        reason: reason.into(),
    }
}

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

/// A relationship delta is claimed only when both sides resolved, read the file completely with
/// the same basis, and listed every row they know.
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

/// How one commit holds a referring file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Referrer {
    /// In the tree and read completely by the facts that found its references.
    Read,
    /// In the tree, but unsupported, failed or not covered: its references are not known.
    Unread,
    /// Not in that commit's tree.
    Absent,
}

/// Whether each reference that appears or disappears comes from a file read at both commits.
/// A reference is only gone when its file was read completely at the comparison commit, or was
/// deleted outright; a file Git finds renamed to one Corral does not read is a coverage loss, not
/// a removed relationship. `added` rows are checked the same way at the pinned commit.
/// `status(target, path)` answers for one side (`None` when it cannot be read); `moved(target,
/// path)` is Git's rename of `path` into that side (`Err` when rename detection failed).
pub(super) fn referrers_comparable(
    removed: &[DiagramRelated],
    added: &[DiagramRelated],
    mut status: impl FnMut(bool, &str) -> Option<Referrer>,
    mut moved: impl FnMut(bool, &str) -> Result<Option<String>, ()>,
) -> Result<(), String> {
    let side = |target: bool| {
        if target {
            "the comparison commit"
        } else {
            "the pinned commit"
        }
    };
    let rows = removed
        .iter()
        .map(|row| (true, row))
        .chain(added.iter().map(|row| (false, row)));
    for (target, row) in rows {
        let DiagramRelated::FileReference { path, .. } = row;
        match status(target, path) {
            Some(Referrer::Read) => {}
            Some(Referrer::Unread) | None => {
                return Err(format!(
                    "{path} is not read completely at {}, so its references there are unknown",
                    side(target)
                ));
            }
            Some(Referrer::Absent) => match moved(target, path) {
                Ok(None) => {}
                Ok(Some(other)) if status(target, &other) == Some(Referrer::Read) => {}
                Ok(Some(other)) => {
                    return Err(format!(
                        "{path} is {other} at {}, which Corral does not read completely",
                        side(target)
                    ));
                }
                Err(()) => {
                    return Err(format!(
                        "Git could not tell whether {path} was renamed at {}",
                        side(target)
                    ));
                }
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use corral_types::{ReferenceBasis, ReferenceKind};

    use super::*;

    fn row(path: &str) -> DiagramRelated {
        DiagramRelated::FileReference {
            path: path.into(),
            basis: ReferenceBasis::Syntax,
            reference_kind: ReferenceKind::Use,
            count: 1,
        }
    }

    #[test]
    fn a_reference_is_gone_only_from_a_file_read_or_deleted_at_the_comparison_commit() {
        let check = |status: Option<Referrer>, renamed: Result<Option<&str>, ()>| {
            referrers_comparable(
                &[row("src/submit.rs")],
                &[],
                |_, path| match path {
                    "src/submit.rs" => status,
                    _ => Some(Referrer::Unread),
                },
                |_, _| renamed.map(|to| to.map(str::to_owned)),
            )
        };
        // Deleted outright, or still read and no longer referring: a real removal.
        assert_eq!(check(Some(Referrer::Absent), Ok(None)), Ok(()));
        assert_eq!(check(Some(Referrer::Read), Ok(None)), Ok(()));
        // Moved with its text to a path Corral does not read: coverage loss, not removal.
        assert!(check(Some(Referrer::Absent), Ok(Some("src/submit.txt"))).is_err());
        assert!(check(Some(Referrer::Unread), Ok(None)).is_err());
        assert!(check(None, Ok(None)).is_err());
        assert!(check(Some(Referrer::Absent), Err(())).is_err());
    }
}
