//! Durable, committed-candidate annotations. Adapters supply authenticated admission, not authors.
use anyhow::{Context, Result};
pub use corral_core::annotations::*;
use corral_core::{IdentityChange, Store, TaskHead};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[cfg(test)]
#[path = "annotations_tests.rs"]
mod tests;

/// Trusted entry-point capability. The callback must reauthenticate scope and live generation
/// on *every* invocation, including inside the committing task lease and before retry lookup.
/// Never construct this from client author/role fields. Host adapters own origin authentication.
pub struct AnnotationActor<'a> {
    admission: Box<dyn Fn(&Store, &TaskHead) -> Result<AnnotationAuthor> + 'a>,
}
impl<'a> AnnotationActor<'a> {
    pub fn owner_cli(cwd: PathBuf) -> Self {
        Self::authenticated(move |store, task| {
            store.annotation_owner_admission()?;
            let root = corral_git::repo_root(&cwd)?;
            let canonical = store
                .root()
                .canonicalize()
                .context("canonical annotation root")?;
            require(
                root == canonical
                    && !task
                        .current_run
                        .as_ref()
                        .is_some_and(|r| cwd.starts_with(&r.worktree)),
                AnnotationCode::NotOwner,
                "owner CLI cannot mutate from a run worktree",
            )?;
            Ok(AnnotationAuthor::Owner {
                version: 1,
                surface: AnnotationSurface::Cli,
            })
        })
    }
    /// Only authenticated CLI/host code may mint this capability. Errors propagate unchanged
    /// (e.g. host fencing); the service checks it again under the Store transaction's leases.
    pub fn authenticated(
        admission: impl Fn(&Store, &TaskHead) -> Result<AnnotationAuthor> + 'a,
    ) -> Self {
        Self {
            admission: Box::new(admission),
        }
    }
    fn admit(&self, store: &Store, head: &TaskHead) -> Result<AnnotationAuthor> {
        let author = (self.admission)(store, head)?;
        let AnnotationAuthor::Owner { version: v, .. } = &author;
        version(*v)?;
        Ok(author)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAnnotation {
    pub expected_run: String,
    pub base: String,
    pub head: String,
    pub path: String,
    pub side: AnnotationSide,
    pub range: Option<LineRange>,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeAnnotation {
    pub thread: String,
    pub expected_revision: u32,
    #[serde(default)]
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationAction {
    Open(OpenAnnotation),
    Reply(ChangeAnnotation),
    Resolve(ChangeAnnotation),
    Reopen(ChangeAnnotation),
}
impl AnnotationAction {
    fn kind(&self) -> AnnotationKind {
        match self {
            Self::Open(_) => AnnotationKind::Opened,
            Self::Reply(_) => AnnotationKind::Reply,
            Self::Resolve(_) => AnnotationKind::Resolved,
            Self::Reopen(_) => AnnotationKind::Reopened,
        }
    }
    fn validate(&self) -> Result<()> {
        match self {
            Self::Open(a) => require(
                !a.expected_run.is_empty()
                    && a.expected_run.len() <= 128
                    && a.base.len() <= 64
                    && a.head.len() <= 64
                    && a.path.len() <= 1024,
                AnnotationCode::InvalidPath,
                "candidate fields/path exceed input bounds",
            ),
            Self::Reply(a) | Self::Resolve(a) | Self::Reopen(a) => require(
                a.thread.len() <= 16,
                AnnotationCode::ThreadNotFound,
                "invalid thread identity",
            ),
        }
    }
    fn text(&self) -> &str {
        match self {
            Self::Open(a) => &a.text,
            Self::Reply(a) | Self::Resolve(a) | Self::Reopen(a) => &a.text,
        }
    }
}
pub fn payload_sha256(task: &str, action: &AnnotationAction) -> Result<String> {
    Ok(hash(&serde_json::to_vec(&(1, task, action))?))
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn candidate(head: &TaskHead) -> CandidateAvailability {
    let Some(run) = &head.current_run else {
        return CandidateAvailability::Unavailable {
            reason: "no_run".into(),
        };
    };
    if !run.worktree.is_dir() {
        return CandidateAvailability::Unavailable {
            reason: "worktree_missing".into(),
        };
    }
    let read = || -> Result<AnnotationCandidate> {
        // patch_base resolves HEAD itself: compare again so a moving HEAD never forms a mixed anchor.
        let commit = corral_git::rev_sha(&run.worktree, "HEAD")?;
        let base = crate::review_contracts::patch_base(run)?;
        let tree = corral_git::rev_sha(&run.worktree, &format!("{commit}^{{tree}}"))?;
        require(
            corral_git::rev_sha(&run.worktree, "HEAD")? == commit,
            AnnotationCode::CandidateUnavailable,
            "HEAD moved while reading candidate",
        )?;
        Ok(AnnotationCandidate {
            run: run.id.clone(),
            base,
            head: commit,
            tree,
        })
    };
    match read() {
        Ok(candidate) => CandidateAvailability::Available { candidate },
        Err(_) => CandidateAvailability::Unavailable {
            reason: "git_failed".into(),
        },
    }
}
fn current(head: &TaskHead, expected: &OpenAnnotation) -> Result<AnnotationCandidate> {
    match candidate(head) {
        CandidateAvailability::Available { candidate } => {
            if candidate.run != expected.expected_run
                || candidate.base != expected.base
                || candidate.head != expected.head
            {
                return Err(annotation_error(
                    AnnotationCode::StaleCandidate,
                    "candidate moved",
                    json!(candidate),
                ));
            }
            Ok(candidate)
        }
        other => Err(annotation_error(
            AnnotationCode::CandidateUnavailable,
            "candidate cannot be verified",
            json!(other),
        )),
    }
}
fn anchor(
    head: &TaskHead,
    a: &OpenAnnotation,
    c: &AnnotationCandidate,
) -> Result<AnnotationAnchor> {
    validate_path(&a.path)?;
    if let Some(range) = a.range {
        range.validate()?;
    }
    let repo = &head
        .current_run
        .as_ref()
        .context("annotation run")?
        .worktree;
    require(
        corral_git::committed_paths(repo, &c.base, &c.head)?.contains(&a.path),
        AnnotationCode::PathOutOfScope,
        "path is not changed in the committed candidate",
    )?;
    let commit = if a.side == AnnotationSide::Old {
        &c.base
    } else {
        &c.head
    };
    let blob = corral_git::committed_blob(repo, commit, &a.path)?;
    require(
        blob.is_some(),
        AnnotationCode::PathOutOfScope,
        "path has no blob on the requested side",
    )?;
    let blob = blob.context("checked annotation blob")?;
    let size = corral_git::blob_size(repo, &blob)?;
    if a.range.is_some() {
        require(
            size <= 8 * 1024 * 1024,
            AnnotationCode::FileTooLarge,
            "line anchor blob exceeds 8 MiB",
        )?;
    }
    let mut digest = Sha256::new();
    let (mut prefix, mut text, mut bytes, mut lines) = (Vec::new(), Vec::new(), 0u64, 0u32);
    corral_git::stream_blob(repo, &blob, |chunk| {
        bytes += chunk.len() as u64;
        require(
            bytes <= size,
            AnnotationCode::CandidateUnavailable,
            "blob exceeded declared size",
        )?;
        if a.range.is_some() {
            text.extend_from_slice(chunk);
        } else {
            digest.update(chunk);
            for &b in chunk.iter().take(2048) {
                if prefix.len() == 2048 || lines == 40 {
                    break;
                }
                prefix.push(b);
                lines += u32::from(b == b'\n');
            }
        }
        Ok(())
    })?;
    require(
        bytes == size,
        AnnotationCode::CandidateUnavailable,
        "incomplete blob stream",
    )?;
    let selected = if let Some(r) = a.range {
        require(
            !text.contains(&0),
            AnnotationCode::BinaryRange,
            "line anchors require text blobs",
        )?;
        let text = std::str::from_utf8(&text).map_err(|_| {
            annotation_error(
                AnnotationCode::BinaryRange,
                "line anchor is not UTF-8",
                Value::Null,
            )
        })?;
        let mut selected = 0;
        let mut count = 0;
        for (i, line) in text.split_inclusive('\n').enumerate() {
            count = i + 1;
            if count >= r.start as usize && count <= r.end as usize {
                digest.update(line.as_bytes());
                selected += line.len();
                if count - (r.start as usize) < 40 {
                    prefix.extend(line.bytes().take(2048 - prefix.len()));
                }
            }
        }
        require(
            r.end as usize <= count,
            AnnotationCode::InvalidRange,
            "range exceeds file lines",
        )?;
        while std::str::from_utf8(&prefix).is_err() {
            prefix.pop();
        }
        selected
    } else {
        usize::try_from(size).unwrap_or(usize::MAX)
    };
    // Truncate only at a UTF-8 boundary; lossy decoding is allowed for file-level binary excerpts.
    let mut excerpt = String::from_utf8_lossy(&prefix).into_owned();
    while excerpt.len() > 2048 {
        excerpt.pop();
    }
    Ok(AnnotationAnchor {
        version: 1,
        task: head.id.clone(),
        run: c.run.clone(),
        base: c.base.clone(),
        head: c.head.clone(),
        tree: c.tree.clone(),
        path: a.path.clone(),
        side: a.side,
        blob: Some(blob),
        range: a.range,
        excerpt_truncated: selected > excerpt.len(),
        excerpt,
        excerpt_sha256: format!("{:x}", digest.finalize()),
    })
}
fn replay(head: &TaskHead, request: &AnnotationRequest) -> Result<Option<AnnotationResult>> {
    head.annotation_request(&request.key)?
        .map(|(thread, entry)| {
            require(
                entry.request.payload_sha256 == request.payload_sha256,
                AnnotationCode::RequestKeyConflict,
                "request key already binds another payload",
            )?;
            Ok(AnnotationResult {
                task: head.id.clone(),
                thread,
                entry: entry.seq,
                revision: entry.seq,
                state: entry.state,
                replayed: true,
            })
        })
        .transpose()
}

pub fn open(
    store: &Store,
    actor: &AnnotationActor<'_>,
    task: &str,
    key: &str,
    action: OpenAnnotation,
) -> Result<AnnotationResult> {
    mutate(store, actor, task, key, AnnotationAction::Open(action))
}
pub fn reply(
    store: &Store,
    actor: &AnnotationActor<'_>,
    task: &str,
    key: &str,
    action: ChangeAnnotation,
) -> Result<AnnotationResult> {
    mutate(store, actor, task, key, AnnotationAction::Reply(action))
}
pub fn resolve(
    store: &Store,
    actor: &AnnotationActor<'_>,
    task: &str,
    key: &str,
    action: ChangeAnnotation,
) -> Result<AnnotationResult> {
    mutate(store, actor, task, key, AnnotationAction::Resolve(action))
}
pub fn reopen(
    store: &Store,
    actor: &AnnotationActor<'_>,
    task: &str,
    key: &str,
    action: ChangeAnnotation,
) -> Result<AnnotationResult> {
    mutate(store, actor, task, key, AnnotationAction::Reopen(action))
}
pub fn mutate(
    store: &Store,
    actor: &AnnotationActor<'_>,
    task: &str,
    key: &str,
    action: AnnotationAction,
) -> Result<AnnotationResult> {
    let task = Store::normalize_id(task)?;
    validate_key(key)?;
    action.validate()?;
    validate_text(
        action.text(),
        matches!(
            action.kind(),
            AnnotationKind::Opened | AnnotationKind::Reply
        ),
    )?;
    let request = AnnotationRequest {
        key: key.into(),
        payload_sha256: payload_sha256(&task, &action)?,
    };
    let before = store.head(&task)?;
    actor.admit(store, &before)?;
    if let Some(result) = replay(&before, &request)? {
        return Ok(result);
    }
    let prepared = if let AnnotationAction::Open(a) = &action {
        let c = current(&before, a)?;
        Some(anchor(&before, a, &c).map_err(|error| {
            if error.downcast_ref::<AnnotationError>().is_some() {
                error
            } else {
                annotation_error(
                    AnnotationCode::CandidateUnavailable,
                    "Git anchor unavailable",
                    json!(error.to_string()),
                )
            }
        })?)
    } else {
        None
    };
    let mut result = None;
    store.try_update_head(&task, |head| {
        let author = actor.admit(store, head.head())?;
        if let Some(old) = replay(head.head(), &request)? {
            result = Some(old);
            return Ok(false);
        }
        let mut thread = match &action {
            AnnotationAction::Open(a) => {
                let c = current(head.head(), a)?;
                let anchor = prepared.as_ref().context("prepared anchor")?;
                require(
                    c.tree == anchor.tree,
                    AnnotationCode::StaleCandidate,
                    "candidate tree moved",
                )?;
                AnnotationThread {
                    version: 1,
                    id: format!("N{}", head.head().annotation_summary.next_thread.max(1)),
                    anchor: anchor.clone(),
                    revision: 0,
                    state: AnnotationState::Open,
                    entries: vec![],
                }
            }
            AnnotationAction::Reply(a)
            | AnnotationAction::Resolve(a)
            | AnnotationAction::Reopen(a) => {
                let t = head.head().annotation_thread(&a.thread)?.ok_or_else(|| {
                    annotation_error(
                        AnnotationCode::ThreadNotFound,
                        "thread not found",
                        json!({"thread":a.thread}),
                    )
                })?;
                if t.revision != a.expected_revision {
                    return Err(annotation_error(
                        AnnotationCode::ThreadRevisionConflict,
                        "thread revision moved",
                        json!({"revision":t.revision,"state":t.state}),
                    ));
                }
                require(
                    !matches!(
                        (action.kind(), t.state),
                        (AnnotationKind::Resolved, AnnotationState::Resolved)
                            | (AnnotationKind::Reopened, AnnotationState::Open)
                    ),
                    AnnotationCode::InvalidTransition,
                    "state already matches transition",
                )?;
                t
            }
        };
        let seq = thread.revision + 1;
        let state = action.kind().state(thread.state);
        let entry = AnnotationEntry {
            version: 1,
            seq,
            kind: action.kind(),
            state,
            author,
            at: store.now(),
            text: action.text().into(),
            request: request.clone(),
        };
        result = Some(AnnotationResult {
            task: task.clone(),
            thread: thread.id.clone(),
            entry: seq,
            revision: seq,
            state,
            replayed: false,
        });
        let id = thread.id.clone();
        thread.entries.clear();
        head.append_annotation(thread, entry, matches!(action, AnnotationAction::Open(_)))?;
        store.log_event(
            Some(&task),
            action.kind().event(),
            json!({"thread":id,"entry":seq}),
        )?;
        Ok(true)
    })?;
    result.context("annotation mutation returned no result")
}

fn status(
    thread: &AnnotationThread,
    head: &TaskHead,
    availability: &CandidateAvailability,
) -> AnchorStatus {
    if head
        .current_run
        .as_ref()
        .is_some_and(|r| r.id != thread.anchor.run)
    {
        return AnchorStatus::Superseded;
    }
    match availability {
        CandidateAvailability::Unavailable { reason } => AnchorStatus::Unavailable {
            reason: reason.clone(),
        },
        CandidateAvailability::Available { candidate: c } => {
            let a = &thread.anchor;
            let changes: Vec<_> = [
                ("run", &a.run, &c.run),
                ("base", &a.base, &c.base),
                ("head", &a.head, &c.head),
                ("tree", &a.tree, &c.tree),
            ]
            .into_iter()
            .filter(|(_, x, y)| x != y)
            .map(|(field, x, y)| IdentityChange {
                field: field.into(),
                expected: x.clone(),
                actual: y.clone(),
            })
            .collect();
            if changes.is_empty() {
                AnchorStatus::Current
            } else {
                AnchorStatus::Stale { changes }
            }
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    task: String,
    thread: Option<String>,
    state: Option<AnnotationState>,
    anchor: Option<String>,
    /// Bounded membership and upper revisions frozen at the start of a traversal.
    snapshot: Vec<(String, u32)>,
    pos: usize,
}
fn encode(c: &Cursor) -> Result<String> {
    let bytes = serde_json::to_vec(c)?;
    Ok(format!(
        "{}:{}",
        hash(&bytes),
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
fn decode(value: &str) -> Result<Cursor> {
    let parse = || -> Result<Cursor> {
        require(
            value.len() <= 16384,
            AnnotationCode::InvalidCursor,
            "cursor too large",
        )?;
        let (sum, hex) = value.split_once(':').context("cursor encoding")?;
        anyhow::ensure!(hex.len() % 2 == 0, "cursor hex");
        let bytes = hex
            .as_bytes()
            .chunks(2)
            .map(|b| Ok(u8::from_str_radix(std::str::from_utf8(b)?, 16)?))
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(hash(&bytes) == sum, "cursor checksum");
        let c: Cursor = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            c.version == 1 && c.snapshot.len() <= TASK_THREADS as usize,
            "cursor version/bound"
        );
        Ok(c)
    };
    parse().map_err(|e| {
        annotation_error(
            AnnotationCode::InvalidCursor,
            "invalid cursor",
            json!(e.to_string()),
        )
    })
}
fn item(t: &AnnotationThread, head: &TaskHead, c: &CandidateAvailability) -> AnnotationItem {
    AnnotationItem {
        id: t.id.clone(),
        revision: t.revision,
        state: t.state,
        anchor: Some(t.anchor.clone()),
        anchor_status: status(t, head, c),
    }
}
pub fn list(store: &Store, task: &str, q: &AnnotationsQuery) -> Result<AnnotationsView> {
    read(store, task, None, q)
}
pub fn show(
    store: &Store,
    task: &str,
    thread: &str,
    q: &AnnotationsQuery,
) -> Result<AnnotationsView> {
    read(store, task, Some(thread), q)
}
fn read(
    store: &Store,
    task: &str,
    thread: Option<&str>,
    q: &AnnotationsQuery,
) -> Result<AnnotationsView> {
    let head = store.head(task)?;
    let candidate = candidate(&head);
    require(
        q.limit.is_none_or(|n| (1..=64).contains(&n)),
        AnnotationCode::InvalidCursor,
        "limit must be 1..64",
    )?;
    require(
        q.anchor
            .as_deref()
            .is_none_or(|s| matches!(s, "current" | "stale" | "superseded" | "unavailable")),
        AnnotationCode::InvalidCursor,
        "unknown anchor filter",
    )?;
    let mut cursor = if let Some(cursor) = &q.cursor {
        let c = decode(cursor)?;
        require(
            c.task == head.id
                && c.thread.as_deref() == thread
                && c.state == q.state
                && c.anchor == q.anchor,
            AnnotationCode::InvalidCursor,
            "cursor is bound to another query",
        )?;
        c
    } else {
        let threads = if let Some(id) = thread {
            vec![head.annotation_thread(id)?.ok_or_else(|| {
                annotation_error(
                    AnnotationCode::ThreadNotFound,
                    "thread not found",
                    Value::Null,
                )
            })?]
        } else {
            head.annotation_threads()?
        };
        let snapshot = threads
            .into_iter()
            .filter(|t| {
                q.state.is_none_or(|s| s == t.state)
                    && q.anchor.as_deref().is_none_or(|s| {
                        serde_json::to_value(status(t, &head, &candidate))
                            .ok()
                            .and_then(|v| v["status"].as_str().map(str::to_owned))
                            .as_deref()
                            == Some(s)
                    })
            })
            .map(|t| (t.id, t.revision))
            .collect();
        Cursor {
            version: 1,
            task: head.id.clone(),
            thread: thread.map(str::to_owned),
            state: q.state,
            anchor: q.anchor.clone(),
            snapshot,
            pos: 0,
        }
    };
    let mut view = AnnotationsView {
        version: 1,
        task: head.id.clone(),
        candidate,
        counts: head.annotation_summary.clone(),
        paths: vec![],
        paths_truncated: false,
        page: AnnotationsPage::List { items: vec![] },
        next_cursor: None,
    };
    let max = q.limit.unwrap_or(16) as usize;
    let start = cursor.pos;
    if let Some(id) = thread {
        require(
            cursor.snapshot.len() == 1 && cursor.snapshot[0].0 == id,
            AnnotationCode::InvalidCursor,
            "thread cursor membership",
        )?;
        let upper = cursor.snapshot[0].1;
        let mut t = head.annotation_thread(id)?.context("thread disappeared")?;
        require(
            upper > 0 && upper <= t.revision && cursor.pos < upper as usize,
            AnnotationCode::InvalidCursor,
            "thread cursor ordinal",
        )?;
        t.revision = upper;
        t.state = head
            .annotation_entry(t.number()?, upper)?
            .context("missing upper entry")?
            .state;
        // Entry pages do not repeat the anchor (available in list); reserve room for any
        // legal 4-KiB note even with worst-case JSON escaping.
        let mut summary = item(&t, &head, &view.candidate);
        summary.anchor = None;
        view.page = AnnotationsPage::Thread {
            thread: summary,
            entries: vec![],
            upper_revision: upper,
        };
        while cursor.pos < upper as usize {
            let e = head
                .annotation_entry(t.number()?, cursor.pos as u32 + 1)?
                .context("missing annotation entry")?;
            let old = view.clone();
            if let AnnotationsPage::Thread { entries, .. } = &mut view.page {
                entries.push(e);
            }
            cursor.pos += 1;
            view.next_cursor = (cursor.pos < upper as usize)
                .then(|| encode(&cursor))
                .transpose()?;
            if serde_json::to_vec(&view)?.len() > RESPONSE_BYTES {
                cursor.pos -= 1;
                view = old;
                break;
            }
            if let AnnotationsPage::Thread { entries, .. } = &view.page {
                if entries.len() >= max {
                    break;
                }
            }
        }
    } else {
        require(
            cursor.pos <= cursor.snapshot.len(),
            AnnotationCode::InvalidCursor,
            "list cursor position",
        )?;
        while cursor.pos < cursor.snapshot.len() {
            let (id, upper) = &cursor.snapshot[cursor.pos];
            let mut t = head.annotation_thread(id)?.context("thread disappeared")?;
            require(
                *upper > 0 && *upper <= t.revision,
                AnnotationCode::InvalidCursor,
                "list cursor revision",
            )?;
            t.revision = *upper;
            t.state = head
                .annotation_entry(t.number()?, *upper)?
                .context("missing upper entry")?
                .state;
            let old = view.clone();
            if let AnnotationsPage::List { items } = &mut view.page {
                items.push(item(&t, &head, &view.candidate));
            }
            cursor.pos += 1;
            view.next_cursor = (cursor.pos < cursor.snapshot.len())
                .then(|| encode(&cursor))
                .transpose()?;
            if serde_json::to_vec(&view)?.len() > RESPONSE_BYTES {
                cursor.pos -= 1;
                view = old;
                break;
            }
            if let AnnotationsPage::List { items } = &view.page {
                if items.len() >= max {
                    break;
                }
            }
        }
    }
    let more = if thread.is_some() {
        cursor.pos < cursor.snapshot[0].1 as usize
    } else {
        cursor.pos < cursor.snapshot.len()
    };
    require(
        !more || cursor.pos > start,
        AnnotationCode::AnnotationLimit,
        "one item exceeds the response budget",
    )?;
    view.next_cursor = more.then(|| encode(&cursor)).transpose()?;
    require(
        serde_json::to_vec(&view)?.len() <= RESPONSE_BYTES,
        AnnotationCode::AnnotationLimit,
        "response envelope exceeds 32 KiB",
    )?;
    if let CandidateAvailability::Available { candidate: c } = &view.candidate {
        let repo = &head
            .current_run
            .as_ref()
            .context("annotation run")?
            .worktree;
        let paths = corral_git::committed_paths(repo, &c.base, &c.head)?;
        for path in paths {
            if path.len() > 1024 {
                view.paths_truncated = true;
                continue;
            }
            view.paths.push(path);
            if view.paths.len() > 64 || serde_json::to_vec(&view)?.len() > RESPONSE_BYTES - 32 {
                view.paths.pop();
                view.paths_truncated = true;
                break;
            }
        }
    }
    Ok(view)
}
