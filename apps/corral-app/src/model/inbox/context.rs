//! Lazy question context reads share the plan graph and canonical artifact cache.
use corral_protocol::detail::{ArtifactRef, DetailGet, DetailRef, EvidenceBody, QuestionDetail};

use super::{State, question_detail};
use crate::model::daemon::Feed;

pub(super) fn plan_id(detail: &QuestionDetail) -> Option<&str> {
    detail
        .place
        .position
        .as_ref()
        .map(|position| position.plan.as_str())
        .or(detail.plan.as_deref())
}

/// Resolve only canonical catalog references; URLs and arbitrary paths never become reads.
pub(super) fn image_ref(artifact: &ArtifactRef) -> Option<DetailRef> {
    let key = artifact.source_ref.as_str();
    let owner = if let Some(path) = key.strip_prefix("artifacts/") {
        let mut parts = path.split('/');
        match parts.next()? {
            "assistant" => parts.next()?,
            task => task,
        }
    } else {
        key.split_once(':')?.0
    };
    if !(owner.starts_with("T-") || owner.starts_with("A-")) {
        return None;
    }
    Some(DetailRef::Artifact {
        task: owner.into(),
        key: key.into(),
    })
}

/// Context currently visible on a question, kept pinned while its view is mounted.
pub fn question_context_refs(state: &State, feed: &Feed, key: &str) -> Vec<DetailRef> {
    let Some(detail) = question_detail(feed, key) else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    if let Some(id) = plan_id(detail) {
        refs.push(DetailRef::Plan { id: id.into() });
    }
    let index = state
        .desk
        .draft(key)
        .map_or(0, |draft| draft.evidence)
        .min(detail.evidence.len().saturating_sub(1));
    if let Some(block) = detail.evidence.get(index)
        && let EvidenceBody::Image { artifact } = &block.body
        && let Some(target) = image_ref(artifact)
    {
        refs.push(target);
    }
    refs
}

/// Missing context and continuation pages only. Failures remain visible, not retry loops.
pub fn question_dependencies(state: &State, feed: &Feed, key: &str) -> Vec<DetailGet> {
    question_context_refs(state, feed, key)
        .into_iter()
        .filter_map(|target| {
            let cache = feed.detail(&target);
            if cache.is_some_and(|cache| cache.loading || cache.error.is_some()) {
                return None;
            }
            let (etag, page) = match cache {
                None => (None, None),
                Some(cache) if cache.stale => {
                    (cache.pages.first().map(|page| page.etag.clone()), None)
                }
                Some(cache) => (None, Some(cache.pages.last()?.next.clone()?)),
            };
            Some(DetailGet { target, etag, page })
        })
        .collect()
}
