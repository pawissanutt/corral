//! Numbering and pairing mirror the Review mockup, including its run-end comment anchor.

use crate::model::board::{LineKind, hunk};

use super::data::{Comment, File, HunkSource, SourceLine};

/// A half-open UTF-8 byte range in the original, unescaped code text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Highlight {
    /// First byte included in the word highlight.
    pub start: usize,
    /// First byte after the word highlight.
    pub end: usize,
}

/// One code line with both unified gutter numbers and a word-level highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumberedLine {
    /// Context, deletion or addition, also determining the line's state colour.
    pub kind: LineKind,
    /// Old gutter number; absent on additions.
    pub old: Option<u32>,
    /// New gutter number; absent on deletions.
    pub new: Option<u32>,
    /// Empty for context, `-` for a deletion and `+` for an addition.
    pub sign: &'static str,
    /// Code without a sign or gutter.
    pub text: &'static str,
    /// First matching word in raw code; never HTML-escaped offsets.
    pub highlight: Option<Highlight>,
}

/// One unified code row and the discussion placed directly after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnifiedRow {
    /// Numbered code line.
    pub line: NumberedLine,
    /// Original zero-based hunk line index.
    pub source_index: usize,
    /// Comment inserted after this row, when attached to its source index.
    pub comment: Option<Comment>,
}

/// One side-by-side row. A missing side is an empty cell, not a context line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitRow {
    /// Old side: use its old gutter number.
    pub left: Option<NumberedLine>,
    /// New side: use its new gutter number.
    pub right: Option<NumberedLine>,
    /// Context index or the final source index of the entire deletion/addition run.
    pub source_index: usize,
    /// Mockup comment placement: run-end anchor plus a nonempty added right side.
    pub comment: Option<Comment>,
}

/// Finds only the first highlighted word, matching `wordMark` before HTML rendering.
pub fn word_mark(text: &str, word: Option<&str>) -> Option<Highlight> {
    let word = word.filter(|word| !word.is_empty())?;
    text.find(word).map(|start| Highlight {
        start,
        end: start + word.len(),
    })
}

fn numbered(
    kind: LineKind,
    text: &'static str,
    word: Option<&str>,
    old: &mut u32,
    new: &mut u32,
) -> NumberedLine {
    let old_number = if kind == LineKind::Add {
        None
    } else {
        let number = *old;
        *old += 1;
        Some(number)
    };
    let new_number = if kind == LineKind::Del {
        None
    } else {
        let number = *new;
        *new += 1;
        Some(number)
    };
    NumberedLine {
        kind,
        old: old_number,
        new: new_number,
        sign: match kind {
            LineKind::Context => "",
            LineKind::Del => "-",
            LineKind::Add => "+",
        },
        text,
        highlight: word_mark(text, word),
    }
}

/// Numbers source lines: context advances both counters, deletion only old, addition only new.
pub fn number_lines(lines: &[SourceLine], start: (u32, u32)) -> Vec<NumberedLine> {
    let (mut old, mut new) = start;
    lines
        .iter()
        .map(|line| numbered(line.kind, line.text, line.word, &mut old, &mut new))
        .collect()
}

/// Returns the file's hunk heading; collapsed files have no supplied heading.
pub fn hunk_header(file: &File) -> Option<&'static str> {
    match file.hunk {
        HunkSource::Inline { header, .. } => Some(header),
        HunkSource::Board => Some(hunk().at),
        HunkSource::Collapsed => None,
    }
}

/// Ports `hunkLines`: the real board patch uses Review's historical (1990, 1990) gutters.
/// Its header still reads `-1959`; this intentionally preserves the supplied mockup.
pub fn hunk_lines(file: &File) -> Vec<NumberedLine> {
    match file.hunk {
        HunkSource::Inline { start, lines, .. } => number_lines(lines, start),
        HunkSource::Board => {
            let (mut old, mut new) = (1990, 1990);
            hunk()
                .lines
                .iter()
                .map(|&(kind, text)| numbered(kind, text, None, &mut old, &mut new))
                .collect()
        }
        HunkSource::Collapsed => Vec::new(),
    }
}

/// Unified rows in source order; comments immediately follow their exact source line.
pub fn unified(file: &File) -> Vec<UnifiedRow> {
    hunk_lines(file)
        .into_iter()
        .enumerate()
        .map(|(source_index, line)| UnifiedRow {
            line,
            source_index,
            comment: file.comment.filter(|comment| comment.after == source_index),
        })
        .collect()
}

/// Pairs each deletion run with the additions immediately following it, row by row.
/// Context occupies both sides; unmatched deletions/additions leave the other side empty.
/// All rows of a change run share its final source index, as in the original `split`.
pub fn split_rows(lines: &[NumberedLine], comment: Option<Comment>) -> Vec<SplitRow> {
    let mut rows = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind == LineKind::Context {
            rows.push(SplitRow {
                left: Some(lines[i]),
                right: Some(lines[i]),
                source_index: i,
                comment: None,
            });
            i += 1;
            continue;
        }
        let del_start = i;
        while i < lines.len() && lines[i].kind == LineKind::Del {
            i += 1;
        }
        let add_start = i;
        while i < lines.len() && lines[i].kind == LineKind::Add {
            i += 1;
        }
        let deletions = &lines[del_start..add_start];
        let additions = &lines[add_start..i];
        let source_index = i - 1;
        for j in 0..deletions.len().max(additions.len()) {
            let right = additions.get(j).copied();
            rows.push(SplitRow {
                left: deletions.get(j).copied(),
                right,
                source_index,
                comment: comment.filter(|comment| {
                    comment.after == source_index
                        && right.is_some_and(|line| line.kind == LineKind::Add)
                }),
            });
        }
    }
    rows
}

/// Side-by-side file rows with the mockup's exact run-end inline-comment behavior.
pub fn split(file: &File) -> Vec<SplitRow> {
    split_rows(&hunk_lines(file), file.comment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(kind: LineKind) -> SourceLine {
        SourceLine {
            kind,
            text: "",
            word: None,
        }
    }

    #[test]
    fn split_pairs_following_additions_without_dropping_unmatched_lines() {
        for (deleted, added) in [(3, 1), (1, 3), (2, 2), (0, 2), (2, 0)] {
            let mut source_lines = vec![source(LineKind::Context)];
            source_lines.extend((0..deleted).map(|_| source(LineKind::Del)));
            source_lines.extend((0..added).map(|_| source(LineKind::Add)));
            source_lines.push(source(LineKind::Context));
            let lines = number_lines(&source_lines, (10, 20));
            let rows = split_rows(&lines, None);
            assert_eq!(rows.len(), deleted.max(added) + 2);
            assert_eq!(rows[0].left, rows[0].right);
            for j in 0..deleted.max(added) {
                let row = rows[j + 1];
                assert_eq!(row.left, (j < deleted).then(|| lines[1 + j]));
                assert_eq!(row.right, (j < added).then(|| lines[1 + deleted + j]));
            }
            assert_eq!(
                rows.last().map(|row| row.left),
                rows.last().map(|row| row.right)
            );
        }
    }

    #[test]
    fn unified_numbering_does_not_advance_the_absent_side() {
        let lines = [
            source(LineKind::Context),
            source(LineKind::Del),
            source(LineKind::Del),
            source(LineKind::Add),
            source(LineKind::Context),
            source(LineKind::Add),
        ];
        let actual = number_lines(&lines, (10, 20));
        let numbers: Vec<_> = actual.iter().map(|line| (line.old, line.new)).collect();
        assert_eq!(
            numbers,
            vec![
                (Some(10), Some(20)),
                (Some(11), None),
                (Some(12), None),
                (None, Some(21)),
                (Some(13), Some(22)),
                (None, Some(23))
            ]
        );
    }

    #[test]
    fn split_comment_uses_run_end_and_requires_an_added_right_cell() {
        let lines = number_lines(
            &[
                source(LineKind::Del),
                source(LineKind::Del),
                source(LineKind::Del),
                source(LineKind::Add),
                source(LineKind::Add),
            ],
            (1, 1),
        );
        let comment = Comment {
            after: 4,
            who: "",
            at: "",
            sha: "",
            text: "",
        };
        let rows = split_rows(&lines, Some(comment));
        assert_eq!(
            rows.iter()
                .map(|row| row.comment.is_some())
                .collect::<Vec<_>>(),
            vec![true, true, false]
        );
        assert!(
            split_rows(
                &lines,
                Some(Comment {
                    after: 1,
                    ..comment
                })
            )
            .iter()
            .all(|row| row.comment.is_none())
        );
    }
}
