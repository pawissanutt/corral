//! Bounded fuzzy search under a retained checkout descriptor. No file contents are served.
use std::{
    borrow::Cow,
    collections::BTreeSet,
    fs::File,
    io::{self, Read},
    os::fd::{AsRawFd, OwnedFd},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_protocol::detail::SessionFileSearch;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rustix::fs::{Dir, FileType, Mode, OFlags, Stat, fstat, openat2};

use super::linux::{CONFINED, DIR, PATH_BYTES, visible};
use crate::views::json_len;

const SCAN: usize = 16_384;
const DEPTH: usize = 32;
const TIME: Duration = Duration::from_millis(500);
const RESULT_JSON_BYTES: usize = 40 * 1024;
const IGNORE_FILE_BYTES: usize = 16 * 1024;
const IGNORE_TOTAL_BYTES: usize = 192 * 1024;
const IGNORE_FILE_RULES: usize = 256;
const IGNORE_TOTAL_RULES: usize = 2048;
const IGNORE_RULE_BYTES: usize = 1024;
const PIN: OFlags = OFlags::PATH.union(OFlags::CLOEXEC).union(OFlags::NOFOLLOW);

type Rank = (u8, usize, usize, usize);

pub(super) fn read(root: &OwnedFd, query: &str, limit: usize) -> Result<SessionFileSearch> {
    let query = query.to_lowercase();
    let mut search = Search {
        root,
        query: &query,
        needle: query.chars().collect(),
        deadline: Instant::now() + TIME,
        entries_left: SCAN,
        ignore_bytes_left: IGNORE_TOTAL_BYTES,
        ignore_rules_left: IGNORE_TOTAL_RULES,
        limit,
        hits: BTreeSet::new(),
        truncated: false,
    };
    search.walk("", 0, &mut Vec::new());

    let mut paths = Vec::with_capacity(search.hits.len());
    let mut left = RESULT_JSON_BYTES;
    for (_, path) in search.hits {
        // Include actual string escaping and separators, leaving room for both reply envelopes.
        let bytes = json_len(&path)? + 1;
        if bytes > left {
            search.truncated = true;
            break;
        }
        left -= bytes;
        paths.push(path);
    }
    Ok(SessionFileSearch {
        paths,
        truncated: search.truncated,
    })
}

struct Scope {
    start: usize,
    matcher: Gitignore,
}

struct Search<'a> {
    root: &'a OwnedFd,
    query: &'a str,
    needle: Vec<char>,
    deadline: Instant,
    entries_left: usize,
    ignore_bytes_left: usize,
    ignore_rules_left: usize,
    limit: usize,
    hits: BTreeSet<(Rank, String)>,
    truncated: bool,
}

impl Search<'_> {
    fn check_time(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }

    fn timed<T>(&self, call: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        // This is cooperative: one filesystem call can block past the deadline.
        self.check_time()?;
        let result = call();
        self.check_time()?;
        result
    }

    fn open(&self, path: &str, flags: OFlags) -> io::Result<OwnedFd> {
        // Always resolve from the checkout, not from a directory that may have been moved
        // outside it since readdir. Names from Dir alone never authorize a result or read.
        self.timed(|| openat2(self.root, path, flags, Mode::empty(), CONFINED).map_err(Into::into))
    }

    fn inspect(&self, path: &str) -> io::Result<Stat> {
        let pin = self.open(path, PIN)?;
        self.timed(|| fstat(&pin).map_err(Into::into))
    }

    fn walk(&mut self, path: &str, depth: usize, stack: &mut Vec<Scope>) {
        let dir_path = if path.is_empty() { "." } else { path };
        let Ok(mut dir) = self
            .open(dir_path, DIR)
            .and_then(|fd| self.timed(|| Dir::new(fd).map_err(Into::into)))
        else {
            self.truncated = true;
            return;
        };
        let Ok(scope) = self.load_ignore(path) else {
            // An unreadable, unsafe or over-budget ignore file is not permission to expose
            // this subtree without its rules. Other branches can still be searched.
            self.truncated = true;
            return;
        };
        let has_scope = scope.is_some();
        stack.extend(scope);
        loop {
            if self.entries_left == 0 {
                self.truncated = true;
                break;
            }
            let entry = match self.timed(|| dir.read().transpose().map_err(Into::into)) {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(_) => {
                    self.truncated = true;
                    break;
                }
            };
            self.entries_left -= 1;
            let Ok(name) = entry.file_name().to_str() else {
                continue;
            };
            if !visible(name) {
                continue;
            }
            let mut kind = entry.file_type();
            if !matches!(
                kind,
                FileType::Directory | FileType::RegularFile | FileType::Unknown
            ) {
                continue;
            }
            if path.len() + usize::from(!path.is_empty()) + name.len() > PATH_BYTES {
                self.truncated = true;
                continue;
            }
            let relative = child(path, name);
            let stat = if kind == FileType::Unknown {
                let Ok(stat) = self.inspect(&relative) else {
                    self.truncated = true;
                    continue;
                };
                kind = FileType::from_raw_mode(stat.st_mode);
                Some(stat)
            } else {
                None
            };
            let directory = kind == FileType::Directory;
            // Closest .gitignore with an opinion wins; its last matching rule wins.
            if stack
                .iter()
                .rev()
                .map(|scope| scope.matcher.matched(&relative[scope.start..], directory))
                .find(|matched| !matched.is_none())
                .is_some_and(|matched| matched.is_ignore())
            {
                // In particular, Git cannot re-include a file under an excluded parent.
                continue;
            }
            if directory {
                if depth < DEPTH {
                    self.walk(&relative, depth + 1, stack);
                } else {
                    self.truncated = true;
                }
            } else if kind == FileType::RegularFile {
                self.consider(relative, stat);
            }
        }
        if has_scope {
            stack.pop();
        }
    }

    fn consider(&mut self, path: String, stat: Option<Stat>) {
        let Some(rank) = rank(&path.to_lowercase(), self.query, &self.needle) else {
            return;
        };
        let stat = match stat.map_or_else(|| self.inspect(&path), Ok) {
            Ok(stat) => stat,
            Err(_) => {
                self.truncated = true;
                return;
            }
        };
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
            return;
        }
        self.hits.insert((rank, path));
        if self.hits.len() > self.limit {
            self.hits.pop_last();
            self.truncated = true;
        }
    }

    fn load_ignore(&mut self, path: &str) -> Result<Option<Scope>> {
        let pin = match self.open(&child(path, ".gitignore"), PIN) {
            Ok(pin) => pin,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).context("pin worktree .gitignore"),
        };
        let before = self
            .timed(|| fstat(&pin).map_err(Into::into))
            .context("stat worktree .gitignore")?;
        // Like Git, never follow a .gitignore symlink. Do not read special files either.
        if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile {
            return Ok(None);
        }
        let cap = IGNORE_FILE_BYTES.min(self.ignore_bytes_left);
        ensure!(
            before.st_nlink == 1
                && u64::try_from(before.st_size).is_ok_and(|size| size <= cap as u64),
            "unsafe or over-budget worktree .gitignore"
        );
        let mut file = self
            .timed(|| File::open(format!("/proc/self/fd/{}", pin.as_raw_fd())))
            .context("open pinned worktree .gitignore")?;
        let mut data = Vec::new();
        let mut buf = [0; 4096];
        while data.len() <= cap {
            let len = buf.len().min(cap + 1 - data.len());
            let n = self
                .timed(|| file.read(&mut buf[..len]))
                .context("read pinned worktree .gitignore")?;
            if n == 0 {
                break;
            }
            self.ignore_bytes_left = self.ignore_bytes_left.saturating_sub(n);
            data.extend_from_slice(&buf[..n]);
        }
        ensure!(data.len() <= cap, "over-budget worktree .gitignore");
        let after = self
            .timed(|| fstat(&file).map_err(Into::into))
            .context("restat worktree .gitignore")?;
        ensure!(
            after.st_nlink == 1
                && (before.st_size, before.st_ctime, before.st_ctime_nsec)
                    == (after.st_size, after.st_ctime, after.st_ctime_nsec),
            "worktree .gitignore changed while being read"
        );

        let text = std::str::from_utf8(&data).context("non-UTF-8 worktree .gitignore")?;
        let mut builder = GitignoreBuilder::new(".");
        for (i, line) in text
            .strip_prefix('\u{feff}')
            .unwrap_or(text)
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .enumerate()
        {
            self.check_time()?;
            ensure!(
                i < IGNORE_FILE_RULES
                    && self.ignore_rules_left > 0
                    && line.len() <= IGNORE_RULE_BYTES,
                "over-budget worktree .gitignore rules"
            );
            self.ignore_rules_left -= 1;
            builder
                .add_line(None, &literal_braces(line))
                .context("parse worktree .gitignore")?;
        }
        self.check_time()?;
        let matcher = builder.build().context("compile worktree .gitignore")?;
        self.check_time()?;
        Ok((!matcher.is_empty()).then_some(Scope {
            start: path.len() + usize::from(!path.is_empty()),
            matcher,
        }))
    }
}

fn literal_braces(line: &str) -> Cow<'_, str> {
    if !line.contains(['{', '}']) {
        return Cow::Borrowed(line);
    }
    // Git's wildmatch treats braces literally. Globset treats them as alternation,
    // so a negated Git rule could otherwise re-include additional names.
    let mut escaped = String::with_capacity(line.len());
    let mut chars = line.chars();
    let mut classes = true;
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                escaped.push(ch);
                escaped.extend(chars.next());
            }
            '[' if classes => {
                escaped.push(ch);
                // A leading ] (after an optional negation) is itself a class member.
                // Leave the whole class unchanged; globset does not expand braces there.
                let prefix = chars.as_str();
                let first = 1 + usize::from(prefix.starts_with(['!', '^']));
                if let Some((at, _)) = prefix.char_indices().skip(first).find(|&(_, c)| c == ']') {
                    escaped.push_str(&prefix[..=at]);
                    chars = prefix[at + 1..].chars();
                } else {
                    classes = false;
                }
            }
            '{' | '}' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    Cow::Owned(escaped)
}

fn child(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}/{name}")
    }
}

fn rank(path: &str, query: &str, needle: &[char]) -> Option<Rank> {
    let basename = path.rsplit('/').next()?;
    let stem = basename.rsplit_once('.').map_or(basename, |(stem, _)| stem);
    if query == basename || query == stem {
        Some((0, 0, 0, path.len()))
    } else if basename.starts_with(query) {
        Some((1, 0, basename.len(), path.len()))
    } else if let Some(at) = basename.find(query) {
        Some((2, at, basename.len(), path.len()))
    } else if let Some(at) = path.rfind(query) {
        Some((3, 0, path.len() - at - query.len(), path.len()))
    } else {
        // Reward tight matches near the basename, including cross-component matches such
        // as sess[ion/]mod.rs. Try the basename separately in case earlier directories match.
        let (gaps, tail) = [
            fuzzy(path, needle, query.len()),
            fuzzy(basename, needle, query.len()),
        ]
        .into_iter()
        .flatten()
        .min()?;
        Some((4, gaps, tail, path.len()))
    }
}

fn fuzzy(haystack: &str, needle: &[char], query_bytes: usize) -> Option<(usize, usize)> {
    let mut chars = haystack.char_indices();
    let mut end = 0;
    for &want in needle {
        let (at, ch) = chars.find(|&(_, ch)| ch == want)?;
        end = at + ch.len_utf8();
    }
    // Tighten the first complete match by aligning backwards from its end. Linear in the
    // path length, rather than enumerating all possible subsequence alignments.
    let mut chars = haystack[..end].char_indices().rev();
    let mut start = end;
    for &want in needle.iter().rev() {
        start = chars.find(|&(_, ch)| ch == want)?.0;
    }
    Some((end - start - query_bytes, haystack.len() - end))
}

#[cfg(test)]
mod tests {
    include!("session_file_search_tests.rs");
}
