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

/// One input a verdict read. A verdict depends on exactly its reads; where it is reported
/// is never itself a dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// The value at a node and everything below it. Reading an absent entry reads its
    /// absence, which only a change at that entry or an ancestor can affect.
    Value(JsonPath),
    /// Only which entries a collection has, not what they contain.
    Members(JsonPath),
}

impl Read {
    /// Whether replacing or removing the node at `change` can alter what was read.
    pub fn changed_by(&self, change: &JsonPath) -> bool {
        match self {
            Self::Value(node) => node.related(change),
            Self::Members(collection) => {
                collection.0.starts_with(&change.0)
                    || change.0.len() == collection.0.len() + 1
                        && change.0.starts_with(&collection.0)
            }
        }
    }

    pub fn map(&self, f: impl FnOnce(&JsonPath) -> JsonPath) -> Self {
        match self {
            Self::Value(node) => Self::Value(f(node)),
            Self::Members(collection) => Self::Members(f(collection)),
        }
    }
}

/// A rejected value reported at `path`, with the inputs its verdict read, so a caller can
/// discard exactly the verdicts computed from placeholder-repaired input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathProblem {
    pub path: JsonPath,
    pub message: String,
    pub reads: Vec<Read>,
}

impl PathProblem {
    /// Whether the verdict read any of the `changed` nodes.
    pub fn depends_on(&self, changed: &[JsonPath]) -> bool {
        self.reads
            .iter()
            .any(|read| changed.iter().any(|change| read.changed_by(change)))
    }
}

#[derive(Debug, Default)]
pub struct PathProblems(pub Vec<PathProblem>);

impl PathProblems {
    /// A verdict about the whole value at `path` that also read the whole values at `also`.
    pub fn push(&mut self, path: &JsonPath, also: &[&JsonPath], message: impl Into<String>) {
        let reads = std::iter::once(path)
            .chain(also.iter().copied())
            .map(|node| Read::Value(node.clone()))
            .collect();
        self.push_reading(path, reads, message);
    }

    /// A verdict reported at `path` that read exactly `reads`.
    pub fn push_reading(&mut self, path: &JsonPath, reads: Vec<Read>, message: impl Into<String>) {
        self.0.push(PathProblem {
            path: path.clone(),
            message: message.into(),
            reads,
        });
    }

    /// Like [`Self::require`], but `path` only locates the report: the verdict depends on
    /// exactly `reads`.
    pub fn require_reading(
        &mut self,
        ok: bool,
        path: &JsonPath,
        reads: Vec<Read>,
        message: impl Into<String>,
    ) {
        if !ok {
            self.push_reading(path, reads, message);
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
