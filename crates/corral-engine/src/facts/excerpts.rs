//! Committed source around each located lens element, so an owner reading a lens sees the
//! anchored line without a file viewer. Only Git objects at the checked commit are read,
//! never the working tree. Each blob is streamed once and only the lines an excerpt needs are
//! kept, so memory follows the excerpts, not the files; reads, kept text and output are all
//! bounded. A file that cannot be read within bounds (missing, too large, not UTF-8 where an
//! excerpt needs it) yields no excerpt rather than a guess.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::anyhow;
use corral_facts::Revision;
use corral_types::lenses::{LensExcerpt, LensResult};

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

/// The lines one file's excerpts need, by one-based number.
type Wanted = BTreeMap<String, BTreeSet<u64>>;

impl FactsReader<'_> {
    /// One list per result, in the results' order.
    pub fn lens_excerpts(&self, rev: &Revision, results: &[LensResult]) -> Vec<Vec<LensExcerpt>> {
        let mut wanted = Wanted::new();
        for location in results
            .iter()
            .flat_map(|r| &r.locations)
            .take(MAX_LOCATIONS)
        {
            if location.line == 0
                || (!wanted.contains_key(&location.path) && wanted.len() >= MAX_FILES)
            {
                continue;
            }
            let first = location.line.saturating_sub(CONTEXT).max(1);
            wanted
                .entry(location.path.clone())
                .or_default()
                .extend(first..=location.line + CONTEXT);
        }
        let mut read: BTreeMap<String, Option<BTreeMap<u64, String>>> = BTreeMap::new();
        for (path, lines) in wanted {
            if self.cancelled.is_some_and(|cancelled| cancelled()) {
                break;
            }
            let kept = self.wanted_lines(rev, &path, &lines);
            read.insert(path, kept);
        }
        let mut spent = 0usize;
        results
            .iter()
            .map(|result| {
                let mut used = 0usize;
                let mut out = Vec::new();
                for location in &result.locations {
                    let Some(Some(lines)) = read.get(&location.path) else {
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

    /// The `wanted` lines of a committed file, clipped; none when it cannot be read within
    /// bounds or a wanted line is not UTF-8. Lines past the file's end are simply absent.
    fn wanted_lines(
        &self,
        rev: &Revision,
        path: &str,
        wanted: &BTreeSet<u64>,
    ) -> Option<BTreeMap<u64, String>> {
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

/// The kept lines around one-based `line`; none when the file has no such line.
fn excerpt(
    lines: &BTreeMap<u64, String>,
    element: &str,
    path: &str,
    line: u64,
) -> Option<LensExcerpt> {
    lines.get(&line)?;
    let first = line.saturating_sub(CONTEXT).max(1);
    let shown: Vec<String> = (first..=line + CONTEXT)
        .map_while(|n| lines.get(&n).cloned())
        .collect();
    Some(LensExcerpt {
        element: element.to_owned(),
        path: path.to_owned(),
        line,
        first,
        lines: shown,
    })
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
