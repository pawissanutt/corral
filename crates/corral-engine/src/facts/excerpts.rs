//! Committed source around a review or lens anchor, without opening a file viewer.
//! Only Git objects at the requested commit are read, never the working tree. Each blob is
//! streamed once and only the lines an excerpt needs are kept. A file that cannot be read
//! within bounds (missing, too large, not UTF-8 where an excerpt needs it) yields no excerpt.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::anyhow;
use corral_facts::Revision;
use corral_types::{
    SourceExcerpt,
    lenses::{LensExcerpt, LensResult},
};

use super::FactsReader;

/// Lines shown on each side of the anchored line.
const CONTEXT: u64 = 2;
/// A longer line is clipped to this many characters.
const LINE_CHARS: usize = 200;
/// A larger blob is not read.
const MAX_BLOB: u64 = 1024 * 1024;
/// Distinct files read for one check.
const MAX_FILES: usize = 64;
/// Located elements given an excerpt in one check.
const MAX_LOCATIONS: usize = 2048;
/// Excerpt text kept per lens; later elements go without once it is spent.
const LENS_EXCERPT_BYTES: usize = 16 * 1024;
/// Excerpt text kept for one check across all its lenses.
const CHECK_EXCERPT_BYTES: usize = 256 * 1024;

/// Source review context has its own budget (per revision).
const SOURCE_EXCERPT_BYTES: usize = 64 * 1024;
const SOURCE_LINE_BYTES: usize = 200;

/// A file's lines by one-based number.
type Wanted = BTreeMap<String, BTreeSet<u64>>;
type Kept = BTreeMap<u64, String>;

fn window(line: u64) -> std::ops::RangeInclusive<u64> {
    line.saturating_sub(CONTEXT).max(1)..=line.saturating_add(CONTEXT)
}

fn want(wanted: &mut Wanted, path: &str, line: u64) {
    if line != 0 && (wanted.len() < MAX_FILES || wanted.contains_key(path)) {
        wanted
            .entry(path.to_owned())
            .or_default()
            .extend(window(line));
    }
}

impl FactsReader<'_> {
    /// One list per result, in the results' order.
    pub fn lens_excerpts(&self, rev: &Revision, results: &[LensResult]) -> Vec<Vec<LensExcerpt>> {
        let mut wanted = Wanted::new();
        for location in results
            .iter()
            .flat_map(|r| &r.locations)
            .take(MAX_LOCATIONS)
        {
            want(&mut wanted, &location.path, location.line);
        }
        let read = self.read_wanted(rev, wanted);
        let mut spent = 0usize;
        results
            .iter()
            .map(|result| {
                let mut used = 0usize;
                let mut out = Vec::new();
                for location in &result.locations {
                    let Some(lines) = read.get(&location.path) else {
                        continue;
                    };
                    let Some(excerpt) =
                        excerpt(lines, &location.element, &location.path, location.line)
                    else {
                        continue;
                    };
                    let size = excerpt.lines.iter().map(String::len).sum::<usize>()
                        + excerpt.path.len()
                        + excerpt.element.len();
                    if used + size > LENS_EXCERPT_BYTES || spent + size > CHECK_EXCERPT_BYTES {
                        break;
                    }
                    used += size;
                    spent += size;
                    out.push(excerpt);
                }
                out
            })
            .collect()
    }

    /// Committed lines around each anchor, in input order. At most 64 files, 2048
    /// anchors and 64 KiB of excerpt text are retained per call.
    pub fn line_excerpts(
        &self,
        rev: &Revision,
        anchors: &[(String, u64)],
    ) -> Vec<Option<SourceExcerpt>> {
        let mut wanted = Wanted::new();
        for (path, line) in anchors.iter().take(MAX_LOCATIONS) {
            want(&mut wanted, path, *line);
        }
        let read = self.read_wanted(rev, wanted);
        let mut remaining = SOURCE_EXCERPT_BYTES;
        let mut excerpts: Vec<_> = anchors
            .iter()
            .take(MAX_LOCATIONS)
            .map(|(path, line)| {
                if remaining == 0 {
                    return None;
                }
                let (first, mut lines) = excerpt_lines(read.get(path)?, *line)?;
                for text in &mut lines {
                    // Bound UTF-8 text bytes as well as the display width.
                    if text.len() > SOURCE_LINE_BYTES {
                        text.truncate(text.floor_char_boundary(SOURCE_LINE_BYTES));
                        text.push('…');
                    }
                }
                let size = lines.iter().map(String::len).sum();
                let Some(left) = remaining.checked_sub(size) else {
                    remaining = 0;
                    return None;
                };
                remaining = left;
                Some(SourceExcerpt {
                    path: path.clone(),
                    commit: rev.commit().to_owned(),
                    line: *line,
                    first,
                    lines,
                })
            })
            .collect();
        excerpts.resize_with(anchors.len(), || None);
        excerpts
    }

    fn read_wanted(&self, rev: &Revision, wanted: Wanted) -> BTreeMap<String, Kept> {
        let mut read = BTreeMap::new();
        for (path, lines) in wanted {
            if self.cancelled.is_some_and(|cancelled| cancelled()) {
                break;
            }
            if let Some(kept) = self.wanted_lines(rev, &path, &lines) {
                read.insert(path, kept);
            }
        }
        read
    }

    /// The `wanted` lines of a committed file, clipped; none when it cannot be read within
    /// bounds or a wanted line is not UTF-8. Lines past the file's end are simply absent.
    fn wanted_lines(&self, rev: &Revision, path: &str, wanted: &BTreeSet<u64>) -> Option<Kept> {
        let blob = corral_git::committed_blob(&self.repo, rev.commit(), path).ok()??;
        if corral_git::blob_size(&self.repo, &blob).ok()? > MAX_BLOB {
            return None;
        }
        let last = *wanted.last()?;
        let mut kept = BTreeMap::new();
        let mut line = 1u64;
        let mut current: Vec<u8> = Vec::new();
        let mut keep = |number: u64, bytes: &[u8]| -> anyhow::Result<()> {
            if wanted.contains(&number) {
                let text = std::str::from_utf8(bytes).map_err(|_| anyhow!("not UTF-8"))?;
                kept.insert(number, clip(text.strip_suffix('\r').unwrap_or(text)));
            }
            Ok(())
        };
        let streamed = corral_git::stream_blob(&self.repo, &blob, |chunk| {
            for &byte in chunk {
                if byte == b'\n' {
                    keep(line, &current)?;
                    current.clear();
                    line += 1;
                    if line > last {
                        // Every wanted line is kept; stop reading the rest of the blob.
                        return Err(anyhow!("done"));
                    }
                } else if wanted.contains(&line) {
                    current.push(byte);
                }
            }
            Ok(())
        });
        match streamed {
            Ok(()) => {
                if !current.is_empty() {
                    keep(line, &current).ok()?;
                }
            }
            Err(error) if line > last && error.to_string() == "done" => {}
            Err(_) => return None,
        }
        Some(kept)
    }
}

/// The first `LINE_CHARS` characters of a line, an ellipsis marking the cut.
fn clip(line: &str) -> String {
    match line.char_indices().nth(LINE_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

fn excerpt(lines: &Kept, element: &str, path: &str, line: u64) -> Option<LensExcerpt> {
    let (first, lines) = excerpt_lines(lines, line)?;
    Some(LensExcerpt {
        element: element.to_owned(),
        path: path.to_owned(),
        line,
        first,
        lines,
    })
}

/// The kept lines around one-based `line`; none when the file has no such line.
fn excerpt_lines(lines: &Kept, line: u64) -> Option<(u64, Vec<String>)> {
    lines.get(&line)?;
    let window = window(line);
    let first = *window.start();
    let shown = window.map_while(|n| lines.get(&n).cloned()).collect();
    Some((first, shown))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A location past the end of its committed file (a stale line) must not show a guess.
    #[test]
    fn a_line_outside_the_file_has_no_excerpt() {
        let lines: BTreeMap<u64, String> = [(1, "a"), (2, "b"), (3, "c")]
            .into_iter()
            .map(|(n, s)| (n, s.to_owned()))
            .collect();
        assert!(excerpt(&lines, "state x", "f", 4).is_none());
        let shown = excerpt(&lines, "state x", "f", 1).map(|e| (e.first, e.lines.len()));
        assert_eq!(shown, Some((1, 3)));
    }
}
