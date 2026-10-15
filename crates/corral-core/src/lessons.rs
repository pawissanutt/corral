//! Editable project lessons, version 1.
//!
//! `.corral/lessons/<project>.md` has YAML frontmatter containing `version: 1`
//! and `lessons` metadata. Rule and explanation text live only in the markdown
//! body, between `<!-- lesson ID rule -->`, `<!-- lesson ID why -->` and
//! `<!-- lesson ID end -->` lines, beneath `## ID`. Edit text between those
//! markers freely; whitespace (including trailing newlines) is preserved.
//! The scaffold and marker lines are structural and must not occur in text.
//! `<project>.index.json` is a disposable versioned ID/scope index, never read
//! as authority. Save or [`Store::reindex_lessons`] rebuilds it from markdown.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::store::atomic_write;
use crate::{Store, Task, validate_glob};

/// A reusable project rule. Confidence is stored unchanged; matching decays its rank.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rule: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    #[serde(default)]
    pub evidence: Vec<LessonEvidence>,
    #[serde(default)]
    pub scope: LessonScope,
    pub confidence: f64,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub contradicted: bool,
    pub last_seen: Timestamp,
}

/// One observation supporting a lesson.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LessonEvidence {
    pub task: String,
    pub ts: Timestamp,
    pub source: String,
}

/// Dimensions are ANDed, alternatives within a dimension are ORed.
/// Empty dimensions impose no restriction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LessonScope {
    pub files: Vec<String>,
    pub kinds: Vec<String>,
    pub profiles: Vec<String>,
    pub harnesses: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LessonsFile {
    version: u32,
    lessons: Vec<Lesson>,
}

fn key(key: &str) -> Result<()> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c))
        || key == "."
        || key == ".."
        || key.ends_with('.')
    {
        bail!("invalid lessons project key or lesson id {key:?}");
    }
    Ok(())
}

/// Reject symlinks and special files before reading, or following a directory.
fn regular(path: &Path, directory: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta)
            if if directory {
                meta.is_dir()
            } else {
                meta.is_file()
            } =>
        {
            Ok(())
        }
        Ok(_) => bail!(
            "{} must be a regular {}",
            path.display(),
            if directory { "directory" } else { "file" }
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("inspecting {}", path.display())),
    }
}

fn validate(lessons: &[Lesson]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for lesson in lessons {
        key(&lesson.id)?;
        if !ids.insert(&lesson.id) {
            bail!("duplicate lesson id {:?}", lesson.id);
        }
        if !lesson.confidence.is_finite() || !(0.0..=1.0).contains(&lesson.confidence) {
            bail!(
                "lesson {} confidence must be finite and within [0,1]",
                lesson.id
            );
        }
        for glob in &lesson.scope.files {
            validate_glob(glob).with_context(|| format!("lesson {} files", lesson.id))?;
        }
        for text in [&lesson.rule, &lesson.why] {
            if text.lines().any(|line| line.starts_with("<!-- lesson ")) {
                bail!("lesson {} text contains a reserved marker line", lesson.id);
            }
        }
    }
    Ok(())
}

fn render(lessons: &[Lesson]) -> Result<String> {
    let mut metadata = lessons.to_vec();
    for lesson in &mut metadata {
        lesson.rule.clear();
        lesson.why.clear();
    }
    let yaml = serde_yaml_ng::to_string(&LessonsFile {
        version: 1,
        lessons: metadata,
    })
    .context("serializing lessons metadata")?;
    let mut text = format!("---\n{yaml}---\n");
    for lesson in lessons {
        text.push_str(&format!(
            "\n## {id}\n\n<!-- lesson {id} rule -->\n{rule}\n<!-- lesson {id} why -->\n{why}\n<!-- lesson {id} end -->\n",
            id = lesson.id, rule = lesson.rule, why = lesson.why
        ));
    }
    Ok(text)
}

fn parse(text: &str) -> Result<Vec<Lesson>> {
    let rest = text
        .strip_prefix("---\n")
        .context("lessons must start with YAML frontmatter")?;
    let (yaml, mut body) = rest
        .split_once("\n---\n")
        .context("missing lessons frontmatter terminator")?;
    let mut file: LessonsFile =
        serde_yaml_ng::from_str(yaml).context("parsing lessons metadata")?;
    if file.version != 1 {
        bail!("unsupported lessons version {}", file.version);
    }
    for lesson in &mut file.lessons {
        key(&lesson.id)?;
        if !lesson.rule.is_empty() || !lesson.why.is_empty() {
            bail!(
                "lesson {} rule and why must be edited in the markdown body, not frontmatter",
                lesson.id
            );
        }
        let prefix = format!("\n## {id}\n\n<!-- lesson {id} rule -->\n", id = lesson.id);
        body = body
            .strip_prefix(&prefix)
            .with_context(|| format!("missing rule section for lesson {}", lesson.id))?;
        let why_marker = format!("\n<!-- lesson {} why -->\n", lesson.id);
        let (rule, rest) = body
            .split_once(&why_marker)
            .with_context(|| format!("missing why marker for lesson {}", lesson.id))?;
        let end_marker = format!("\n<!-- lesson {} end -->\n", lesson.id);
        let (why, rest) = rest
            .split_once(&end_marker)
            .with_context(|| format!("missing end marker for lesson {}", lesson.id))?;
        lesson.rule = rule.to_string();
        lesson.why = why.to_string();
        body = rest;
    }
    if !body.trim().is_empty() {
        bail!("unexpected text outside lessons sections");
    }
    validate(&file.lessons)?;
    Ok(file.lessons)
}

impl Store {
    fn lessons_path(&self, project: &str) -> Result<PathBuf> {
        key(project)?;
        let dir = self.dir().join("lessons");
        regular(&dir, true)?;
        let path = dir.join(format!("{project}.md"));
        regular(&path, false)?;
        Ok(path)
    }

    /// Reads authoritative markdown, including owner edits. Missing files mean no lessons.
    /// Errors on unsafe paths, malformed documents, invalid confidence, duplicate IDs or globs.
    pub fn lessons(&self, project: &str) -> Result<Vec<Lesson>> {
        let path = self.lessons_path(project)?;
        match fs::read_to_string(&path) {
            Ok(text) => parse(&text).with_context(|| format!("reading {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Replaces a project's lessons under the shared lessons lock, and records `lessons_saved`.
    /// Errors on invalid data, runtime skew, or filesystem/event write failures.
    pub fn save_lessons(&self, project: &str, lessons: &[Lesson]) -> Result<()> {
        let _lock = self.lock("lessons")?;
        self.write_lessons(project, lessons, "lessons_saved")
    }

    /// Saves only if authoritative markdown still equals the request snapshot.
    /// Errors on a stale snapshot (including owner text edits, pinning or deletion).
    pub fn save_lessons_if(
        &self,
        project: &str,
        expected: &[Lesson],
        lessons: &[Lesson],
    ) -> Result<()> {
        let _lock = self.lock("lessons")?;
        self.check_runtime()?;
        if self.lessons(project)? != expected {
            bail!("lessons for {project:?} changed since the snapshot; reload before saving");
        }
        self.write_lessons(project, lessons, "lessons_saved")
    }

    /// Changes a lesson's pin without losing concurrent corral mutations.
    /// Errors if the lesson does not exist, or reading/writing fails.
    pub fn pin_lesson(&self, project: &str, id: &str, pinned: bool) -> Result<()> {
        let _lock = self.lock("lessons")?;
        let mut lessons = self.lessons(project)?;
        let lesson = lessons
            .iter_mut()
            .find(|l| l.id == id)
            .with_context(|| format!("lesson {id:?} not found in {project:?}"))?;
        lesson.pinned = pinned;
        self.write_lessons(project, &lessons, "lesson_pinned")
    }

    /// Removes one lesson. Errors if it does not exist or storage fails.
    pub fn delete_lesson(&self, project: &str, id: &str) -> Result<()> {
        let _lock = self.lock("lessons")?;
        let mut lessons = self.lessons(project)?;
        let index = lessons
            .iter()
            .position(|l| l.id == id)
            .with_context(|| format!("lesson {id:?} not found in {project:?}"))?;
        lessons.remove(index);
        self.write_lessons(project, &lessons, "lesson_deleted")
    }

    /// Rebuilds the disposable index from current markdown without rewriting owner text.
    /// Errors on invalid markdown, runtime skew or filesystem failure.
    pub fn reindex_lessons(&self, project: &str) -> Result<()> {
        let _lock = self.lock("lessons")?;
        self.check_runtime()?;
        let lessons = self.lessons(project)?;
        self.write_lessons_index(project, &lessons)
    }

    fn write_lessons(&self, project: &str, lessons: &[Lesson], event: &str) -> Result<()> {
        self.check_runtime()?;
        let path = self.lessons_path(project)?;
        validate(lessons)?;
        let text = render(lessons)?;
        fs::create_dir_all(self.dir().join("lessons")).context("creating lessons directory")?;
        regular(&path.with_extension("md.tmp"), false)?;
        atomic_write(&path, &text)?;
        self.write_lessons_index(project, lessons)?;
        self.log_event(
            None,
            event,
            serde_json::json!({
                "project": project, "ids": lessons.iter().map(|l| &l.id).collect::<Vec<_>>()
            }),
        )
    }

    fn write_lessons_index(&self, project: &str, lessons: &[Lesson]) -> Result<()> {
        let path = self.lessons_path(project)?.with_extension("index.json");
        fs::create_dir_all(self.dir().join("lessons")).context("creating lessons directory")?;
        regular(&path, false)?;
        regular(&path.with_extension("json.tmp"), false)?;
        let scopes: std::collections::BTreeMap<_, _> =
            lessons.iter().map(|l| (&l.id, &l.scope)).collect();
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "version": 1, "scopes": scopes
        }))
        .context("serializing lessons index")?;
        atomic_write(&path, &text)
    }
}

/// Returns matching lessons, pinned first, then age-decayed confidence, then ID.
///
/// Unpinned ranking confidence has a 90-day half-life; future timestamps have zero
/// age. Stored/returned confidence is unchanged. Pins are immune to decay but not
/// scope restrictions. Contradicted lessons are always excluded, even when pinned.
/// Tags are case-insensitive whole words/phrases in title/body. File candidates
/// include likely files, punctuation-delimited text references and write scopes.
#[must_use]
pub fn match_lessons(
    lessons: &[Lesson],
    task: &Task,
    profile: Option<&str>,
    harness: Option<&str>,
    likely_files: &[String],
    limit: usize,
    now: Timestamp,
) -> Vec<Lesson> {
    let text = format!("{}\n{}", task.title, task.body);
    let paths: Vec<_> = text
        .split(|c: char| c.is_whitespace() || "`'\"(),;:<>".contains(c))
        .filter(|p| !p.is_empty())
        .map(|p| p.trim_end_matches('.'))
        .collect();
    let words = text.to_lowercase();
    let matches_dimension = |values: &[String], value: Option<&str>| {
        values.is_empty() || value.is_some_and(|v| values.iter().any(|s| s == v))
    };
    let mut matched: Vec<_> = lessons
        .iter()
        .filter(|lesson| {
            let scope = &lesson.scope;
            !lesson.contradicted
                && lesson.confidence.is_finite()
                && matches_dimension(&scope.kinds, task.kind.as_deref())
                && matches_dimension(&scope.profiles, profile)
                && matches_dimension(&scope.harnesses, harness)
                && (scope.tags.is_empty() || scope.tags.iter().any(|tag| contains_tag(&words, tag)))
                && (scope.files.is_empty()
                    || scope.files.iter().any(|glob| {
                        validate_glob(glob).is_ok()
                            && (likely_files
                                .iter()
                                .map(String::as_str)
                                .chain(paths.iter().copied())
                                .any(|path| glob_matches(glob, path))
                                || task.write_scope.iter().any(|other| {
                                    validate_glob(other).is_ok() && globs_overlap(glob, other)
                                }))
                    }))
        })
        .collect();
    let score = |lesson: &Lesson| {
        let age = now.duration_since(lesson.last_seen).as_secs_f64().max(0.0);
        lesson.confidence
            * if lesson.pinned {
                1.0
            } else {
                2.0_f64.powf(-age / (90.0 * 86400.0))
            }
    };
    matched.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| score(b).total_cmp(&score(a)))
            .then_with(|| a.id.cmp(&b.id))
    });
    matched.into_iter().take(limit).cloned().collect()
}

fn contains_tag(text: &str, tag: &str) -> bool {
    let tag = tag.to_lowercase();
    !tag.is_empty()
        && text.match_indices(&tag).any(|(start, _)| {
            let end = start + tag.len();
            text[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                && text[end..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric() && c != '_')
        })
}

fn glob_matches(glob: &str, path: &str) -> bool {
    let glob: Vec<_> = glob.split('/').collect();
    let path: Vec<_> = path.strip_prefix("./").unwrap_or(path).split('/').collect();
    let mut pending = vec![(0, 0)];
    let mut seen = HashSet::new();
    while let Some((g, p)) = pending.pop() {
        if !seen.insert((g, p)) {
            continue;
        }
        match (glob.get(g), path.get(p)) {
            (None, None) => return true,
            (Some(&"**"), next) => {
                pending.push((g + 1, p));
                if next.is_some() {
                    pending.push((g, p + 1));
                }
            }
            (Some(pattern), Some(segment)) if segment_matches(pattern, segment) => {
                pending.push((g + 1, p + 1))
            }
            _ => {}
        }
    }
    false
}

fn segment_matches(pattern: &str, segment: &str) -> bool {
    let mut pattern = pattern.split('*');
    let first = pattern.next().unwrap_or("");
    let Some(mut rest) = segment.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<_> = pattern.collect();
    if parts.is_empty() {
        return rest.is_empty();
    }
    for (index, part) in parts.iter().enumerate() {
        if index + 1 == parts.len() {
            return rest.ends_with(part);
        }
        let Some(pos) = rest.find(part) else {
            return false;
        };
        rest = &rest[pos + part.len()..];
    }
    false
}

// Product of the two glob automata: ** consumes any path segment, * any
// character within a segment. Visiting each state once keeps overlap bounded.
fn globs_overlap(a: &str, b: &str) -> bool {
    let a: Vec<_> = a.split('/').collect();
    let b: Vec<_> = b.split('/').collect();
    let mut pending = vec![(0, 0)];
    let mut seen = HashSet::new();
    while let Some((i, j)) = pending.pop() {
        if !seen.insert((i, j)) {
            continue;
        }
        let (left, right) = (a.get(i), b.get(j));
        if left.is_none() && right.is_none() {
            return true;
        }
        if left == Some(&"**") {
            pending.push((i + 1, j));
        }
        if right == Some(&"**") {
            pending.push((i, j + 1));
        }
        if let (Some(left), Some(right)) = (left, right)
            && (*left == "**" || *right == "**" || segments_overlap(left, right))
        {
            pending.push((
                i + usize::from(*left != "**"),
                j + usize::from(*right != "**"),
            ));
        }
    }
    false
}

fn segments_overlap(a: &str, b: &str) -> bool {
    let a: Vec<_> = a.chars().collect();
    let b: Vec<_> = b.chars().collect();
    let mut pending = vec![(0, 0)];
    let mut seen = HashSet::new();
    while let Some((i, j)) = pending.pop() {
        if !seen.insert((i, j)) {
            continue;
        }
        let (left, right) = (a.get(i), b.get(j));
        if left.is_none() && right.is_none() {
            return true;
        }
        if left == Some(&'*') {
            pending.push((i + 1, j));
        }
        if right == Some(&'*') {
            pending.push((i, j + 1));
        }
        if let (Some(left), Some(right)) = (left, right)
            && (*left == '*' || *right == '*' || left == right)
        {
            pending.push((
                i + usize::from(*left != '*'),
                j + usize::from(*right != '*'),
            ));
        }
    }
    false
}
