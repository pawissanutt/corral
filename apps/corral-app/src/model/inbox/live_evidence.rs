//! A for a served question: one tab per bounded evidence block corrald resolved (at most five,
//! each at most 8 KiB), its diff, excerpt or log tail, or the reason it could not be resolved.

use corral_protocol::detail::{EvidenceBlock, EvidenceBody as Body, QuestionDetail};

use super::live::Draft;
use super::{Action, Assessment, DiffLine, EvidenceBody, EvidenceTab};
use crate::model::Id;
use crate::model::board::LineKind;
use crate::model::text::{Rich, span};

/// Longest tab label before it is cut.
const TAB_CHARS: usize = 40;

/// A block's name in tabs and proof links.
pub fn label(at: usize, block: &EvidenceBlock) -> String {
    let title = block.title.trim();
    if title.is_empty() {
        return format!("evidence {}", at + 1);
    }
    if title.chars().count() > TAB_CHARS {
        let cut: String = title.chars().take(TAB_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        title.to_owned()
    }
}

/// The evidence band of `key`, with the owner's selected block.
pub fn assessment(key: &Id, draft: Option<&Draft>, detail: &QuestionDetail) -> Assessment {
    if detail.evidence.is_empty() {
        return Assessment::Empty {
            note: "The asker attached no evidence to this question.",
        };
    }
    let selected = draft
        .map(|draft| draft.evidence)
        .unwrap_or(0)
        .min(detail.evidence.len() - 1);
    let tabs = detail
        .evidence
        .iter()
        .enumerate()
        .map(|(at, block)| EvidenceTab {
            label: label(at, block),
            test_id: format!("seg-evidence-{}", at + 1),
            selected: at == selected,
            action: Action::Proof(key.clone(), at),
        })
        .collect();
    Assessment::Ready {
        label: "Evidence",
        tabs,
        body: body(&detail.evidence[selected]),
    }
}

fn truncated(block: &EvidenceBlock) -> Rich {
    vec![
        span("Cut to fit the app's bound; the whole artifact is "),
        span(block.source_ref.clone()).mono(),
        span("."),
    ]
}

fn body(block: &EvidenceBlock) -> EvidenceBody {
    match &block.body {
        Body::Diff {
            path,
            lines,
            truncated: cut,
        } => {
            let (at, lines) = diff(lines);
            EvidenceBody::Diff {
                file: path.clone(),
                at,
                lines,
                note: if *cut {
                    truncated(block)
                } else {
                    vec![span(block.title.clone())]
                },
            }
        }
        Body::Excerpt {
            path,
            start_line,
            lines,
            truncated: cut,
        } => EvidenceBody::Text {
            file: path.clone().unwrap_or_else(|| block.title.clone()),
            attribution: None,
            lines: lines
                .iter()
                .enumerate()
                .map(|(at, line)| {
                    let mut runs = Vec::with_capacity(2);
                    if let Some(start) = start_line {
                        let number = u64::from(*start) + at as u64;
                        runs.push(span(format!("{number:>5}  ")).dim());
                    }
                    runs.push(span(line.clone()).mono());
                    runs
                })
                .collect(),
            note: cut.then(|| truncated(block)),
        },
        Body::LogTail {
            lines,
            truncated: cut,
        } => EvidenceBody::Text {
            file: block.title.clone(),
            attribution: None,
            lines: lines
                .iter()
                .map(|line| vec![span(line.clone()).mono()])
                .collect(),
            note: cut.then(|| truncated(block)),
        },
        Body::Image { artifact } => EvidenceBody::Text {
            file: block.title.clone(),
            attribution: None,
            lines: vec![vec![span(format!(
                "Image: {} ({})",
                artifact.title, artifact.source_ref
            ))]],
            note: None,
        },
        Body::Unresolved { reason } => EvidenceBody::Unresolved {
            file: block.title.clone(),
            reason: reason.clone(),
        },
        Body::Other => EvidenceBody::Unresolved {
            file: block.title.clone(),
            reason: "This kind of evidence is not supported by this app version.".into(),
        },
    }
}

/// Unified diff lines numbered from their hunk headers; the first header heads the box, later
/// ones stay in place unnumbered. Lines before any header carry no numbers.
fn diff(lines: &[String]) -> (String, Vec<DiffLine>) {
    let mut head = String::new();
    let mut numbers: Option<(u32, u32)> = None;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if line.starts_with("@@") {
            numbers = hunk_start(line);
            if head.is_empty() {
                head = line.clone();
                continue;
            }
            out.push(DiffLine {
                old: None,
                new: None,
                kind: LineKind::Context,
                prefix: "",
                text: line.clone(),
            });
            continue;
        }
        if numbers.is_none() && (line.starts_with("+++ ") || line.starts_with("--- ")) {
            continue;
        }
        let (kind, prefix, text) = match line.chars().next() {
            Some('+') => (LineKind::Add, "+", &line[1..]),
            Some('-') => (LineKind::Del, "-", &line[1..]),
            Some(' ') => (LineKind::Context, "", &line[1..]),
            _ => (LineKind::Context, "", line.as_str()),
        };
        let (old, new) = match (&mut numbers, kind) {
            (Some((old, new)), LineKind::Context) if !line.starts_with('\\') => {
                let pair = (Some(*old), Some(*new));
                *old += 1;
                *new += 1;
                pair
            }
            (Some((old, _)), LineKind::Del) => {
                let pair = (Some(*old), None);
                *old += 1;
                pair
            }
            (Some((_, new)), LineKind::Add) => {
                let pair = (None, Some(*new));
                *new += 1;
                pair
            }
            _ => (None, None),
        };
        out.push(DiffLine {
            old,
            new,
            kind,
            prefix,
            text: text.to_owned(),
        });
    }
    (head, out)
}

/// The first old and new line of `@@ -a,b +c,d @@`.
fn hunk_start(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace().skip(1);
    let start = |part: Option<&str>, sign: char| -> Option<u32> {
        part?.strip_prefix(sign)?.split(',').next()?.parse().ok()
    };
    let old = start(parts.next(), '-')?;
    let new = start(parts.next(), '+')?;
    Some((old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn served_diff_numbers_follow_hunk_headers_not_line_positions() {
        let lines: Vec<String> = ["@@ -10,3 +20,3 @@ fn f", " a", "-b", "+c", " d"]
            .into_iter()
            .map(String::from)
            .collect();
        let (head, numbered) = diff(&lines);
        assert_eq!(head, "@@ -10,3 +20,3 @@ fn f");
        let pairs: Vec<_> = numbered.iter().map(|line| (line.old, line.new)).collect();
        assert_eq!(
            pairs,
            [
                (Some(10), Some(20)),
                (Some(11), None),
                (None, Some(21)),
                (Some(12), Some(22)),
            ]
        );
    }
}
