//! Formatting helpers for autopilot diagnostics.

use anyhow::{Context, Result};
use corral_core::ArtifactKind;

use super::Pilot;

pub(super) const LOG_TAIL_LINES: usize = 30;

pub(super) fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

pub(super) fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

pub(super) fn log_tail(path: &std::path::Path, lines: usize) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..].join("\n")
        }
        Err(e) => format!("(could not read {}: {e})", path.display()),
    }
}

impl Pilot<'_> {
    pub(super) fn snapshot(
        &self,
        name: &str,
        text: &str,
        title: &str,
    ) -> Result<corral_core::Artifact> {
        let tmp = self.ctx.store.state_dir().join("tmp").join(format!(
            "{}-{}-autopilot",
            self.id(),
            self.run.id
        ));
        std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        let file = tmp.join(name);
        let added = std::fs::write(&file, text)
            .with_context(|| format!("writing {}", file.display()))
            .and_then(|()| {
                crate::artifacts::add_artifact_for_run(
                    self.ctx,
                    self.id(),
                    &self.run.id,
                    &file,
                    Some(ArtifactKind::Report),
                    Some(title),
                )
            });
        if let Err(e) = std::fs::remove_dir_all(&tmp) {
            crate::messages::report(
                Some(self.id()),
                "cleanup",
                format!("could not remove {} (ignored): {e}", tmp.display()),
            );
        }
        added
    }
}
