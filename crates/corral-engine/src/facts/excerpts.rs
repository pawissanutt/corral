//! Committed source around each located lens element, so an owner reading a lens sees the
//! anchored line without a file viewer. Only Git objects at the checked commit are read,
//! never the working tree; every read and the result are bounded, and a file that cannot be
//! read within bounds (missing, too large, not UTF-8) yields no excerpt rather than a guess.

use std::collections::BTreeMap;
use std::rc::Rc;

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
/// Excerpt text kept per lens; later elements go without once it is spent.
pub const LENS_EXCERPT_BYTES: usize = 16 * 1024;

impl FactsReader<'_> {
    /// One list per result, in the results' order.
    pub fn lens_excerpts(&self, rev: &Revision, results: &[LensResult]) -> Vec<Vec<LensExcerpt>> {
        let mut files: BTreeMap<String, Option<Rc<Vec<String>>>> = BTreeMap::new();
        results
            .iter()
            .map(|result| {
                let mut used = 0usize;
                let mut out = Vec::new();
                for location in &result.locations {
                    if !files.contains_key(&location.path) {
                        let read = (files.len() < MAX_FILES)
                            .then(|| self.source_lines(rev, &location.path))
                            .flatten()
                            .map(Rc::new);
                        files.insert(location.path.clone(), read);
                    }
                    let Some(Some(lines)) = files.get(&location.path) else {
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
                    if used + size > LENS_EXCERPT_BYTES {
                        break;
                    }
                    used += size;
                    out.push(excerpt);
                }
                out
            })
            .collect()
    }

    fn source_lines(&self, rev: &Revision, path: &str) -> Option<Vec<String>> {
        let blob = corral_git::committed_blob(&self.repo, rev.commit(), path).ok()??;
        let size = corral_git::blob_size(&self.repo, &blob).ok()?;
        if size > MAX_BLOB {
            return None;
        }
        let mut bytes = Vec::with_capacity(size as usize);
        corral_git::stream_blob(&self.repo, &blob, |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })
        .ok()?;
        let text = String::from_utf8(bytes).ok()?;
        Some(text.lines().map(str::to_owned).collect())
    }
}

fn clip(line: &str) -> String {
    match line.char_indices().nth(LINE_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

/// The lines around one-based `line`; none when the file has no such line.
fn excerpt(lines: &[String], element: &str, path: &str, line: u64) -> Option<LensExcerpt> {
    let total = lines.len() as u64;
    if line == 0 || line > total {
        return None;
    }
    let first = line.saturating_sub(CONTEXT).max(1);
    let last = (line + CONTEXT).min(total);
    Some(LensExcerpt {
        element: element.to_owned(),
        path: path.to_owned(),
        line,
        first,
        lines: lines[(first - 1) as usize..last as usize]
            .iter()
            .map(|l| clip(l))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A location past the end of its committed file (a stale line) must not show a guess.
    #[test]
    fn a_line_outside_the_file_has_no_excerpt() {
        let lines: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        assert!(excerpt(&lines, "state x", "f", 4).is_none());
        let shown = excerpt(&lines, "state x", "f", 1).map(|e| (e.first, e.lines.len()));
        assert_eq!(shown, Some((1, 3)));
    }
}
