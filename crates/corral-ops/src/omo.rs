//! OmO session-file activity. herdr misdetects OmO panes (docs/herdr-cli.md "Observed facts"), so
//! sync treats a recently written OmO session transcript as proof the agent is working.
//! Path logic ported from omo-scope/src/store.rs (`agent_dir`, `encode_cwd`, `sessions_dir`).

use std::path::{Path, PathBuf};

use corral_core::Run;
use jiff::Timestamp;

/// True when the run was launched with the OmO harness (legacy runs: the agent key).
pub(crate) fn is_omo(run: &Run) -> bool {
    run.harness.as_deref().unwrap_or(&run.agent) == "omo"
}

/// `configured`, else $OMO_CODING_AGENT_DIR, else $SENPI_CODING_AGENT_DIR, else ~/.omo/agent.
pub(crate) fn agent_dir(configured: Option<&Path>) -> PathBuf {
    if let Some(dir) = configured {
        return dir.to_path_buf();
    }
    ["OMO_CODING_AGENT_DIR", "SENPI_CODING_AGENT_DIR"]
        .iter()
        .find_map(|k| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".omo")
                .join("agent")
        })
}

/// `/home/u/repo/.worktrees/T-001` -> `--home-u-repo-.worktrees-T-001--`.
pub(crate) fn encode_cwd(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let body: String = s
        .trim_start_matches(['/', '\\'])
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{body}--")
}

/// Modification time of the newest `*.jsonl` session file OmO keeps for `cwd`, if any.
pub(crate) fn last_activity(agent: &Path, cwd: &Path) -> Option<Timestamp> {
    let dir = agent.join("sessions").join(encode_cwd(cwd));
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .filter_map(|m| m.modified().ok())
        .filter_map(|t| Timestamp::try_from(t).ok())
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_cwd_matches_omo_session_dirs() {
        assert_eq!(
            encode_cwd(Path::new(
                "/home/pawis/miti-workspace/corral/.worktrees/T-001"
            )),
            "--home-pawis-miti-workspace-corral-.worktrees-T-001--"
        );
    }
}
