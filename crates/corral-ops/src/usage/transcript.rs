//! Bounded transcript output for one run's provider window.
use super::*;

/// The reviewer transcript of a review child: only its cwd and time window, not other rounds
/// sharing a session.
pub(crate) fn transcript_head(run: &corral_core::RunHead, store: &Store) -> Result<Vec<u8>> {
    let harness = harness_of(run.harness.as_deref(), &run.agent, &store.config);
    transcript_of(harness, (&run.worktree, run.started, run.ended), store)
}

pub(crate) fn has_transcript(run: &Run, store: &Store) -> bool {
    matches!(run_harness(run, &store.config), "omo" | "omp" | "claude")
}

pub(crate) fn transcript_run(run: &Run, store: &Store) -> Result<Vec<u8>> {
    transcript_of(
        run_harness(run, &store.config),
        (&run.worktree, run.started, run.ended),
        store,
    )
}

fn transcript_of(
    harness: &str,
    (worktree, started, ended): (&Path, Timestamp, Option<Timestamp>),
    store: &Store,
) -> Result<Vec<u8>> {
    let (root, claude) = session_root_of(harness, worktree, store)?;
    let end = ended.unwrap_or_else(Timestamp::now);
    let mut bytes = Vec::new();
    for path in session_paths_of(&root, worktree, claude)? {
        let mut matched = false;
        let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        for line in BufReader::new(file).lines() {
            let line = line.context("reading reviewer transcript")?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: Entry =
                serde_json::from_str(&line).context("parsing reviewer transcript")?;
            if let Some(cwd) = entry.cwd {
                matched = cwd == worktree;
            }
            if matched && entry.timestamp.is_some_and(|at| at >= started && at <= end) {
                bytes.extend_from_slice(line.as_bytes());
                bytes.push(b'\n');
                if u64::try_from(bytes.len()).context("transcript length overflow")?
                    > store.config.artifacts.max_bytes
                {
                    bail!("reviewer transcript exceeds artifact max_bytes");
                }
            }
        }
    }
    Ok(bytes)
}
