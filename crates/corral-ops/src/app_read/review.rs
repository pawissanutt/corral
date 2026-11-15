//! Committed-candidate review readers. Only canonical run repositories, immutable
//! object IDs, registered artifact references and candidate-member literal paths are
//! accepted. No working-tree, index, untracked-file or arbitrary host-file reads.
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};

use super::short;
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{RunHead, TaskHead};
use corral_protocol::detail::{
    AnnotationMessage, ArtifactRef, DetailBody, PatchPage, ReviewAnnotation, ReviewCandidate,
    ReviewDetail, ReviewFile, ReviewGate, ReviewVerdict,
};

// Even JSON's worst-case six-byte control-character escaping remains below CONTROL's
// page budget. A page always advances at a UTF-8 boundary; no patch bytes are discarded.
const PATCH_BYTES: usize = 8 * 1024;

struct Candidate {
    repo: PathBuf,
    base: String,
    head: String,
}

fn oid(value: &str) -> Result<()> {
    ensure!(
        (value.len() == 40 || value.len() == 64)
            && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid candidate object ID"
    );
    Ok(())
}

fn candidate(ctx: &Ctx, task: &TaskHead, run: &RunHead) -> Result<Option<Candidate>> {
    let repo = if run.worktree.is_dir() {
        run.worktree.clone()
    } else {
        crate::review::main_repo(ctx, task, run)?
    };
    let recorded = run
        .review
        .as_ref()
        .map(|r| (r.at, r.sha.as_str()))
        .into_iter()
        .chain(run.gate.as_ref().map(|g| (g.at, g.sha.as_str())))
        .chain(
            run.facts
                .latest_checkpoint
                .as_ref()
                .map(|c| (c.captured, c.head.as_str())),
        )
        .max_by_key(|(at, _)| *at)
        .map(|(_, sha)| sha.to_owned());
    // Manual no-gate runs may not yet have a candidate receipt. Reading HEAD is safe;
    // patch_revision binds that exact OID into the gateway continuation's etag.
    let head = if run.worktree.is_dir() {
        Some(corral_git::rev_sha(&repo, "HEAD")?)
    } else {
        recorded
    };
    let Some(head) = head else {
        return Ok(None);
    };
    oid(&head)?;
    let identity = run
        .review
        .as_ref()
        .and_then(|review| review.identity.as_ref())
        .filter(|identity| identity.head == head)
        .or_else(|| {
            run.gate
                .as_ref()
                .and_then(|gate| gate.identity.as_ref())
                .filter(|identity| identity.head == head)
        });
    let base = identity
        .map(|identity| identity.integration_base.clone())
        .or_else(|| {
            run.facts
                .latest_checkpoint
                .as_ref()
                .filter(|c| c.head == head)
                .map(|c| c.base.clone())
        })
        .unwrap_or_else(|| run.base_sha.clone());
    oid(&base)?;
    Ok(Some(Candidate { repo, base, head }))
}

/// Content identity used by the gateway before/after a patch stream, including
/// unrecorded HEAD movement that cannot be represented by the store feed alone.
pub(super) fn patch_revision(ctx: &Ctx, task: &str) -> Result<String> {
    let task = ctx.store.head(task)?;
    let run = task
        .current_run
        .as_deref()
        .context("task has no coding run")?;
    let candidate = candidate(ctx, &task, run)?.context("candidate unavailable")?;
    Ok(format!("{}:{}:{}", run.id, candidate.base, candidate.head))
}

pub(super) fn review(ctx: &Ctx, id: &str) -> Result<DetailBody> {
    let task = ctx.store.head(id)?;
    let run = task
        .current_run
        .as_deref()
        .context("task has no coding run")?;
    let candidate = candidate(ctx, &task, run)?;
    let files = candidate
        .as_ref()
        .map(candidate_files)
        .transpose()?
        .unwrap_or_default();
    let mut verdicts = Vec::new();
    for n in 1..=run.facts.children {
        if let Some(child) = task.review_child_at(&run.id, usize::try_from(n)?)?
            && let Some(review) = child.review.as_ref()
        {
            verdicts.push(verdict(review, candidate.as_ref()));
        }
    }
    if let Some(review) = &run.review {
        let row = verdict(review, candidate.as_ref());
        if !verdicts.iter().any(|old| old == &row) {
            verdicts.push(row);
        }
    }
    let artifacts = task
        .artifacts_of(&run.id)?
        .into_iter()
        .map(|artifact| ArtifactRef {
            source_ref: artifact.path,
            title: short(&artifact.title, 512),
            media_type: None,
        })
        .collect();
    let acceptance = task
        .completion
        .as_ref()
        .map(|manifest| {
            manifest
                .criteria
                .iter()
                .map(|criterion| {
                    short(
                        &format!(
                            "{}: {} — {}",
                            criterion.id, criterion.scenario, criterion.expected
                        ),
                        8192,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let annotations = annotation_threads(&task, run, candidate.as_ref())?;
    let gate_log_tail = gate_tail(ctx, &task, run)?;
    let gates = gates(ctx, &task, run, candidate.as_ref())?;
    let approvable = candidate
        .as_ref()
        .is_some_and(|candidate| approval(ctx, &task, run, candidate).is_ok());
    Ok(DetailBody::Review(ReviewDetail {
        task: task.id.clone(),
        candidate_sha: candidate.as_ref().map(|c| c.head.clone()),
        base: candidate.as_ref().map(|c| c.base.clone()),
        state: task.status.as_str().into(),
        verdicts,
        gates,
        files,
        report: run.summary.as_ref().map(|text| short(text, 4096)),
        acceptance,
        gate_log_tail,
        artifacts,
        annotations,
        approvable,
    }))
}

fn verdict(review: &corral_core::ReviewRecord, candidate: Option<&Candidate>) -> ReviewVerdict {
    ReviewVerdict {
        by: review.reviewer.clone(),
        sha: review.sha.clone(),
        verdict: review.verdict.map_or_else(
            || {
                if review.cancelled {
                    "cancelled"
                } else if review.process_lost {
                    "unavailable"
                } else {
                    "pending"
                }
                .into()
            },
            |value| value.as_str().into(),
        ),
        at: Some(review.at.to_string()),
        text: review.notes.as_ref().map(|v| short(v, 8192)),
        stale: candidate.is_none_or(|candidate| candidate.head != review.sha),
    }
}

fn gates(
    ctx: &Ctx,
    task: &TaskHead,
    run: &RunHead,
    candidate: Option<&Candidate>,
) -> Result<Vec<ReviewGate>> {
    let Some(gate) = &run.gate else {
        return Ok(Vec::new());
    };
    // Use the attempt's captured commands, not mutable policy or rail progress.
    let mut by = if let Some(identity) = &gate.identity
        && let Some(attempt) = ctx
            .store
            .gate_attempt(&task.id, &run.id, &identity.attempt)?
        && let Some(bytes) = ctx.store.blob_bytes(&attempt.commands)?
    {
        serde_json::from_slice::<Vec<String>>(&bytes)
            .context("decoding canonical gate commands")?
            .join("\n")
    } else {
        let mut commands = String::new();
        for receipt in &gate.commands {
            if !commands.is_empty() {
                commands.push('\n');
            }
            commands.push_str(&receipt.command);
        }
        commands
    };
    if by.is_empty() {
        by.push_str("gate");
    }
    Ok(vec![ReviewGate {
        by,
        sha: gate.sha.clone(),
        verdict: if gate.process_lost && gate.result == corral_core::GateResult::Running {
            "unavailable".into()
        } else {
            gate.result.as_str().into()
        },
        at: Some(gate.at.to_string()),
        text: gate.failed_cmd.as_ref().map(|text| short(text, 8192)),
        stale: gate.result == corral_core::GateResult::Stale
            || candidate.is_none_or(|candidate| {
                gate.sha != candidate.head
                    || gate.identity.as_ref().is_some_and(|identity| {
                        identity.head != candidate.head
                            || identity.integration_base != candidate.base
                    })
            }),
    }])
}

fn approval(ctx: &Ctx, task: &TaskHead, run: &RunHead, candidate: &Candidate) -> Result<()> {
    ensure!(
        crate::review::awaiting_owner(task)?,
        "candidate is not awaiting owner review"
    );
    ensure!(run.worktree.is_dir(), "candidate checkout is unavailable");
    ensure!(
        corral_git::rev_sha(&run.worktree, "HEAD")? == candidate.head,
        "candidate moved"
    );
    ensure!(
        !run.gate
            .as_ref()
            .is_some_and(|g| g.result == corral_core::GateResult::Stale),
        "gate is stale"
    );
    crate::recovery::require_ordinary_approval(ctx, &task.id)?;
    crate::assistant::workflow::handoff::admission::require(
        &ctx.store,
        task,
        corral_core::OperatingActionKind::Accept,
        false,
    )?;
    crate::operating::require_completion(task)?;
    crate::plan_target::validate_run(&ctx.store, task, run)?;
    crate::review::require_manual_receipts(ctx, task)?;
    Ok(())
}

fn annotation_threads(
    task: &TaskHead,
    run: &RunHead,
    candidate: Option<&Candidate>,
) -> Result<Vec<ReviewAnnotation>> {
    let mut rows = Vec::new();
    for thread in task.annotation_threads()? {
        let number = thread
            .id
            .strip_prefix('N')
            .context("invalid annotation thread ID")?
            .parse::<u32>()
            .context("invalid annotation ordinal")?;
        let mut messages = Vec::new();
        for seq in 1..=thread.revision {
            let entry = task
                .annotation_entry(number, seq)?
                .context("annotation entry is missing")?;
            messages.push(AnnotationMessage {
                by: "owner".into(),
                at: entry.at.to_string(),
                text: entry.text,
            });
        }
        let anchor = thread.anchor;
        rows.push(ReviewAnnotation {
            id: thread.id,
            revision: thread.revision,
            run: anchor.run.clone(),
            base: anchor.base.clone(),
            head: anchor.head.clone(),
            path: anchor.path,
            side: match anchor.side {
                corral_core::annotations::AnnotationSide::New => {
                    corral_protocol::cmd::annotation::AnnotationSide::New
                }
                corral_core::annotations::AnnotationSide::Old => {
                    corral_protocol::cmd::annotation::AnnotationSide::Old
                }
            },
            range: anchor
                .range
                .map(|range| corral_protocol::cmd::annotation::LineRange {
                    start: range.start,
                    end: range.end,
                }),
            resolved: thread.state == corral_core::annotations::AnnotationState::Resolved,
            stale: anchor.run != run.id
                || candidate.is_none_or(|candidate| {
                    anchor.head != candidate.head || anchor.base != candidate.base
                }),
            messages,
        });
    }
    Ok(rows)
}

fn gate_tail(ctx: &Ctx, task: &TaskHead, run: &RunHead) -> Result<Vec<String>> {
    let Some(identity) = run.gate.as_ref().and_then(|gate| gate.identity.as_ref()) else {
        return Ok(Vec::new());
    };
    let Some(attempt) = ctx
        .store
        .gate_attempt(&task.id, &run.id, &identity.attempt)?
    else {
        return Ok(Vec::new());
    };
    let Some(log) = attempt.log else {
        return Ok(Vec::new());
    };
    let Some(mut reader) = ctx.store.blob_reader(log.hash())? else {
        return Ok(Vec::new());
    };
    // Verification readers hash the whole stream, but memory and transferred tail
    // stay bounded even for large logs. Never open the gate's host output path.
    let mut tail = VecDeque::with_capacity(8192);
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader
            .read(&mut buffer)
            .context("reading canonical gate log")?;
        if n == 0 {
            break;
        }
        for byte in &buffer[..n] {
            if tail.len() == 8192 {
                tail.pop_front();
            }
            tail.push_back(*byte);
        }
    }
    let bytes: Vec<_> = tail.into_iter().collect();
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<_> = text
        .lines()
        .rev()
        .take(60)
        .map(|line| short(line, 1024))
        .collect();
    lines.reverse();
    Ok(lines)
}

pub(super) fn review_file(
    ctx: &Ctx,
    id: &str,
    path: &str,
    offset: usize,
) -> Result<(DetailBody, Option<usize>)> {
    corral_core::annotations::validate_path(path)?;
    let task = ctx.store.head(id)?;
    let run = task
        .current_run
        .as_deref()
        .context("task has no coding run")?;
    let candidate = candidate(ctx, &task, run)?.context("candidate unavailable")?;
    ensure!(
        candidate_files(&candidate)?
            .iter()
            .any(|file| file.path == path),
        "path is not part of the selected candidate's recorded base..head diff"
    );
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--submodule=short",
        "--unified=3",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        &candidate.base,
        &candidate.head,
        "--",
        path,
    ];
    let (patch, next) = git_stream(&candidate.repo, &args, |stdout| {
        let mut remaining = offset;
        let mut buffer = [0u8; 8192];
        while remaining > 0 {
            let limit = remaining.min(buffer.len());
            let n = stdout
                .read(&mut buffer[..limit])
                .context("seeking patch page")?;
            ensure!(n != 0, "patch offset exceeds content");
            remaining -= n;
        }
        let mut bytes = Vec::with_capacity(PATCH_BYTES + 4);
        stdout
            .take((PATCH_BYTES + 4) as u64)
            .read_to_end(&mut bytes)
            .context("reading bounded candidate patch")?;
        let full = bytes.len() == PATCH_BYTES + 4;
        let end = match std::str::from_utf8(&bytes) {
            Ok(text) => {
                let mut end = text.len().min(PATCH_BYTES);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                end
            }
            Err(error) if full && error.error_len().is_none() => {
                let text = std::str::from_utf8(&bytes[..error.valid_up_to()])?;
                let mut end = text.len().min(PATCH_BYTES);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                end
            }
            Err(error) => {
                return Err(error).context("candidate patch is not UTF-8 or offset is invalid");
            }
        };
        let next = (full || end < bytes.len())
            .then_some(offset.checked_add(end).context("patch offset overflow")?);
        let patch = std::str::from_utf8(&bytes[..end])?.to_owned();
        Ok(((patch, next), full))
    })?;
    Ok((
        DetailBody::ReviewFile(PatchPage {
            path: path.into(),
            patch,
            truncated: next.is_some(),
            candidate: Some(ReviewCandidate {
                run: run.id.to_string(),
                base: candidate.base,
                head: candidate.head,
            }),
        }),
        next,
    ))
}

fn candidate_files(candidate: &Candidate) -> Result<Vec<ReviewFile>> {
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--numstat",
        "-z",
        &candidate.base,
        &candidate.head,
        "--",
    ];
    git_stream(&candidate.repo, &args, |stdout| {
        let mut reader = BufReader::new(stdout);
        let mut files = Vec::new();
        let mut record = Vec::with_capacity(1024);
        loop {
            record.clear();
            let n = reader
                .by_ref()
                .take(8193)
                .read_until(0, &mut record)
                .context("reading committed candidate file list")?;
            if n == 0 {
                break;
            }
            ensure!(
                record.last() == Some(&0) && record.len() <= 8192,
                "candidate file metadata exceeds bound"
            );
            record.pop();
            let text = std::str::from_utf8(&record).context("candidate file path is not UTF-8")?;
            let mut columns = text.splitn(3, '\t');
            let added = columns.next().context("missing candidate added count")?;
            let deleted = columns.next().context("missing candidate deleted count")?;
            let path = columns.next().context("missing candidate file path")?;
            let count = |value: &str| -> Result<u32> {
                if value == "-" {
                    return Ok(0);
                } // Binary files have no text-line count.
                Ok(value.parse::<u64>()?.min(u64::from(u32::MAX)) as u32)
            };
            files.push(ReviewFile {
                path: path.into(),
                added: count(added)?,
                deleted: count(deleted)?,
            });
        }
        Ok((files, false))
    })
}

/// Fixed argv, literal pathspecs, no executable diff drivers, inherited Git
/// repository/config redirection, shell or pager. Every started child is reaped.
fn git_stream<T>(
    repo: &Path,
    args: &[&str],
    consume: impl FnOnce(&mut ChildStdout) -> Result<(T, bool)>,
) -> Result<T> {
    corral_git::work::record();
    let _timer = corral_git::work::timed();
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("GIT_")
            || text.starts_with("CORRAL_GATE_")
            || text.starts_with("HERDR_")
        {
            command.env_remove(name);
        }
    }
    let mut child = command
        .arg("--no-pager")
        .arg("--literal-pathspecs")
        .args(["-c", "core.fsmonitor=false", "-c", "diff.algorithm=myers"])
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning committed candidate diff")?;
    let result = match child.stdout.take() {
        Some(mut stdout) => consume(&mut stdout),
        None => Err(anyhow::anyhow!("candidate diff stdout unavailable")),
    };
    let stopped = result.as_ref().is_ok_and(|(_, stopped)| *stopped);
    if stopped || result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().context("reaping committed candidate diff")?;
    let (value, _) = result?;
    ensure!(
        stopped || status.success(),
        "committed candidate diff failed: {status}"
    );
    Ok(value)
}

/// Page borrowed UTF-8 chunks without allocating a complete plan-revision patch.
pub(super) fn text_page<'a>(
    chunks: impl IntoIterator<Item = &'a str>,
    offset: usize,
) -> Result<(String, Option<usize>)> {
    let mut skip = offset;
    let mut patch = String::with_capacity(PATCH_BYTES);
    for chunk in chunks {
        if skip >= chunk.len() {
            skip -= chunk.len();
            continue;
        }
        ensure!(
            chunk.is_char_boundary(skip),
            "patch offset is not a UTF-8 boundary"
        );
        let chunk = &chunk[skip..];
        skip = 0;
        let mut take = chunk.len().min(PATCH_BYTES - patch.len());
        while !chunk.is_char_boundary(take) {
            take -= 1;
        }
        patch.push_str(&chunk[..take]);
        if take < chunk.len() {
            let next = offset
                .checked_add(patch.len())
                .context("patch offset overflow")?;
            return Ok((patch, Some(next)));
        }
    }
    ensure!(skip == 0, "patch offset exceeds content");
    Ok((patch, None))
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
