//! The foot of Impact's map (docs/plans/system-map.md §3.3): a legend with a sample of each
//! mark the map draws at this level, and one provenance line naming the commit the map shows,
//! the commits the change is measured between, what produced the facts, and that no agent ran.

use corral_types::SystemAt;

use super::{request, review_of, short};
use crate::model::daemon::Feed;
use crate::model::system::words::Swatch;

/// The legend of Impact's map at `at` (the project level when none): the marks, then the
/// changed-edge line where the level draws references between packages or modules.
pub fn legend(at: Option<&SystemAt>) -> Vec<(Swatch, String)> {
    let mut out = vec![
        (Swatch::Touched, "changed in this task".to_owned()),
        (
            Swatch::Dependents,
            "holds dependents of a changed definition".to_owned(),
        ),
        (Swatch::Untouched, "untouched".to_owned()),
    ];
    if matches!(at, None | Some(SystemAt::Container { .. })) {
        out.push((
            Swatch::ChangedEdge,
            "reference count changed, before → after".to_owned(),
        ));
    }
    out
}

/// The provenance line of task `task`'s impact; none until its review is read.
pub fn provenance(feed: &Feed, task: &str) -> Option<String> {
    let (review, stale) = review_of(feed, &request(task, None))?;
    let facts = match &review.provider {
        Some(p) if p.error.is_some() => {
            format!(
                "git, manifests and the import scan ({} failed)",
                p.extension
            )
        }
        Some(p) if !p.coverage.provided.is_empty() => format!(
            "git, manifests, the import scan and {} ({})",
            p.extension,
            p.coverage.provided.join(", ")
        ),
        _ => "git, manifests and the import scan".to_owned(),
    };
    let base = if review.base_estimated {
        format!("{} (inferred)", short(&review.base))
    } else {
        short(&review.base).to_owned()
    };
    Some(format!(
        "Map at {head} · change measured from {base} to {head} · facts from {facts} · no agent ran{}",
        if stale { " · as last read" } else { "" },
        head = short(&review.head),
    ))
}

/// The kind a breadcrumb names after a level: the project, then each level drilled into.
pub fn crumb_kind(at: Option<&SystemAt>) -> &'static str {
    match at {
        None => "project",
        Some(SystemAt::Container { .. }) => "package",
        Some(SystemAt::Module { .. }) => "module",
        Some(SystemAt::File { .. }) => "file",
    }
}
