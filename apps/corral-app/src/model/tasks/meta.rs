//! A task's planning metadata: its size (how much work, `xs` to `xl`), its difficulty (how
//! complex or uncertain, `routine` to `hard`) and its tags, each exact as the engine stores it.
//! An unset size or difficulty is said as unset, never guessed; a size outside the scale that
//! an older task carries is shown and kept as it is. Here too the filter that narrows Tasks by
//! words and by these facts.

use std::collections::BTreeSet;

use corral_types::{BUILTIN_TAGS, TaskDifficulty, validate_tag};

use crate::model::daemon::Feed;

/// What an unset size or difficulty says, in a row's cell as in a labelled fact.
pub const UNSET: &str = "unset";
/// What an untagged task says where its tags are a labelled fact.
pub const UNTAGGED: &str = "none";
/// How many tags a row shows before it counts the rest.
pub const ROW_TAGS: usize = 3;

/// A size as a labelled fact: the stored word, or unset.
pub fn size_word(size: Option<&str>) -> String {
    size.map_or_else(|| UNSET.to_owned(), str::to_owned)
}

/// A difficulty as a labelled fact: its word, or unset.
pub fn difficulty_word(difficulty: Option<TaskDifficulty>) -> String {
    difficulty.map_or_else(
        || UNSET.to_owned(),
        |difficulty| difficulty.as_str().to_owned(),
    )
}

/// Tags as a labelled fact: each exact, or none.
pub fn tags_word(tags: &[String]) -> String {
    if tags.is_empty() {
        UNTAGGED.to_owned()
    } else {
        tags.join(", ")
    }
}

/// The tags a row shows, the rest counted: `bug`, `testing`, `+2`.
pub fn row_tags(tags: &[String]) -> Vec<String> {
    let mut shown: Vec<String> = tags.iter().take(ROW_TAGS).cloned().collect();
    if tags.len() > ROW_TAGS {
        shown.push(format!("+{}", tags.len() - ROW_TAGS));
    }
    shown
}

/// A row's metadata line where there are no columns for it (the phone, a build's board):
/// `size m · difficulty unset · bug, testing`, an unset size or difficulty said.
pub fn line(size: Option<&str>, difficulty: Option<TaskDifficulty>, tags: &[String]) -> String {
    let mut parts = vec![
        format!("size {}", size_word(size)),
        format!("difficulty {}", difficulty_word(difficulty)),
    ];
    if !tags.is_empty() {
        parts.push(tags.join(", "));
    }
    parts.join(" · ")
}

/// A proposed task's facts as the owner confirms them: kind, size and difficulty (unset said),
/// tags and profile.
pub fn plan_facts(task: &corral_protocol::detail::BoardTaskPlan) -> String {
    let mut facts: Vec<String> = task.kind.iter().cloned().collect();
    facts.push(format!("size {}", size_word(task.size.as_deref())));
    facts.push(format!("difficulty {}", difficulty_word(task.difficulty)));
    if !task.tags.is_empty() {
        facts.push(format!("tags {}", task.tags.join(", ")));
    }
    facts.extend(task.profile.iter().cloned());
    facts.join(" · ")
}

/// The tags a form offers: the built-in ones, then every other tag a task the client holds
/// carries, in order, each once.
pub fn known_tags(feed: &Feed) -> Vec<String> {
    let custom: BTreeSet<&str> = feed
        .board
        .tasks()
        .iter()
        .flat_map(|task| task.task().tags.iter())
        .map(String::as_str)
        .filter(|tag| !BUILTIN_TAGS.contains(tag))
        .collect();
    BUILTIN_TAGS
        .iter()
        .copied()
        .chain(custom)
        .map(str::to_owned)
        .collect()
}

/// A tag list as one form value: one tag a line, which no tag can hold.
pub fn join_tags(tags: &[String]) -> String {
    tags.join("\n")
}

/// The tags of a form value, in order.
pub fn split_tags(value: &str) -> Vec<String> {
    value
        .split('\n')
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The value with `tag` added at the end, unless it is there already; a label the engine
/// would refuse (surrounding blanks included) is refused here with its reason, never altered.
pub fn add_tag(value: &str, tag: &str) -> Result<String, String> {
    validate_tag(tag)?;
    let mut tags = split_tags(value);
    if !tags.iter().any(|held| held == tag) {
        tags.push(tag.to_owned());
    }
    Ok(join_tags(&tags))
}

/// The value without `tag`.
pub fn remove_tag(value: &str, tag: &str) -> String {
    let tags: Vec<String> = split_tags(value)
        .into_iter()
        .filter(|held| held != tag)
        .collect();
    join_tags(&tags)
}

/// The offered tags the value does not hold yet, narrowed to those containing what is typed.
pub fn offered_tags(known: &[String], value: &str, typed: &str) -> Vec<String> {
    let held = split_tags(value);
    let typed = typed.trim().to_lowercase();
    known
        .iter()
        .filter(|tag| !held.contains(tag) && tag.to_lowercase().contains(&typed))
        .cloned()
        .collect()
}

/// What the Tasks filter asks of a task: every word in its id, title, project or a tag, and
/// every `size:`, `difficulty:` and `tag:` token met exactly. `size:unset` and
/// `difficulty:unset` ask for the unset ones; a tag with blanks is quoted, `tag:"needs design"`,
/// and a backslash keeps the next character as it is (`\"`, `\\`), so every tag can be named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    words: Vec<String>,
    sizes: Vec<Option<String>>,
    difficulties: Vec<Option<String>>,
    tags: Vec<String>,
}

/// The filter token that asks for exactly `tag`.
pub fn tag_filter(tag: &str) -> String {
    format!("tag:\"{}\"", tag.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The filter's tokens: blanks part them except inside double quotes, which are dropped, and a
/// backslash takes the next character as it is.
fn tokens(filter: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut chars = filter.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => token.extend(chars.next()),
            '"' => quoted = !quoted,
            ch if ch.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            ch => token.push(ch),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

impl Filter {
    pub fn parse(filter: &str) -> Self {
        let mut parsed = Self::default();
        let unset = |value: &str| (value != UNSET).then(|| value.to_lowercase());
        for token in tokens(filter) {
            let (key, value) = token.split_once(':').unwrap_or(("", token.as_str()));
            match key.to_lowercase().as_str() {
                "size" if !value.is_empty() => parsed.sizes.push(unset(value)),
                "difficulty" if !value.is_empty() => parsed.difficulties.push(unset(value)),
                "tag" if !value.is_empty() => parsed.tags.push(value.to_owned()),
                _ => parsed.words.push(token.to_lowercase()),
            }
        }
        parsed
    }

    /// Whether the filter asks nothing.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
            && self.sizes.is_empty()
            && self.difficulties.is_empty()
            && self.tags.is_empty()
    }

    pub fn matches(&self, task: &corral_types::Task) -> bool {
        let size = task.size.as_deref().map(str::to_lowercase);
        let difficulty = task
            .difficulty
            .map(|difficulty| difficulty.as_str().to_owned());
        if !self.sizes.iter().all(|wanted| *wanted == size)
            || !self.difficulties.iter().all(|wanted| *wanted == difficulty)
            || !self.tags.iter().all(|wanted| task.tags.contains(wanted))
        {
            return false;
        }
        if self.words.is_empty() {
            return true;
        }
        let text = format!(
            "{} {} {} {}",
            task.id,
            task.title,
            task.project.as_deref().unwrap_or_default(),
            task.tags.join(" ")
        )
        .to_lowercase();
        self.words.iter().all(|word| text.contains(word.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(
        size: Option<&str>,
        difficulty: Option<TaskDifficulty>,
        tags: &[&str],
    ) -> corral_types::Task {
        corral_types::Task {
            id: "T-4".into(),
            title: "Zoom presets".into(),
            size: size.map(str::to_owned),
            difficulty,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            ..corral_types::Task::default()
        }
    }

    /// A tag filter matched by substring found `bug` in `bugfix-later`, and size and difficulty
    /// could not be asked for at all: each token is met exactly, unset included, and words
    /// still match anywhere.
    #[test]
    fn the_filter_meets_size_difficulty_and_tags_exactly() {
        let hard = task(
            Some("m"),
            Some(TaskDifficulty::Hard),
            &["bugfix-later", "needs design"],
        );
        let plain = task(None, None, &["bug"]);
        let shows = |filter: &str, task: &corral_types::Task| Filter::parse(filter).matches(task);
        assert!(shows("tag:bug", &plain) && !shows("tag:bug", &hard));
        assert!(shows("tag:\"needs design\" size:M difficulty:hard", &hard));
        assert!(!shows("tag:Bug", &plain));
        assert!(shows("size:unset difficulty:unset", &plain));
        assert!(!shows("difficulty:unset", &hard) && !shows("size:s", &hard));
        assert!(shows("zoom bugfix", &hard));
        let odd = task(None, None, &[r#"say "hi" \ bye"#]);
        assert!(shows(&tag_filter(r#"say "hi" \ bye"#), &odd));
        assert!(!shows(&tag_filter("say"), &odd));
    }

    /// Adding a tag kept nothing exact: spelling, case and inner blanks survive, a second
    /// identical label is not added, and a label the engine refuses never enters the list.
    #[test]
    fn tags_are_added_and_removed_exactly() {
        let value = add_tag("", "Needs  Design").and_then(|value| add_tag(&value, "bug"));
        assert_eq!(value.as_deref(), Ok("Needs  Design\nbug"));
        assert!(add_tag("", " bug").is_err());
        let value = value.unwrap_or_default();
        assert_eq!(add_tag(&value, "bug").as_deref(), Ok(value.as_str()));
        assert!(add_tag(&value, "a\tb").is_err() && add_tag(&value, "  ").is_err());
        assert_eq!(split_tags(&remove_tag(&value, "Needs  Design")), ["bug"]);
        let known: Vec<String> = ["bug", "testing", "perf-lab"].map(String::from).to_vec();
        assert_eq!(offered_tags(&known, &value, "t"), ["testing"]);
    }
}
