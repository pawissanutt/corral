//! JSONPath-style locations for problems found in structured setup input.
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathSeg {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct JsonPath(pub Vec<PathSeg>);

impl JsonPath {
    pub fn key(&self, key: &str) -> Self {
        let mut path = self.clone();
        path.0.push(PathSeg::Key(key.to_owned()));
        path
    }

    pub fn index(&self, index: usize) -> Self {
        let mut path = self.clone();
        path.0.push(PathSeg::Index(index));
        path
    }

    /// Whether either path contains the other.
    pub fn related(&self, other: &Self) -> bool {
        self.0.starts_with(&other.0) || other.0.starts_with(&self.0)
    }
}

impl fmt::Display for JsonPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("$")?;
        for seg in &self.0 {
            match seg {
                PathSeg::Key(key)
                    if !key.is_empty()
                        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
                {
                    write!(f, ".{key}")?;
                }
                PathSeg::Key(key) => write!(f, "[{key:?}]")?,
                PathSeg::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}

/// A rejected value at `path`. `subjects` names the nodes the verdict read (with `path`
/// itself unless `display_only`), and `members` names collections whose membership it
/// read, so a caller can discard verdicts computed from placeholder-repaired input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathProblem {
    pub path: JsonPath,
    pub message: String,
    pub subjects: Vec<JsonPath>,
    pub members: Vec<JsonPath>,
    pub display_only: bool,
}

impl PathProblem {
    /// Whether the verdict depended on any of the `changed` nodes.
    pub fn depends_on(&self, changed: &[JsonPath]) -> bool {
        let own = (!self.display_only).then_some(&self.path);
        let nodes = own.into_iter().chain(&self.subjects);
        let node_changed = nodes
            .flat_map(|node| changed.iter().map(move |change| node.related(change)))
            .any(|related| related);
        node_changed
            || self.members.iter().any(|collection| {
                changed.iter().any(|change| {
                    collection.0.starts_with(&change.0)
                        || change.0.len() == collection.0.len() + 1
                            && change.0.starts_with(&collection.0)
                })
            })
    }
}

#[derive(Debug, Default)]
pub struct PathProblems(pub Vec<PathProblem>);

impl PathProblems {
    pub fn push(&mut self, path: &JsonPath, subjects: &[&JsonPath], message: impl Into<String>) {
        self.0.push(PathProblem {
            path: path.clone(),
            message: message.into(),
            subjects: subjects.iter().map(|subject| (*subject).clone()).collect(),
            members: Vec::new(),
            display_only: false,
        });
    }

    /// Like [`Self::require`], but `path` only locates the report: the verdict depends on
    /// exactly `subjects` and the membership of `members`.
    pub fn require_reading(
        &mut self,
        ok: bool,
        path: &JsonPath,
        subjects: Vec<JsonPath>,
        members: Vec<JsonPath>,
        message: impl Into<String>,
    ) {
        if !ok {
            self.0.push(PathProblem {
                path: path.clone(),
                message: message.into(),
                subjects,
                members,
                display_only: true,
            });
        }
    }

    pub fn require(
        &mut self,
        ok: bool,
        path: &JsonPath,
        subjects: &[&JsonPath],
        message: impl Into<String>,
    ) {
        if !ok {
            self.push(path, subjects, message);
        }
    }

    pub fn error(&mut self, path: &JsonPath, subjects: &[&JsonPath], error: &anyhow::Error) {
        self.push(path, subjects, format!("{error:#}"));
    }

    /// Fail-fast view for callers that only need the first rejection.
    pub fn first(self) -> anyhow::Result<()> {
        match self.0.into_iter().next() {
            Some(problem) => Err(anyhow::anyhow!(problem.message)),
            None => Ok(()),
        }
    }
}
