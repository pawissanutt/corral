//! A detail read a page at a time (the artifact finder's list, a job's output), and how a
//! detail's pages are merged into the one body the app reads.

use super::*;

/// Whether the detail is read a page at a time, each further page only when the owner asks for
/// it: the artifact finder's list and a job's output (newest first, older on request).
pub(super) fn walked(target: &DetailRef) -> bool {
    matches!(
        target,
        DetailRef::Artifacts { .. }
            | DetailRef::SessionJobOutput { .. }
            | DetailRef::ProjectBranches { .. }
    )
}

impl AppState {
    /// A page of the walked list. A walk to a further page adds it after those walked; a
    /// revalidation keeps each saved page corrald finds unchanged (asking for the next with its
    /// own etag while the chain of tokens is the saved one) and takes each changed one, until
    /// as many pages as were walked are current.
    pub(super) fn walked_reply(
        &mut self,
        feed: Signal<Feed>,
        id: String,
        key: String,
        request: DetailGet,
        page: DetailReply,
    ) -> Vec<Outgoing> {
        let saved = feed.with_untracked(|feed| {
            feed.details
                .get(&key)
                .map(|cache| cache.pages.clone())
                .unwrap_or_default()
        });
        let chain = self.detail_pages.remove(&key).unwrap_or_default();
        // A branch refresh starts a new window, not a replay of the old walked depth.
        if matches!(request.target, DetailRef::ProjectBranches { .. }) && request.page.is_none() {
            let current = if page.body.is_some() {
                page
            } else if let Some(kept) = saved.first().filter(|kept| kept.etag == page.etag) {
                DetailReply {
                    as_of: page.as_of,
                    seq: page.seq,
                    ..kept.clone()
                }
            } else {
                self.detail_failed(
                    feed,
                    &key,
                    &id,
                    ClientError {
                        code: "invalid_reply".into(),
                        message: "Uncached branch page marked unchanged.".into(),
                    },
                );
                return vec![];
            };
            if self.over_budget(feed, &key, &id, &current, 0) {
                return vec![];
            }
            self.publish(feed, &key, &id, &request.target, vec![current]);
            return vec![];
        }
        if request.page.is_some() && chain.is_empty() && request.etag.is_none() {
            let mut pages = saved;
            // Branch inventories can be arbitrarily large. Keep a small moving window rather
            // than stranding the picker at the general detail cache's 256-page bound.
            if matches!(request.target, DetailRef::ProjectBranches { .. }) {
                trim_branch_window(&mut pages);
            }
            if self.over_budget(feed, &key, &id, &page, pages.len()) {
                return vec![];
            }
            pages.push(page);
            self.publish(feed, &key, &id, &request.target, pages);
            return vec![];
        }
        let at = chain.len();
        let current = match page.body {
            Some(_) => page,
            None => match saved.get(at).filter(|kept| kept.etag == page.etag) {
                Some(kept) => DetailReply {
                    as_of: page.as_of,
                    seq: page.seq,
                    ..kept.clone()
                },
                None => {
                    let error = ClientError {
                        code: "invalid_reply".into(),
                        message: "corrald said an artifact page is unchanged that this device does not hold.".into(),
                    };
                    self.detail_failed(feed, &key, &id, error);
                    return vec![];
                }
            },
        };
        if self.over_budget(feed, &key, &id, &current, chain.len()) {
            return vec![];
        }
        let next = current.next.clone();
        let mut chain = chain;
        chain.push(current);
        let following = chain.len();
        match next.filter(|_| following < saved.len()) {
            Some(token) => {
                // The saved page is still the one this token names only if the chain led to it.
                let etag = (saved[following - 1].next.as_deref() == Some(token.as_str()))
                    .then(|| saved[following].etag.clone());
                self.detail_pages.insert(key, chain);
                vec![out(
                    DETAIL_GET,
                    value(DetailGet {
                        target: request.target,
                        etag,
                        page: Some(token),
                    }),
                    Some(id),
                )]
            }
            None => {
                self.publish(feed, &key, &id, &request.target, chain);
                vec![]
            }
        }
    }
}

fn trim_branch_window(pages: &mut Vec<DetailReply>) {
    const WINDOW: usize = 4;
    if pages.len() >= WINDOW {
        pages.drain(..pages.len() + 1 - WINDOW);
    }
}

pub(super) fn merge_pages(pages: &[DetailReply]) -> Result<Option<DetailBody>, String> {
    if let Some(DetailBody::ProjectBranches(first)) =
        pages.first().and_then(|page| page.body.as_ref())
    {
        let mut merged = first.clone();
        for page in &pages[1..] {
            let Some(DetailBody::ProjectBranches(part)) = &page.body else {
                return Err("Branch page kind changed.".into());
            };
            merged.branches.extend(part.branches.iter().cloned());
        }
        return Ok(Some(DetailBody::ProjectBranches(merged)));
    }
    if let Some(DetailBody::Artifact(first)) = pages.first().and_then(|page| page.body.as_ref()) {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut artifact = first.clone();
        // A text artifact's page that is not UTF-8 arrives as bytes: then all of it is bytes.
        let mut bytes = vec![];
        for page in pages {
            let Some(DetailBody::Artifact(part)) = &page.body else {
                return Err("Artifact page kind changed.".into());
            };
            if part.task != artifact.task
                || part.key != artifact.key
                || part.media_type != artifact.media_type
            {
                return Err("Artifact page identity changed.".into());
            }
            match part.encoding {
                ArtifactEncoding::Text => bytes.extend_from_slice(part.content.as_bytes()),
                ArtifactEncoding::Base64 => {
                    artifact.encoding = ArtifactEncoding::Base64;
                    bytes.extend(
                        STANDARD
                            .decode(&part.content)
                            .map_err(|error| format!("Invalid artifact base64: {error}"))?,
                    )
                }
                ArtifactEncoding::Other => return Ok(Some(DetailBody::Artifact(first.clone()))),
            }
            artifact.truncated = part.truncated;
            if bytes.len() > MAX_DETAIL_BYTES {
                return Err("Artifact exceeds this device's bounded byte cache.".into());
            }
        }
        artifact.content = match artifact.encoding {
            ArtifactEncoding::Text => String::from_utf8(bytes)
                .map_err(|_| "Artifact text pages are not UTF-8.".to_owned())?,
            _ => STANDARD.encode(bytes),
        };
        return Ok(Some(DetailBody::Artifact(artifact)));
    }
    // A job's output is read newest page first, each further page older: the pages' words join
    // oldest first, and the output starts where the oldest page read starts.
    if let Some(DetailBody::SessionJobOutput(newest)) =
        pages.first().and_then(|page| page.body.as_ref())
    {
        let mut output = newest.clone();
        output.text.clear();
        for page in pages.iter().rev() {
            let Some(DetailBody::SessionJobOutput(part)) = &page.body else {
                return Err("Job output page kind changed.".into());
            };
            output.text.push_str(&part.text);
            if output.text.len() > MAX_DETAIL_BYTES {
                return Err("Job output exceeds this device's bounded byte cache.".into());
            }
        }
        if let Some(DetailBody::SessionJobOutput(oldest)) =
            pages.last().and_then(|page| page.body.as_ref())
        {
            output.offset = oldest.offset;
        }
        return Ok(Some(DetailBody::SessionJobOutput(output)));
    }
    // A proposed task's brief arrives in byte ranges: the pages' words join in offset order.
    if let Some(DetailBody::BuildPlanTask(first)) =
        pages.first().and_then(|page| page.body.as_ref())
    {
        let mut parts = Vec::with_capacity(pages.len());
        for page in pages {
            let Some(DetailBody::BuildPlanTask(part)) = &page.body else {
                return Err("Brief page kind changed.".into());
            };
            if part.key != first.key || part.bytes != first.bytes {
                return Err("Brief page identity changed.".into());
            }
            parts.push(part);
        }
        parts.sort_by_key(|part| part.offset);
        let mut brief = first.clone();
        brief.body.clear();
        brief.offset = parts.first().map_or(0, |part| part.offset);
        for part in parts {
            brief.body.push_str(&part.body);
            if brief.body.len() > MAX_DETAIL_BYTES {
                return Err("Brief exceeds this device's bounded byte cache.".into());
            }
        }
        return Ok(Some(DetailBody::BuildPlanTask(brief)));
    }
    // The plan document arrives the same way.
    if let Some(DetailBody::SessionPlanModeDoc(first)) =
        pages.first().and_then(|page| page.body.as_ref())
    {
        let mut parts = Vec::with_capacity(pages.len());
        for page in pages {
            let Some(DetailBody::SessionPlanModeDoc(part)) = &page.body else {
                return Err("Plan page kind changed.".into());
            };
            if part.revision != first.revision || part.bytes != first.bytes {
                return Err("Plan page identity changed.".into());
            }
            parts.push(part);
        }
        parts.sort_by_key(|part| part.offset);
        let mut plan = first.clone();
        plan.body.clear();
        plan.offset = parts.first().map_or(0, |part| part.offset);
        for part in parts {
            plan.body.push_str(&part.body);
            if plan.body.len() > MAX_DETAIL_BYTES {
                return Err("Plan exceeds this device's bounded byte cache.".into());
            }
        }
        return Ok(Some(DetailBody::SessionPlanModeDoc(plan)));
    }
    // Only a map's or impact's rows are paged; every page repeats the rest whole, its provider's
    // category lists and the impact's category summaries included, so the first page holds them.
    if let Some(DetailBody::SystemMap(first)) = pages.first().and_then(|page| page.body.as_ref()) {
        let mut map = first.clone();
        for page in &pages[1..] {
            let Some(DetailBody::SystemMap(part)) = &page.body else {
                return Err("System map page kind changed.".into());
            };
            map.rows.extend(part.rows.iter().cloned());
        }
        return Ok(Some(DetailBody::SystemMap(map)));
    }
    if let Some(DetailBody::SystemReview(first)) = pages.first().and_then(|page| page.body.as_ref())
    {
        let mut review = first.clone();
        for page in &pages[1..] {
            let Some(DetailBody::SystemReview(part)) = &page.body else {
                return Err("Impact page kind changed.".into());
            };
            review.rows.extend(part.rows.iter().cloned());
        }
        return Ok(Some(DetailBody::SystemReview(review)));
    }
    let mut merged: Option<Value> = None;
    for page in pages {
        if let Some(body) = &page.body {
            let body = serde_json::to_value(body).map_err(|error| error.to_string())?;
            match &mut merged {
                Some(merged) => append(merged, body, ""),
                None => merged = Some(body),
            }
        }
    }
    merged
        .map(|body| {
            serde_json::from_value(body)
                .map_err(|error| format!("invalid assembled detail: {error}"))
        })
        .transpose()
}

fn append(current: &mut Value, next: Value, field: &str) {
    match (current, next) {
        (Value::Array(current), Value::Array(next)) => {
            if field == "stages" {
                return;
            }
            // Oversized id-bearing objects recur across pages with nested arrays split.
            let keyed = current
                .iter()
                .chain(next.iter())
                .all(|item| item_key(item).is_some());
            if keyed {
                let mut positions: BTreeMap<String, usize> = current
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| item_key(item).map(|id| (id.into(), index)))
                    .collect();
                for item in next {
                    let id = item_key(&item).unwrap_or_default().to_owned();
                    if let Some(index) = positions.get(&id) {
                        append(&mut current[*index], item, "");
                    } else {
                        positions.insert(id, current.len());
                        current.push(item);
                    }
                }
            } else {
                current.extend(next);
            }
        }
        (Value::Object(current), Value::Object(next)) => {
            for (key, value) in next {
                if let Some(current) = current.get_mut(&key) {
                    append(current, value, &key);
                } else {
                    current.insert(key, value);
                }
            }
        }
        (Value::String(current), Value::String(next)) if field == "patch" => {
            current.push_str(&next)
        }
        (Value::Bool(current), Value::Bool(next)) if field == "truncated" => *current = next,
        // Each continuation repeats scalar metadata; it is not another result.
        _ => {}
    }
}

fn item_key(item: &Value) -> Option<&str> {
    item.get("id").or_else(|| item.get("day"))?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_window_advances_without_growing_and_keeps_latest_cursor() {
        let mut pages = Vec::new();
        for index in 0..6 {
            trim_branch_window(&mut pages);
            pages.push(DetailReply {
                etag: index.to_string(),
                as_of: "2026-11-14T00:00:00Z".into(),
                seq: 1,
                next: Some((index + 1).to_string()),
                body: None,
            });
        }
        assert_eq!(pages.len(), 4);
        assert_eq!(pages[0].etag, "2");
        assert_eq!(
            pages.last().and_then(|page| page.next.as_deref()),
            Some("6")
        );
    }

    #[test]
    fn paged_nested_rows_merge_by_id_and_fixed_stages_do_not_duplicate() {
        let mut first = json!({"stages":[1,2,3,4],"annotations":[
            {"id":"note-1","messages":["one"]}],"patch":"first\n"});
        append(
            &mut first,
            json!({"stages":[1,2,3,4],"annotations":[
            {"id":"note-1","messages":["two"]},{"id":"note-2","messages":["three"]}],"patch":"second\n"}),
            "",
        );
        assert_eq!(first["stages"], json!([1, 2, 3, 4]));
        assert_eq!(first["annotations"][0]["messages"], json!(["one", "two"]));
        assert_eq!(first["annotations"][1]["id"], "note-2");
        assert_eq!(first["patch"], "first\nsecond\n");
    }

    #[test]
    fn binary_artifacts_decode_each_padded_page_before_assembly() {
        let pages = ["AAE=", "AgM="]
            .into_iter()
            .map(|content| DetailReply {
                etag: "content-hash".into(),
                as_of: "2026-11-14T00:00:00Z".into(),
                seq: 1,
                next: None,
                body: Some(DetailBody::Artifact(ArtifactPage {
                    task: "T-1".into(),
                    key: "report".into(),
                    media_type: None,
                    source_format: None,
                    encoding: ArtifactEncoding::Base64,
                    content: content.into(),
                    truncated: false,
                })),
            })
            .collect::<Vec<_>>();
        let Some(DetailBody::Artifact(artifact)) = merge_pages(&pages).unwrap() else {
            panic!("artifact body was not assembled");
        };
        assert_eq!(artifact.content, "AAECAw==");
    }

    #[test]
    fn system_map_pages_keep_one_provider_category_list() {
        let provided = vec!["files".to_owned(), "definitions".to_owned()];
        let pages = (0..3)
            .map(|_| DetailReply {
                etag: "map".into(),
                as_of: "2026-11-22T00:00:00Z".into(),
                seq: 1,
                next: None,
                body: Some(DetailBody::SystemMap(corral_types::SystemMap {
                    project: "corral".into(),
                    head: Some("abc".into()),
                    status: corral_types::SystemStatus::Ready,
                    error: None,
                    provider: Some(corral_types::SystemProvider {
                        extension: "codeindex".into(),
                        coverage: corral_types::ProviderCoverage {
                            provided: provided.clone(),
                            not_provided: vec!["references".into()],
                            skipped: BTreeMap::new(),
                            omitted: BTreeMap::new(),
                            dropped_unknown_files: 0,
                        },
                        error: None,
                    }),
                    coverage: None,
                    omitted_rows: 0,
                    rows: Vec::new(),
                })),
            })
            .collect::<Vec<_>>();
        let Some(DetailBody::SystemMap(map)) = merge_pages(&pages).unwrap() else {
            panic!("map body was not assembled");
        };
        let coverage = map.provider.unwrap().coverage;
        assert_eq!(coverage.provided, provided);
        assert_eq!(coverage.not_provided, ["references"]);
    }
}
