use super::*;

const MAX_PAGES: usize = 256;
const MAX_DETAIL_BYTES: usize = 8 * 1024 * 1024;

impl AppState {
    pub(super) fn detail_request(
        &mut self,
        feed: Signal<Feed>,
        id: String,
        request: DetailGet,
    ) -> Vec<Outgoing> {
        let mut request = match request {
            DetailGet {
                target: DetailRef::SessionItem { id, token },
                ..
            } => {
                return self.session_item(feed, id, token);
            }
            request => request,
        };
        let key = detail_key(&request.target);
        // The finder's list is walked: each page is asked for as the owner reaches it and adds
        // to those before, rather than all of them being fetched and merged up front.
        let walked = walked(&request.target);
        if walked && request.page.is_none() && request.etag.is_none() {
            // Revalidated page by page from its first: each unchanged page walked stays.
            request.etag = feed.with_untracked(|feed| {
                feed.details
                    .get(&key)
                    .filter(|cache| cache.merged.is_some() && cache.error.is_none())
                    .and_then(|cache| cache.pages.first())
                    .map(|page| page.etag.clone())
            });
        }
        // One page of a paged list is shown at a time, so a new page retires its siblings.
        let paged = walked || matches!(&request.target, DetailRef::Closed { .. });
        let mut retired = if paged {
            let list = std::mem::discriminant(&request.target);
            feed.with_untracked(|feed| {
                feed.details
                    .iter()
                    .filter(|(cached_key, cache)| {
                        cached_key.as_str() != key.as_str()
                            && std::mem::discriminant(&cache.target) == list
                    })
                    .map(|(cached_key, _)| (cached_key.clone(), None))
                    .collect::<Vec<_>>()
            })
        } else {
            Vec::new()
        };
        for (retired_key, retired_id) in &mut retired {
            *retired_id = self.detail_inflight.remove(retired_key);
            self.evict_detail(feed, retired_key);
        }
        feed.update(|feed| {
            for (_, retired_id) in &retired {
                if let Some(operation) = retired_id
                    .as_ref()
                    .and_then(|id| feed.operations.get_mut(id.as_str()))
                {
                    operation.loading = false;
                    operation.value = None;
                    operation.error = Some(ClientError {
                        code: "superseded".into(),
                        message: "A different page of this list replaced this detail request."
                            .into(),
                    });
                }
            }
            let cache = feed
                .details
                .entry(key.clone())
                .or_insert_with(|| DetailCache {
                    target: request.target.clone(),
                    pages: vec![],
                    loading: false,
                    stale: true,
                    error: None,
                    merged: None,
                });
            cache.loading = self.ready;
            cache.stale = true;
            cache.error = (!self.ready).then(offline);
        });
        // A walked list is revalidated from its first page, never from a page walked to.
        let remembered = DetailGet {
            page: request.page.clone().filter(|_| !walked),
            ..request.clone()
        };
        self.retained.select(&remembered, &id);
        if !self.ready {
            self.touch_detail(feed, &request.target);
            self.trim_retained(feed);
            return vec![];
        }
        if let DetailRef::Session { id } = &request.target {
            if let Some(reread) =
                feed.with_untracked(|feed| feed.live.get(id.as_str()).map(|live| live.reread))
            {
                self.detail_reread.insert(key.clone(), reread);
            } else {
                self.detail_reread.remove(&key);
            }
        }
        // Only the latest request for a stable selector can publish its pages.
        if let Some(previous) = self.detail_inflight.insert(key.clone(), id.clone()) {
            feed.update(|feed| {
                if let Some(operation) = feed.operations.get_mut(previous.as_str()) {
                    operation.loading = false;
                    operation.error = Some(ClientError {
                        code: "superseded".into(),
                        message: "A newer fetch replaced this detail request.".into(),
                    });
                }
            });
        }
        self.detail_pages.insert(key.clone(), vec![]);
        self.detail_bytes.insert(key.clone(), 0);
        let target = request.target.clone();
        let outgoing = self.operation(feed, id.clone(), DETAIL_GET, value(request));
        self.touch_detail(feed, &target);
        self.trim_retained(feed);
        if self.detail_inflight.get(&key) == Some(&id) {
            outgoing
        } else {
            vec![]
        }
    }
    pub(super) fn detail_reply(
        &mut self,
        feed: Signal<Feed>,
        tag: Option<String>,
        params: Value,
        outcome: Result<Value, (corral_protocol::rpc::ErrorCode, String)>,
    ) -> Result<Vec<Outgoing>, String> {
        let request: DetailGet = decode(params)?;
        if let DetailRef::SessionItem { id, token } = &request.target {
            return self.session_item_reply(feed, tag, id, token, outcome);
        }
        let key = detail_key(&request.target);
        let Some(id) = tag else {
            return Err("detail reply has no correlation identity".into());
        };
        if self.detail_inflight.get(&key) != Some(&id) {
            return Ok(vec![]);
        }
        let page: DetailReply = match outcome {
            Ok(value) => decode(value)?,
            Err((code, message)) => {
                let error = ClientError {
                    code: value(code).as_str().unwrap_or("unknown").into(),
                    message,
                };
                self.detail_failed(feed, &key, &id, error);
                return Ok(vec![]);
            }
        };
        if walked(&request.target) {
            return Ok(self.walked_reply(feed, id, key, request, page));
        }
        let pages = self.detail_pages.entry(key.clone()).or_default();
        if let Some(first) = pages.first() {
            if first.etag != page.etag {
                self.detail_failed(feed, &key, &id, ClientError { code: "conflict".into(),
                    message: "Detail changed while its pages were loading; refresh to fetch a current snapshot.".into() });
                return Ok(vec![]);
            }
        }
        if page.body.is_none() {
            if !pages.is_empty() || request.page.is_some() {
                return Err("daemon returned not-modified inside a detail continuation".into());
            }
            let valid = feed.with_untracked(|feed| {
                feed.details.get(&key).is_some_and(|cache| {
                    cache.merged.is_some()
                        && cache.pages.first().is_some_and(|old| old.etag == page.etag)
                })
            });
            if !valid {
                return Err("daemon returned not-modified without a matching cached detail".into());
            }
            let reread = self.detail_reread.remove(&key);
            feed.update(|feed| {
                feed.settle_session(&request.target, reread);
                if let Some(cache) = feed.details.get_mut(&key) {
                    cache.loading = false;
                    cache.stale = false;
                    cache.error = None;
                    if let Some(first) = cache.pages.first_mut() {
                        first.as_of = page.as_of.clone();
                        first.seq = page.seq;
                    }
                }
                if let Some(operation) = feed.operations.get_mut(id.as_str()) {
                    operation.loading = false;
                    operation.value = Some(value(page));
                    operation.error = None;
                }
            });
            self.detail_pages.remove(&key);
            self.detail_inflight.remove(&key);
            self.detail_bytes.remove(&key);
            self.touch_detail(feed, &request.target);
            self.trim_retained(feed);
            return Ok(vec![]);
        }
        let next = page.next.clone();
        let held = self.detail_pages.get(&key).map_or(0, Vec::len);
        if self.over_budget(feed, &key, &id, &page, held) {
            return Ok(vec![]);
        }
        let pages = self.detail_pages.entry(key.clone()).or_default();
        pages.push(page);
        if let Some(next) = next {
            self.touch_detail(feed, &request.target);
            self.trim_retained(feed);
            if self.detail_inflight.get(&key) != Some(&id) {
                return Ok(vec![]);
            }
            return Ok(vec![out(
                DETAIL_GET,
                value(DetailGet {
                    target: request.target,
                    etag: None,
                    page: Some(next),
                }),
                Some(id),
            )]);
        }
        let pages = self.detail_pages.remove(&key).unwrap_or_default();
        self.publish(feed, &key, &id, &request.target, pages);
        Ok(vec![])
    }

    /// A page of the walked list. A walk to a further page adds it after those walked; a
    /// revalidation keeps each saved page corrald finds unchanged (asking for the next with its
    /// own etag while the chain of tokens is the saved one) and takes each changed one, until
    /// as many pages as were walked are current.
    fn walked_reply(
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
        if request.page.is_some() && chain.is_empty() && request.etag.is_none() {
            if self.over_budget(feed, &key, &id, &page, saved.len()) {
                return vec![];
            }
            let mut pages = saved;
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

    /// Adds `page` to the bytes the request has fetched, after the `held` pages before it; once
    /// over this device's bound the request fails instead.
    fn over_budget(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
        page: &DetailReply,
        held: usize,
    ) -> bool {
        let bytes = self.detail_bytes.entry(key.to_owned()).or_default();
        *bytes = bytes.saturating_add(cache::bytes(page));
        if held < MAX_PAGES && *bytes <= MAX_DETAIL_BYTES {
            return false;
        }
        self.detail_failed(
            feed,
            key,
            id,
            ClientError {
                code: "too_large".into(),
                message: "Detail exceeds this device's bounded cache (8 MiB).".into(),
            },
        );
        true
    }

    /// Publishes the request's pages, merged, as the detail's current snapshot.
    fn publish(
        &mut self,
        feed: Signal<Feed>,
        key: &str,
        id: &str,
        target: &DetailRef,
        mut pages: Vec<DetailReply>,
    ) {
        let seq = pages.iter().map(|page| page.seq).max().unwrap_or(0);
        let as_of = pages.last().map(|page| page.as_of.clone());
        if let Some(first) = pages.first_mut() {
            first.seq = seq;
            if let Some(as_of) = as_of {
                first.as_of = as_of;
            }
        }
        self.detail_bytes.remove(key);
        let merged = match merge_pages(&pages) {
            Ok(merged) => merged,
            Err(message) => {
                self.detail_failed(
                    feed,
                    key,
                    id,
                    ClientError {
                        code: "invalid_reply".into(),
                        message,
                    },
                );
                return;
            }
        };
        self.detail_inflight.remove(key);
        let reread = self.detail_reread.remove(key);
        feed.update(|feed| {
            if matches!(&merged, Some(DetailBody::Session(_))) {
                feed.settle_session(target, reread);
            }
            if let Some(cache) = feed.details.get_mut(key) {
                cache.pages = pages;
                cache.merged = merged;
                cache.loading = false;
                cache.stale = false;
                cache.error = None;
            }
            let response = feed
                .details
                .get(key)
                .and_then(|cache| cache.pages.first())
                .map(value);
            if let Some(operation) = feed.operations.get_mut(id) {
                operation.loading = false;
                operation.value = response;
                operation.error = None;
            }
        });
        self.touch_detail(feed, target);
        self.trim_retained(feed);
    }
    fn detail_failed(&mut self, feed: Signal<Feed>, key: &str, id: &str, error: ClientError) {
        self.detail_pages.remove(key);
        self.detail_inflight.remove(key);
        self.detail_bytes.remove(key);
        self.detail_reread.remove(key);
        feed.update(|feed| {
            if let Some(cache) = feed.details.get_mut(key) {
                cache.loading = false;
                cache.stale = true;
                cache.error = Some(error.clone());
            }
            if let Some(operation) = feed.operations.get_mut(id) {
                operation.loading = false;
                operation.error = Some(error);
            }
        });
        let target = self.retained_target(feed, key);
        if let Some(target) = target {
            self.touch_detail(feed, &target);
        }
        self.trim_retained(feed);
    }
    pub(super) fn refresh_details(&mut self, feed: Signal<Feed>) -> Vec<Outgoing> {
        let targets = feed.with_untracked(|feed| {
            feed.details
                .values()
                .map(|cache| cache.target.clone())
                .collect::<Vec<_>>()
        });
        let mut outgoing = vec![];
        for target in targets {
            outgoing.extend(self.detail_request(
                feed,
                requests::new_id(),
                self.retained.selected(target),
            ));
        }
        outgoing
    }
    fn retained_target(&self, feed: Signal<Feed>, key: &str) -> Option<DetailRef> {
        feed.with_untracked(|feed| feed.details.get(key).map(|cache| cache.target.clone()))
    }
}

fn merge_pages(pages: &[DetailReply]) -> Result<Option<DetailBody>, String> {
    if let Some(DetailBody::Artifact(first)) = pages.first().and_then(|page| page.body.as_ref()) {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut artifact = first.clone();
        let mut bytes = vec![];
        artifact.content.clear();
        for page in pages {
            let Some(DetailBody::Artifact(part)) = &page.body else {
                return Err("Artifact page kind changed.".into());
            };
            if part.task != artifact.task
                || part.key != artifact.key
                || part.encoding != artifact.encoding
                || part.media_type != artifact.media_type
            {
                return Err("Artifact page identity or encoding changed.".into());
            }
            match part.encoding {
                ArtifactEncoding::Text => artifact.content.push_str(&part.content),
                ArtifactEncoding::Base64 => bytes.extend(
                    STANDARD
                        .decode(&part.content)
                        .map_err(|error| format!("Invalid artifact base64: {error}"))?,
                ),
                ArtifactEncoding::Other => return Ok(Some(DetailBody::Artifact(first.clone()))),
            }
            artifact.truncated = part.truncated;
            if bytes.len() > MAX_DETAIL_BYTES || artifact.content.len() > MAX_DETAIL_BYTES {
                return Err("Artifact exceeds this device's bounded byte cache.".into());
            }
        }
        if artifact.encoding == ArtifactEncoding::Base64 {
            artifact.content = STANDARD.encode(bytes);
        }
        return Ok(Some(DetailBody::Artifact(artifact)));
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
            let mut body: DetailBody = serde_json::from_value(body)
                .map_err(|error| format!("invalid assembled detail: {error}"))?;
            if let DetailBody::Session(session) = &mut body {
                session
                    .messages
                    .sort_by(|left, right| left.at.cmp(&right.at));
            }
            Ok(body)
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

/// Whether the detail is a list the owner walks a page at a time: the artifact finder's.
fn walked(target: &DetailRef) -> bool {
    matches!(target, DetailRef::Artifacts { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
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
}

#[cfg(test)]
mod artifact_tests {
    use super::*;
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
}

#[cfg(test)]
#[path = "closed_tests.rs"]
mod closed_tests;
