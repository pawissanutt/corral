//! The branches a landing target field offers beside what is typed: the project's local
//! branches as corrald lists them, newest tip first. The field still takes any typed name; the
//! engine decides whether it exists.

use crate::model::daemon::Feed;

/// How many branches a field offers at once.
const SHOWN: usize = 6;

/// The caption before the branches a field offers.
pub const BRANCHES_CAPTION: &str = "Branches";

/// The local branches of `project`, newest tip first; empty when corrald does not list them.
pub fn branches(feed: &Feed, project: Option<&str>) -> Vec<String> {
    project
        .and_then(|id| feed.projects.get(id))
        .and_then(|row| row.local_branches.clone())
        .unwrap_or_default()
}

/// The branches to offer for `typed`: those whose names contain it (ignoring case), or the
/// newest ones while the field is empty or already names a listed branch; never the typed one
/// itself.
pub fn suggest(branches: &[String], typed: &str) -> Vec<String> {
    let typed = typed.trim();
    let listed = branches.iter().any(|name| name == typed);
    let needle = typed.to_lowercase();
    branches
        .iter()
        .filter(|name| name.as_str() != typed)
        .filter(|name| listed || name.to_lowercase().contains(&needle))
        .take(SHOWN)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::suggest;

    /// A field holding a listed branch offers the newest others; typing narrows them.
    #[test]
    fn typing_narrows_the_offered_branches() {
        let names: Vec<String> = ["corral/T-14", "main", "release/2026.11"]
            .map(String::from)
            .to_vec();
        assert_eq!(suggest(&names, "main"), ["corral/T-14", "release/2026.11"]);
        assert_eq!(suggest(&names, "REL"), ["release/2026.11"]);
    }
}
