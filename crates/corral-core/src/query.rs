//! Shared task-query language. Terms are ANDed; a leading `-` excludes a term.
//! Text ranks exact/substring matches above fuzzy word subsequences, with titles
//! above summaries, artifact titles and body text. Dates compare task update time.

use anyhow::{Context, Result, bail};
use jiff::{SignedDuration, Timestamp, civil::Date, tz::TimeZone};

use crate::{Run, Status, Task, TaskCost};

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;

/// Store-owned text and pin state supplementing a task's durable fields.
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchContext<'a> {
    pub pinned: bool,
    pub extra_text: &'a str,
}

/// The classified content selected by a structured text predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentField {
    Test,
    Error,
    Verdict,
}

/// Document-owned text and metadata, independent of the containing task.
#[derive(Debug, Clone, Copy)]
pub struct ContentContext<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub tests: &'a str,
    pub errors: &'a str,
    pub verdicts: &'a str,
    pub kind: &'a str,
    pub updated: Timestamp,
}

/// A parsed query with relative dates resolved once against a supplied clock.
#[derive(Debug, Clone)]
pub struct Query {
    terms: Vec<Term>,
}

#[derive(Debug, Clone)]
struct Term {
    negative: bool,
    predicate: Predicate,
}

#[derive(Debug, Clone)]
enum Predicate {
    Text(String),
    Content(ContentField, String),
    Status(Status),
    Project(String),
    Kind(String),
    Profile(String),
    Model(String),
    Harness(String),
    Id(String),
    Pinned,
    Since(Timestamp),
    Before(Timestamp),
    Cost(f64),
    Rounds(u32),
    Tokens(u64),
    HasArtifact,
    Sha(String),
}

impl Query {
    /// Parse quoted or whitespace-separated terms.
    ///
    /// # Errors
    /// Rejects unknown fields, empty values, invalid numbers/dates and unclosed quotes.
    pub fn parse(input: &str, now: Timestamp) -> Result<Self> {
        let terms = tokenize(input)?
            .into_iter()
            .map(|(token, literal)| {
                let (negative, value) = match token.strip_prefix('-') {
                    Some(value) => (true, value),
                    None => (false, token.as_str()),
                };
                if value.is_empty() {
                    bail!("missing query term after '-'");
                }
                let predicate = if literal {
                    Predicate::Text(value.to_lowercase())
                } else {
                    parse_predicate(value, now)?
                };
                Ok(Term {
                    negative,
                    predicate,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self { terms })
    }

    /// A matching task's relevance score; `None` means at least one term failed.
    /// Empty or field-only queries score zero, leaving tie ordering to the caller.
    pub fn score(&self, task: &Task, context: SearchContext<'_>) -> Option<u64> {
        let mut total = 0_u64;
        for term in &self.terms {
            let score = term.predicate.score(task, context);
            if term.negative {
                if score.is_some() {
                    return None;
                }
            } else {
                total = total.saturating_add(score?);
            }
        }
        Some(total)
    }

    /// Match a document's content, kind and update time with its task's metadata.
    pub fn matches_content(
        &self,
        task: &Task,
        context: SearchContext<'_>,
        content: ContentContext<'_>,
    ) -> bool {
        self.terms.iter().all(|term| {
            let matched = match &term.predicate {
                Predicate::Text(value) => {
                    contains(content.title, value) || contains(content.text, value)
                }
                Predicate::Content(field, value) => contains(
                    match field {
                        ContentField::Test => content.tests,
                        ContentField::Error => content.errors,
                        ContentField::Verdict => content.verdicts,
                    },
                    value,
                ),
                Predicate::Kind(value) => contains(content.kind, value),
                Predicate::Since(cutoff) => content.updated >= *cutoff,
                Predicate::Before(cutoff) => content.updated < *cutoff,
                Predicate::Status(_)
                | Predicate::Project(_)
                | Predicate::Profile(_)
                | Predicate::Model(_)
                | Predicate::Harness(_)
                | Predicate::Id(_)
                | Predicate::Pinned
                | Predicate::Cost(_)
                | Predicate::Rounds(_)
                | Predicate::Tokens(_)
                | Predicate::HasArtifact
                | Predicate::Sha(_) => term.predicate.score(task, context).is_some(),
            };
            matched != term.negative
        })
    }

    /// Borrow normalized text terms as (value, optional field, negative).
    /// Metadata predicates are omitted; query order is preserved.
    pub fn content_terms(&self) -> Vec<(&str, Option<ContentField>, bool)> {
        self.terms
            .iter()
            .filter_map(|term| {
                let (value, field) = match &term.predicate {
                    Predicate::Text(value) => (value, None),
                    Predicate::Content(field, value) => (value, Some(*field)),
                    Predicate::Status(_)
                    | Predicate::Project(_)
                    | Predicate::Kind(_)
                    | Predicate::Profile(_)
                    | Predicate::Model(_)
                    | Predicate::Harness(_)
                    | Predicate::Id(_)
                    | Predicate::Pinned
                    | Predicate::Since(_)
                    | Predicate::Before(_)
                    | Predicate::Cost(_)
                    | Predicate::Rounds(_)
                    | Predicate::Tokens(_)
                    | Predicate::HasArtifact
                    | Predicate::Sha(_) => return None,
                };
                Some((value.as_str(), field, term.negative))
            })
            .collect()
    }
}

fn parse_predicate(token: &str, now: Timestamp) -> Result<Predicate> {
    if token.eq_ignore_ascii_case("pinned") {
        return Ok(Predicate::Pinned);
    }
    if let Some((field, number)) = token.split_once('>') {
        return Ok(match field.to_ascii_lowercase().as_str() {
            "cost" => {
                let number: f64 = number.parse().context("cost> needs a nonnegative number")?;
                if !number.is_finite() || number < 0.0 {
                    bail!("cost> needs a finite nonnegative number");
                }
                Predicate::Cost(number)
            }
            "rounds" => Predicate::Rounds(
                number
                    .parse()
                    .context("rounds> needs a nonnegative integer")?,
            ),
            "tokens" => Predicate::Tokens(
                number
                    .parse()
                    .context("tokens> needs a nonnegative integer")?,
            ),
            _ => bail!("unknown query field {field:?}"),
        });
    }
    let Some((field, value)) = token.split_once(':') else {
        return Ok(Predicate::Text(token.to_lowercase()));
    };
    if value.is_empty() {
        bail!("query field {field:?} needs a value");
    }
    let value_lower = value.to_lowercase();
    Ok(match field.to_ascii_lowercase().as_str() {
        "test" => Predicate::Content(ContentField::Test, value_lower),
        "error" => Predicate::Content(ContentField::Error, value_lower),
        "verdict" => Predicate::Content(ContentField::Verdict, value_lower),
        "status" => Predicate::Status(
            Status::parse(value).with_context(|| format!("unknown status {value:?}"))?,
        ),
        "project" => Predicate::Project(value_lower),
        "kind" => Predicate::Kind(value_lower),
        "profile" => Predicate::Profile(value_lower),
        "model" => Predicate::Model(value_lower),
        "harness" => Predicate::Harness(value_lower),
        "id" => Predicate::Id(value_lower),
        "since" => Predicate::Since(cutoff(value, now)?),
        "before" => Predicate::Before(cutoff(value, now)?),
        "has" if value.eq_ignore_ascii_case("artifact") => Predicate::HasArtifact,
        "has" => bail!("unknown has: value {value:?}; expected has:artifact"),
        "sha" => Predicate::Sha(value_lower),
        _ => bail!("unknown query field {field:?}"),
    })
}

/// Resolve a query date against the supplied clock; date-only values use UTC.
///
/// # Errors
/// Rejects invalid dates, negative durations and out-of-range timestamps.
pub fn cutoff(value: &str, now: Timestamp) -> Result<Timestamp> {
    let err = || format!("invalid query date {value:?}; use Nh, Nd, Nw, YYYY-MM-DD or RFC3339");
    if let Some(unit) = value.chars().last()
        && let Some(multiplier) = match unit {
            'h' => Some(3600_i64),
            'd' => Some(86400_i64),
            'w' => Some(604800_i64),
            _ => None,
        }
    {
        let count: i64 = value[..value.len() - 1].parse().with_context(err)?;
        if count < 0 {
            bail!("{}", err());
        }
        let seconds = count.checked_mul(multiplier).with_context(err)?;
        return now
            .checked_sub(SignedDuration::from_secs(seconds))
            .with_context(err);
    }
    if let Ok(date) = value.parse::<Date>() {
        return Ok(date
            .at(0, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .with_context(err)?
            .timestamp());
    }
    value.parse::<Timestamp>().with_context(err)
}

/// Shell-like quoting, including `project:"two words"`; a fully quoted token
/// is literal text so searches for strings such as `"https://example"` work.
fn tokenize(input: &str) -> Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut literal = false;
    let mut started = false;
    for c in input.chars() {
        if escaped {
            text.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
            started = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                text.push(c);
            }
        } else if (c == '"' || c == '\'') && (text.is_empty() || text.ends_with([':', '-'])) {
            literal = text.is_empty() || text == "-";
            quote = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                if text.is_empty() {
                    bail!("empty quoted query term");
                }
                out.push((std::mem::take(&mut text), literal));
                started = false;
                literal = false;
            }
        } else {
            started = true;
            text.push(c);
        }
    }
    if quote.is_some() || escaped {
        bail!("unclosed quote or escape in query");
    }
    if started {
        if text.is_empty() {
            bail!("empty quoted query term");
        }
        out.push((text, literal));
    }
    Ok(out)
}

fn contains(field: &str, value: &str) -> bool {
    field.to_lowercase().contains(value)
}

impl Predicate {
    fn score(&self, t: &Task, context: SearchContext<'_>) -> Option<u64> {
        let matched = match self {
            Self::Text(value) => return text_score(t, context.extra_text, value),
            Self::Content(field, value) => {
                structured_contains(&t.body, *field, value)
                    || structured_contains(context.extra_text, *field, value)
                    || t.comments
                        .iter()
                        .any(|c| structured_contains(&c.text, *field, value))
                    || t.runs.iter().any(|r| run_content_matches(r, *field, value))
            }
            Self::Status(status) => t.status == *status,
            Self::Project(value) => t.project.as_deref().is_some_and(|s| contains(s, value)),
            Self::Kind(value) => t.kind.as_deref().is_some_and(|s| contains(s, value)),
            Self::Profile(value) => {
                t.profile.as_deref().is_some_and(|s| contains(s, value))
                    || t.runs
                        .iter()
                        .any(|r| contains(r.profile.as_deref().unwrap_or(&r.agent), value))
            }
            Self::Model(value) => t
                .runs
                .iter()
                .any(|r| r.model.as_deref().is_some_and(|s| contains(s, value))),
            Self::Harness(value) => t
                .runs
                .iter()
                .any(|r| r.harness.as_deref().is_some_and(|s| contains(s, value))),
            Self::Id(value) => glob(value, &t.id.to_lowercase()),
            Self::Pinned => context.pinned,
            Self::Since(cutoff) => t.updated >= *cutoff,
            Self::Before(cutoff) => t.updated < *cutoff,
            Self::Cost(number) => TaskCost::of(t).total() > *number,
            Self::Rounds(number) => t.rounds > *number,
            Self::Tokens(number) => t.total_usage().is_some_and(|u| u.total_tokens() > *number),
            Self::HasArtifact => !t.artifacts.is_empty(),
            Self::Sha(value) => contains(context.extra_text, value),
        };
        matched.then_some(0)
    }
}

/// Collect test, error and verdict lines, in that order, preserving original text.
/// Classification uses case-insensitive words, not arbitrary substrings. A line
/// can belong to multiple fields; unrelated lines and section bodies are omitted.
pub fn content_fields(text: &str) -> (String, String, String) {
    let mut tests = String::new();
    let mut errors = String::new();
    let mut verdicts = String::new();
    for line in text.lines() {
        for (field, output) in [
            (ContentField::Test, &mut tests),
            (ContentField::Error, &mut errors),
            (ContentField::Verdict, &mut verdicts),
        ] {
            if classified_line(line, field) {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(line);
            }
        }
    }
    (tests, errors, verdicts)
}

fn classified_line(line: &str, field: ContentField) -> bool {
    line.split(|c: char| !c.is_alphanumeric()).any(|word| {
        let word = word.to_ascii_lowercase();
        match field {
            ContentField::Test => {
                matches!(
                    word.as_str(),
                    "test" | "tests" | "pytest" | "nextest" | "vitest"
                )
            }
            ContentField::Error => matches!(
                word.as_str(),
                "error" | "errors" | "panic" | "panicked" | "failed" | "failure" | "failures"
            ),
            ContentField::Verdict => matches!(word.as_str(), "verdict" | "verdicts"),
        }
    })
}

fn structured_contains(text: &str, field: ContentField, value: &str) -> bool {
    text.lines()
        .any(|line| classified_line(line, field) && contains(line, value))
}

fn run_content_matches(run: &Run, field: ContentField, value: &str) -> bool {
    run.summary
        .as_deref()
        .is_some_and(|s| structured_contains(s, field, value))
        || run
            .review_runs
            .iter()
            .any(|r| run_content_matches(r, field, value))
}

fn text_score(t: &Task, extra: &str, value: &str) -> Option<u64> {
    let mut scores = vec![
        field_score(&t.id, value, 1200),
        field_score(&t.title, value, 1000),
        field_score(&t.body, value, 300),
        field_score(t.status.as_str(), value, 400),
        field_score(extra, value, 400),
    ];
    for s in [
        t.project.as_deref(),
        t.kind.as_deref(),
        t.profile.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        scores.push(field_score(s, value, 400));
    }
    for run in &t.runs {
        scores.push(run_score(run, value));
    }
    for artifact in &t.artifacts {
        scores.push(field_score(&artifact.title, value, 600));
    }
    for comment in &t.comments {
        scores.push(field_score(&comment.text, value, 300));
    }
    scores.into_iter().flatten().max()
}

fn run_score(run: &Run, value: &str) -> Option<u64> {
    [
        run.summary
            .as_deref()
            .and_then(|s| field_score(s, value, 700)),
        field_score(&run.agent, value, 400),
        run.profile
            .as_deref()
            .and_then(|s| field_score(s, value, 400)),
        run.model
            .as_deref()
            .and_then(|s| field_score(s, value, 400)),
        run.harness
            .as_deref()
            .and_then(|s| field_score(s, value, 400)),
        run.review_runs
            .iter()
            .filter_map(|r| run_score(r, value))
            .max(),
    ]
    .into_iter()
    .flatten()
    .max()
}

fn field_score(field: &str, value: &str, weight: u64) -> Option<u64> {
    let field = field.to_lowercase();
    if field == value {
        return Some(weight + 50);
    }
    if field.contains(value) {
        return Some(weight + 20);
    }
    // Fuzzy matching stays inside one word: a long body cannot match arbitrary
    // letters scattered across unrelated paragraphs.
    field.split_whitespace().find_map(|word| {
        let mut rest = word.chars();
        value
            .chars()
            .all(|c| rest.by_ref().any(|found| c == found))
            .then_some(weight / 4)
    })
}

fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<_> = pattern.split('*').collect();
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(tail) = rest.strip_prefix(part) else {
                return false;
            };
            rest = tail;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some((_, tail)) = rest.split_once(part) {
            rest = tail;
        } else {
            return false;
        }
    }
    rest.is_empty()
}
