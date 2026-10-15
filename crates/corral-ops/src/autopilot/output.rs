//! Formatting helpers for autopilot diagnostics.

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
