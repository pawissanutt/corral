//! Diff numbering, side-by-side pairing, discussion placement and served-patch parsing.

use crate::model::board::LineKind;

/// A half-open UTF-8 byte range in the original, unescaped code text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Highlight {
    /// First byte included in the word highlight.
    pub start: usize,
    /// First byte after the word highlight.
    pub end: usize,
}

/// One code line with both unified gutter numbers and a word-level highlight.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub text: String,
    /// First matching word in raw code; never HTML-escaped offsets.
    pub highlight: Option<Highlight>,
}

/// A discussion placed directly after the row holding the zero-based source line `after`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// Source line index within the hunk.
    pub after: usize,
    /// Index into the file's discussions.
    pub discussion: usize,
}

/// One unified code row and the discussions placed directly after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedRow {
    /// Numbered code line.
    pub line: NumberedLine,
    /// Original zero-based hunk line index.
    pub source_index: usize,
    /// Indexes of the file's discussions inserted after this row.
    pub discussions: Vec<usize>,
}

/// One side-by-side row. A missing side is an empty cell, not a context line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitRow {
    /// Old side: use its old gutter number.
    pub left: Option<NumberedLine>,
    /// New side: use its new gutter number.
    pub right: Option<NumberedLine>,
    /// Context index or the final source index of the entire deletion/addition run.
    pub source_index: usize,
    /// Indexes of the file's discussions inserted after this row.
    pub discussions: Vec<usize>,
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
    text: &str,
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
        text: text.to_owned(),
        highlight: word_mark(text, word),
    }
}

fn placed(anchors: &[Anchor], index: usize) -> Vec<usize> {
    anchors
        .iter()
        .filter(|anchor| anchor.after == index)
        .map(|anchor| anchor.discussion)
        .collect()
}

/// Unified rows in source order; discussions immediately follow their exact source line.
pub fn unified_rows(lines: Vec<NumberedLine>, anchors: &[Anchor]) -> Vec<UnifiedRow> {
    lines
        .into_iter()
        .enumerate()
        .map(|(source_index, line)| UnifiedRow {
            line,
            source_index,
            discussions: placed(anchors, source_index),
        })
        .collect()
}

/// Pairs each deletion run with the additions immediately following it, row by row.
/// Context occupies both sides; unmatched deletions/additions leave the other side empty.
/// All rows of a change run share its final source index. A discussion follows the row that
/// shows its anchored line, on whichever side that line sits.
pub fn split_rows(lines: &[NumberedLine], anchors: &[Anchor]) -> Vec<SplitRow> {
    let mut rows = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind == LineKind::Context {
            rows.push(SplitRow {
                left: Some(lines[i].clone()),
                right: Some(lines[i].clone()),
                source_index: i,
                discussions: placed(anchors, i),
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
        let deleted = add_start - del_start;
        let added = i - add_start;
        let source_index = i - 1;
        for j in 0..deleted.max(added) {
            let mut discussions = Vec::new();
            if j < deleted {
                discussions.extend(placed(anchors, del_start + j));
            }
            if j < added {
                discussions.extend(placed(anchors, add_start + j));
            }
            rows.push(SplitRow {
                left: (j < deleted).then(|| lines[del_start + j].clone()),
                right: (j < added).then(|| lines[add_start + j].clone()),
                source_index,
                discussions,
            });
        }
    }
    rows
}

/// One served hunk: its verbatim header and numbered lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHunk {
    /// The `@@ … @@` line as served.
    pub header: String,
    /// Lines with gutters numbered from the header's starts.
    pub lines: Vec<NumberedLine>,
}

/// A served file patch split into hunks, plus the non-hunk lines git reports for the file
/// (a binary change, a mode change, a rename), which carry no code to number.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedPatch {
    pub hunks: Vec<ParsedHunk>,
    pub notes: Vec<String>,
}

/// The old and new starting line numbers of a `@@ -a,b +c,d @@` header.
fn starts(header: &str) -> Option<(u32, u32)> {
    let mut fields = header.strip_prefix("@@ ")?.split(' ');
    let old = fields.next()?.strip_prefix('-')?;
    let new = fields.next()?.strip_prefix('+')?;
    let first = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((first(old)?, first(new)?))
}

/// Parses served unified-diff text. When `complete` is false more pages follow, so a final
/// line without its newline is cut mid-way and is left for the next page rather than shown.
pub fn parse_patch(text: &str, complete: bool) -> ParsedPatch {
    let usable = if complete || text.ends_with('\n') {
        text
    } else {
        text.rfind('\n').map_or("", |end| &text[..=end])
    };
    let mut patch = ParsedPatch::default();
    let mut counters = (0, 0);
    for raw in usable.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("@@") {
            if let Some(start) = starts(line) {
                counters = start;
                patch.hunks.push(ParsedHunk {
                    header: line.to_owned(),
                    lines: Vec::new(),
                });
                continue;
            }
        }
        let Some(current) = patch.hunks.last_mut() else {
            if [
                "Binary files",
                "GIT binary patch",
                "rename ",
                "copy ",
                "old mode ",
                "new mode ",
                "new file mode ",
                "deleted file mode ",
                "similarity index ",
                "dissimilarity index ",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
            {
                patch.notes.push(line.to_owned());
            }
            continue;
        };
        let (kind, code) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Add, &line[1..]),
            Some(b'-') => (LineKind::Del, &line[1..]),
            Some(b' ') => (LineKind::Context, &line[1..]),
            None => (LineKind::Context, ""),
            // `\ No newline at end of file` and any trailer belong to no side.
            Some(_) => continue,
        };
        let (old, new) = &mut counters;
        current.lines.push(numbered(kind, code, None, old, new));
    }
    patch
}

/// The zero-based index of the line showing `number` on the requested side.
pub fn line_index(lines: &[NumberedLine], number: u32, old_side: bool) -> Option<usize> {
    lines.iter().position(|line| {
        if old_side {
            line.old == Some(number) && line.kind != LineKind::Add
        } else {
            line.new == Some(number) && line.kind != LineKind::Del
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number_lines(lines: &[LineKind], start: (u32, u32)) -> Vec<NumberedLine> {
        let (mut old, mut new) = start;
        lines
            .iter()
            .map(|&kind| numbered(kind, "", None, &mut old, &mut new))
            .collect()
    }

    #[test]
    fn split_pairs_following_additions_without_dropping_unmatched_lines() {
        for (deleted, added) in [(3, 1), (1, 3), (2, 2), (0, 2), (2, 0)] {
            let mut source_lines = vec![LineKind::Context];
            source_lines.extend((0..deleted).map(|_| LineKind::Del));
            source_lines.extend((0..added).map(|_| LineKind::Add));
            source_lines.push(LineKind::Context);
            let lines = number_lines(&source_lines, (10, 20));
            let rows = split_rows(&lines, &[]);
            assert_eq!(rows.len(), deleted.max(added) + 2);
            assert_eq!(rows[0].left, rows[0].right);
            for j in 0..deleted.max(added) {
                let row = &rows[j + 1];
                assert_eq!(row.left, (j < deleted).then(|| lines[1 + j].clone()));
                assert_eq!(
                    row.right,
                    (j < added).then(|| lines[1 + deleted + j].clone())
                );
            }
            assert_eq!(
                rows.last().map(|row| row.left.clone()),
                rows.last().map(|row| row.right.clone())
            );
        }
    }

    #[test]
    fn unified_numbering_does_not_advance_the_absent_side() {
        let lines = [
            LineKind::Context,
            LineKind::Del,
            LineKind::Del,
            LineKind::Add,
            LineKind::Context,
            LineKind::Add,
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
    fn split_discussion_follows_the_row_of_its_anchored_line_once() {
        let lines = number_lines(
            &[
                LineKind::Del,
                LineKind::Del,
                LineKind::Del,
                LineKind::Add,
                LineKind::Add,
            ],
            (1, 1),
        );
        let placed = |after| {
            split_rows(
                &lines,
                &[Anchor {
                    after,
                    discussion: 0,
                }],
            )
            .iter()
            .map(|row| !row.discussions.is_empty())
            .collect::<Vec<_>>()
        };
        assert_eq!(placed(4), vec![false, true, false]);
        assert_eq!(placed(2), vec![false, false, true]);
    }

    #[test]
    fn served_patch_numbers_each_hunk_and_holds_back_a_cut_final_line() {
        let text = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,2 +3,2 @@ fn x\n a\n-b\n+c\n\\ No newline at end of file\n@@ -40 +40,2 @@\n z\n+partial";
        let patch = parse_patch(text, false);
        assert_eq!(patch.hunks.len(), 2);
        let first: Vec<_> = patch.hunks[0]
            .lines
            .iter()
            .map(|line| (line.old, line.new, line.text.as_str()))
            .collect();
        assert_eq!(
            first,
            vec![
                (Some(3), Some(3), "a"),
                (Some(4), None, "b"),
                (None, Some(4), "c")
            ]
        );
        assert_eq!(patch.hunks[1].lines.len(), 1);
        assert_eq!(parse_patch(text, true).hunks[1].lines.len(), 2);
        assert_eq!(line_index(&patch.hunks[0].lines, 4, true), Some(1));
        assert_eq!(line_index(&patch.hunks[0].lines, 4, false), Some(2));
    }

    #[test]
    fn mode_only_and_empty_file_changes_preserve_git_metadata() {
        for headers in [
            "old mode 100644\nnew mode 100755\n",
            "new file mode 100644\n",
            "deleted file mode 100644\n",
        ] {
            let patch = parse_patch(&format!("diff --git a/x b/x\n{headers}"), true);
            assert!(patch.hunks.is_empty());
            assert_eq!(patch.notes, headers.lines().collect::<Vec<_>>());
        }
    }
}
