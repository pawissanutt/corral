//! Bounded, read-only transcript pages using the accounting locators.
use super::*;
use std::io::Read;

pub const PAGE_BYTES: usize = 24 * 1024;
const SCAN_BYTES: u64 = 256 * 1024 * 1024;
const RECORD_BYTES: usize = 1024 * 1024;

#[cfg(test)]
#[path = "pages_tests.rs"]
mod tests;

/// Line numbers are one-based, inclusive, over matching JSONL records, across generations.
pub fn transcript_page(
    store: &Store,
    task: &str,
    run: &str,
    review: Option<usize>,
    first: usize,
    last: usize,
) -> Result<String> {
    if first == 0 || last < first || last - first >= 200 {
        bail!("invalid transcript range: use 1-200 (at most 200 lines)");
    }
    let task = store.get(task)?;
    let worker = task
        .runs
        .iter()
        .find(|r| r.id == run)
        .context("run not found")?;
    let selected = match review {
        Some(n) => worker
            .review_runs
            .get(n.checked_sub(1).context("review numbers start at 1")?)
            .context("review child not found")?,
        None => worker,
    };
    page(selected, store, first, last)
}

fn page(run: &Run, store: &Store, first: usize, last: usize) -> Result<String> {
    let (paths, searched, bound) = if crate::agent::hooks::run_is_pi_native(run) {
        let mut paths = Vec::new();
        if let Some(state) = run.agent_host.as_deref() {
            for bound in state.generations.iter().filter_map(|g| g.bound.as_ref()) {
                if !paths.contains(&bound.session_file) {
                    paths.push(bound.session_file.clone());
                }
            }
        }
        let searched = if paths.is_empty() {
            format!("run {} agent_host.generations[].bound.session_file", run.id)
        } else {
            format!("{paths:?}")
        };
        (paths, searched, true)
    } else {
        let (root, claude) = session_root(run, store).map_err(|e| {
            anyhow::anyhow!(
                "transcript_unavailable: run {} session locator: {e:#}",
                run.id
            )
        })?;
        let searched = run
            .session
            .as_ref()
            .and_then(|s| s.transcript.as_ref())
            .unwrap_or(&root)
            .display()
            .to_string();
        let paths = session_paths(&root, run, claude)
            .map_err(|e| anyhow::anyhow!("transcript_unavailable: searched {searched}: {e:#}"))?;
        (paths, searched, false)
    };
    let mut output = String::new();
    let mut count = 0;
    let mut scanned = 0u64;
    for path in paths {
        let meta = fs::symlink_metadata(&path)
            .with_context(|| format!("transcript_unavailable: searched {}", path.display()))?;
        if !meta.is_file() {
            bail!(
                "transcript_unavailable: searched {}: not a regular file",
                path.display()
            );
        }
        let file = File::open(&path)
            .with_context(|| format!("transcript_unavailable: searched {}", path.display()))?;
        if !file.metadata()?.is_file() {
            bail!(
                "transcript_unavailable: searched {}: not a regular file",
                path.display()
            );
        }
        let mut reader = BufReader::new(file.take(SCAN_BYTES + 1));
        let mut matched = bound;
        loop {
            let mut line = Vec::new();
            // A corrupt or huge record cannot allocate unbounded memory.
            let n = reader
                .by_ref()
                .take((RECORD_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
                .with_context(|| format!("transcript_unavailable: searched {}", path.display()))?;
            if n == 0 {
                break;
            }
            scanned += n as u64;
            if n > RECORD_BYTES || scanned > SCAN_BYTES {
                bail!(
                    "transcript_unavailable: searched {}: transcript scan/record limit exceeded",
                    path.display()
                );
            }
            let value: serde_json::Value = serde_json::from_slice(&line).with_context(|| {
                format!(
                    "transcript_unavailable: searched {}: invalid JSONL",
                    path.display()
                )
            })?;
            if !bound {
                if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
                    matched = Path::new(cwd) == run.worktree;
                }
                let at = value
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<Timestamp>().ok());
                if !matched
                    || !at.is_some_and(|at| {
                        at >= run.started && at <= run.ended.unwrap_or_else(Timestamp::now)
                    })
                {
                    continue;
                }
            }
            count += 1;
            if count >= first {
                let text = String::from_utf8_lossy(&line);
                if output.len() + text.len() > PAGE_BYTES - 100 {
                    let next = if output.is_empty() {
                        let mut end = PAGE_BYTES - 200;
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        output.push_str(&text[..end]);
                        output.push_str("\n[record truncated]\n");
                        count + 1
                    } else {
                        count
                    };
                    output.push_str(&format!(
                        "\n[page byte limit; resume with --lines {next}-{}]\n",
                        next + 199
                    ));
                    return Ok(output);
                }
                output.push_str(&text);
            }
            if count == last {
                return Ok(output);
            }
        }
    }
    if count == 0 {
        bail!("transcript_unavailable: searched {searched}: no matching transcript records");
    }
    Ok(output)
}
