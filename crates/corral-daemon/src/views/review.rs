//! Review reads the exact patch registered by `done`, never the mutable worktree.
use std::{io::Read, ops::Range};

use anyhow::{Context, Result, ensure};
use corral_engine::{Artifact, Engine, Role, Run, Status};
use corral_protocol::detail::{
    DetailBody, DetailGet, DetailReply, PatchPage, ReviewCandidate, ReviewDetail, ReviewFile,
};

use super::{
    NotFound,
    details::{offset, reply},
    fingerprint,
};

const PATCH_BYTES: usize = 8 * 1024; // Safe for the worst JSON escaping.

fn submission(engine: &Engine, task: &str) -> Result<(Option<Run>, Option<Artifact>)> {
    let run = engine
        .store
        .latest_run(task, Role::Worker)?
        .filter(|run| run.summary.is_some());
    let diff = if let Some(run) = &run {
        if let Some(reference) = engine.store.submitted_diff_ref(task, &run.id)? {
            Some(
                engine
                    .store
                    .get_artifact(&reference)?
                    .context("submitted diff missing from artifact catalog")?,
            )
        } else {
            None
        }
    } else {
        None
    };
    Ok((run, diff))
}

fn patch(engine: &Engine, diff: &Artifact) -> Result<String> {
    let mut patch = String::new();
    engine
        .open_artifact_blob(diff)?
        .read_to_string(&mut patch)
        .context("read submitted patch")?;
    Ok(patch)
}

pub(super) fn detail(engine: &Engine, id: &str) -> Result<ReviewDetail> {
    let task = engine
        .store
        .get_task(id)?
        .ok_or(NotFound("task not found"))?;
    let (run, diff) = submission(engine, id)?;
    let run = run.ok_or(NotFound("task has no submitted worker run"))?;
    let max_rounds = engine.config.task_max_rounds(&task);
    let files = if let Some(diff) = &diff {
        sections(&patch(engine, diff)?)
            .into_iter()
            .map(|section| section.file)
            .collect()
    } else {
        Vec::new()
    };
    Ok(ReviewDetail {
        task: task.id,
        run: Some(run),
        diff,
        checks: engine.store.list_checks(id)?,
        max_rounds,
        files,
    })
}

pub(super) fn file(
    engine: &Engine,
    query: &DetailGet,
    seq: u64,
    task: &str,
    path: &str,
) -> Result<DetailReply> {
    let task = engine
        .store
        .get_task(task)?
        .ok_or(NotFound("task not found"))?;
    let (run, diff) = submission(engine, &task.id)?;
    let run = run.ok_or(NotFound("task has no submitted worker run"))?;
    let diff = diff.ok_or(NotFound("task has no stored review patch"))?;
    // Landing can rewrite the run's base and head independently of its stored
    // patch. Do not attach a mutable or retired run's coordinates to that patch.
    let candidate = if task.status == Status::Review && engine.observed_head(&run)? == run.head_sha
    {
        Some(ReviewCandidate {
            run: run.id,
            base: run.base_sha,
            head: run.head_sha.context("submitted commit unavailable")?,
        })
    } else {
        None
    };
    let etag = fingerprint(&(&query.target, &diff, &candidate))?;
    let patch = patch(engine, &diff)?;
    let section = sections(&patch)
        .into_iter()
        .find(|section| section.file.path == path)
        .ok_or(NotFound("file not found in submitted patch"))?;
    let patch = &patch[section.range];
    let at = offset(query.page.as_deref(), &etag)?;
    ensure!(
        at < patch.len() && patch.is_char_boundary(at),
        "invalid: patch page offset"
    );
    let end = patch.floor_char_boundary(at.saturating_add(PATCH_BYTES).min(patch.len()));
    let truncated = end < patch.len();
    let next = truncated.then(|| format!("{etag}:{end}"));
    let body = PatchPage {
        path: section.file.path,
        patch: patch[at..end].to_owned(),
        truncated,
        candidate,
    };
    reply(query, seq, etag, DetailBody::ReviewFile(body), next)
}

struct Section {
    file: ReviewFile,
    range: Range<usize>,
}

fn sections(patch: &str) -> Vec<Section> {
    let mut output = Vec::new();
    let mut section: Option<Section> = None;
    let mut in_hunk = false;
    let mut at = 0;
    for line in patch.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let line = line.strip_suffix('\n').unwrap_or(line);
        if let Some(header) = line.strip_prefix("diff --git ") {
            if let Some(mut prior) = section.take() {
                prior.range.end = start;
                output.push(prior);
            }
            in_hunk = false;
            section = Some(Section {
                file: ReviewFile {
                    path: header_path(header),
                    added: 0,
                    deleted: 0,
                },
                range: start..at,
            });
        } else if let Some(current) = &mut section {
            if line.starts_with("@@") {
                in_hunk = true;
            } else if in_hunk {
                match line.as_bytes().first() {
                    Some(b'+') => current.file.added = current.file.added.saturating_add(1),
                    Some(b'-') => current.file.deleted = current.file.deleted.saturating_add(1),
                    _ => {}
                }
            } else if let Some(path) = line
                .strip_prefix("rename to ")
                .or_else(|| line.strip_prefix("copy to "))
            {
                current.file.path = decode_path(path);
            } else if let Some(path) = line
                .strip_prefix("+++ ")
                .filter(|path| *path != "/dev/null")
            {
                current.file.path = marker_path(path);
            } else if let Some(path) = line
                .strip_prefix("--- ")
                .filter(|path| *path != "/dev/null")
            {
                current.file.path = marker_path(path);
            }
        }
    }
    if let Some(mut section) = section {
        section.range.end = at;
        output.push(section);
    }
    output
}

fn marker_path(path: &str) -> String {
    // Git adds a trailing tab to unquoted ---/+++ paths containing spaces.
    let path = decode_path(path.trim_end_matches('\t'));
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .map_or_else(|| path.clone(), str::to_owned)
}

fn header_path(header: &str) -> String {
    // Header-only diffs (mode changes, binary changes) may not have markers.
    // For equal names, find the separator using the exact repeated old/new path.
    if let Some((old, new)) = header.strip_prefix("a/").and_then(|header| {
        header.match_indices(" b/").find_map(|(i, delimiter)| {
            let new = &header[i + delimiter.len()..];
            (&header[..i] == new).then_some((&header[..i], new))
        })
    }) {
        debug_assert_eq!(old, new);
        return new.to_owned();
    }
    if let Some((_, new)) = header.rsplit_once(" \"b/") {
        // Prefixing the quote restores Git's C-quoted path format.
        if let Some(path) = new.strip_suffix('"') {
            return decode_quoted(path);
        }
    }
    if let Some((_, new)) = header.rsplit_once(" b/") {
        return new.to_owned();
    }
    // Honors default, custom and no-prefix headers for the common equal-path case.
    if let Some(header) = header.strip_prefix('"')
        && let Some(end) = unescaped_quote(header)
    {
        return marker_path(header[end + 1..].trim_start());
    }
    header
        .split_once(' ')
        .map_or(header, |(_, new)| new)
        .to_owned()
}

fn unescaped_quote(text: &str) -> Option<usize> {
    let mut escape = false;
    for (at, byte) in text.bytes().enumerate() {
        if !escape && byte == b'"' {
            return Some(at);
        }
        escape = !escape && byte == b'\\';
    }
    None
}

fn decode_path(path: &str) -> String {
    if let Some(body) = path
        .strip_prefix('"')
        .and_then(|body| body.strip_suffix('"'))
    {
        decode_quoted(body)
    } else {
        path.to_owned()
    }
}

fn decode_quoted(path: &str) -> String {
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        let Some(next) = bytes.next() else { break };
        decoded.push(match next {
            b'a' => 7,
            b'b' => 8,
            b't' => 9,
            b'n' => 10,
            b'v' => 11,
            b'f' => 12,
            b'r' => 13,
            b'0'..=b'3' => {
                let middle = bytes.next().and_then(octal).unwrap_or(0);
                let last = bytes.next().and_then(octal).unwrap_or(0);
                (next - b'0') * 64 + middle * 8 + last
            }
            value => value,
        });
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn octal(byte: u8) -> Option<u8> {
    (b'0'..=b'7').contains(&byte).then(|| byte - b'0')
}
