//! Bounded review-screen formatting.
pub(super) fn truncate_lines(text: String, max: usize) -> String {
    if text.lines().count() <= max {
        return text;
    }
    let mut out: String = text.lines().take(max).flat_map(|l| [l, "\n"]).collect();
    out.push_str("... (truncated)\n");
    out
}

pub(super) fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}
