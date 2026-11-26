//! Read Git objects without taking inputs from the checkout or invoking diff helpers.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::model::{Activity, ActivityCommit, ActivityEntry, FileChange, Revision};

pub(crate) mod diagram;

/// Only blobs up to 4 MiB are retained for syntax and manifest scans. Larger blobs
/// are still streamed for line counts, without retaining their text.
const MAX_SCAN_BYTES: u64 = 4 * 1024 * 1024;
const DAY: i64 = 24 * 60 * 60;
const NULL: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

fn command(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(name);
        }
    }
    cmd.env("LC_ALL", "C")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NULL)
        .env("GIT_GRAFT_FILE", NULL)
        .env("GIT_INDEX_FILE", "")
        .env("GIT_OPTIONAL_LOCKS", "0")
        // Don't fetch missing objects from a partial clone.
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .args(["--no-pager", "--no-replace-objects", "-C"])
        .arg(repo)
        .args([
            "-c",
            "advice.graftFileDeprecated=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            &format!("core.attributesFile={NULL}"),
            "-c",
            &format!("diff.orderFile={NULL}"),
        ])
        .stdin(Stdio::null());
    cmd
}

fn output(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    run(command(repo), repo, args)
}

fn run(mut command: Command, repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = command
        .args(args)
        .output()
        .with_context(|| format!("running git in {}: {args:?}", repo.display()))?;
    ensure!(
        out.status.success(),
        "git in {} {args:?} failed ({}): {}",
        repo.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr).trim_end()
    );
    Ok(out.stdout)
}

pub(crate) fn check_oid(oid: &str) -> Result<()> {
    ensure!(
        matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()),
        "expected a full Git object ID, got {oid:?}"
    );
    Ok(())
}

pub(crate) fn revision(repo: &Path, rev: &str) -> Result<Revision> {
    ensure!(!rev.is_empty(), "expected a Git revision");
    let resolved = output(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ],
    )?;
    let commit = std::str::from_utf8(&resolved)
        .context("Git commit ID is not UTF-8")?
        .trim_end();
    check_oid(commit)?;
    let metadata = output(
        repo,
        &[
            "show",
            "--no-patch",
            "--no-notes",
            "--no-show-signature",
            "--no-decorate",
            "--no-color",
            "--format=%T%n%ct",
            commit,
            "--",
        ],
    )?;
    let (tree, time) = std::str::from_utf8(&metadata)
        .context("Git commit metadata is not UTF-8")?
        .trim_end()
        .split_once('\n')
        .context("missing Git commit metadata")?;
    check_oid(tree)?;
    Ok(Revision {
        commit: commit.to_owned(),
        tree: tree.to_owned(),
        committed_at: time.parse().context("invalid Git commit timestamp")?,
        shallow_boundary: shallow_boundary(repo)?,
    })
}

fn common_dir(repo: &Path) -> Result<PathBuf> {
    let bytes = output(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let bytes = bytes
        .strip_suffix(b"\n")
        .context("missing Git common directory")?;
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(bytes)
    };
    #[cfg(not(unix))]
    let path = std::str::from_utf8(bytes).context("Git common directory is not UTF-8")?;
    Ok(PathBuf::from(path))
}

fn shallow_boundary(repo: &Path) -> Result<Vec<String>> {
    let path = common_dir(repo)?.join("shallow");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let mut oids = Vec::new();
    for oid in text.lines() {
        check_oid(oid)?;
        oids.push(oid.to_ascii_lowercase());
    }
    oids.sort_unstable();
    oids.dedup();
    Ok(oids)
}

/// Diff from Git objects without the original repo's local config or info/attributes.
/// The temporary bare repository uses the original object database read-only.
struct DiffContext {
    dir: tempfile::TempDir,
    objects: PathBuf,
}

impl DiffContext {
    fn new(repo: &Path, commit: &str, shallow: &[String]) -> Result<Self> {
        check_oid(commit)?;
        let objects = common_dir(repo)?.join("objects");
        let dir = tempfile::tempdir().context("create temporary Git diff directory")?;
        let object_format = if commit.len() == 64 { "sha256" } else { "sha1" };
        output(
            dir.path(),
            &[
                "init",
                "--bare",
                "--template=",
                &format!("--object-format={object_format}"),
            ],
        )?;
        if !shallow.is_empty() {
            fs::write(
                dir.path().join("shallow"),
                format!("{}\n", shallow.join("\n")),
            )
            .context("write temporary Git shallow boundary")?;
        }
        Ok(Self { dir, objects })
    }

    fn output(&self, args: &[&str]) -> Result<Vec<u8>> {
        let dir = self.dir.path();
        let mut git = command(dir);
        git.env("GIT_OBJECT_DIRECTORY", &self.objects);
        run(git, dir, args)
    }
}

pub(crate) struct TreeFile<'a> {
    pub path: &'a str,
    pub text: Option<&'a str>,
    pub lines: u64,
    pub regular_blob: Option<&'a str>,
}

fn records(bytes: &[u8]) -> Result<impl Iterator<Item = &[u8]>> {
    let mut records = bytes.split(|&b| b == 0);
    ensure!(
        records.next_back() == Some(&[][..]),
        "unterminated Git NUL record"
    );
    Ok(records)
}

pub(crate) fn tree(
    repo: &Path,
    tree: &str,
    visit: impl FnMut(TreeFile<'_>) -> Result<()>,
) -> Result<u64> {
    tree_selected(repo, tree, |_| true, visit)
}

/// Visit selected paths with the same bounded blob reader as the structure scanner.
/// Paths that do not pass the predicate never have their blobs opened.
pub(crate) fn tree_selected(
    repo: &Path,
    tree: &str,
    mut select: impl FnMut(&str) -> bool,
    mut visit: impl FnMut(TreeFile<'_>) -> Result<()>,
) -> Result<u64> {
    check_oid(tree)?;
    let entries = output(repo, &["ls-tree", "-rz", "--full-tree", tree, "--"])?;
    if entries.is_empty() {
        return Ok(0);
    }
    let mut child = command(repo)
        // Stream loose and non-delta packed blobs at the same threshold as the scanner.
        .args(["-c", "core.bigFileThreshold=4m", "cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning git cat-file --batch in {}", repo.display()))?;
    let result: Result<u64> = (|| {
        let mut input = child.stdin.take().context("missing Git batch stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().context("missing Git batch stdout")?);
        let mut header = String::new();
        let mut blob = Vec::new();
        let mut skipped = 0;
        for entry in records(&entries)? {
            let mut fields = entry.splitn(2, |&b| b == b'\t');
            let meta = fields.next().context("missing Git tree metadata")?;
            let path = fields.next().context("missing Git tree path")?;
            let Ok(path) = std::str::from_utf8(path) else {
                skipped += 1;
                continue;
            };
            if !select(path) {
                continue;
            }
            let mut meta = std::str::from_utf8(meta)
                .context("Git tree metadata is not UTF-8")?
                .split_ascii_whitespace();
            let mode = meta.next().context("missing Git tree mode")?;
            let kind = meta.next().context("missing Git tree kind")?;
            let oid = meta.next().context("missing Git tree object ID")?;
            let regular_blob =
                (matches!(mode, "100644" | "100755") && kind == "blob").then_some(oid);
            let (text, lines) = if let Some(oid) = regular_blob {
                check_oid(oid)?;
                // At most one request is in flight: never fill stdin while the child
                // is waiting for us to drain stdout. Paths never enter this protocol.
                writeln!(input, "{oid}").context("requesting Git batch blob")?;
                header.clear();
                stdout
                    .read_line(&mut header)
                    .context("reading Git batch header")?;
                let mut fields = header.split_ascii_whitespace();
                ensure!(
                    fields.next() == Some(oid) && fields.next() == Some("blob"),
                    "unexpected Git batch response for {oid}: {header:?}"
                );
                let len = fields
                    .next()
                    .context("missing Git batch blob size")?
                    .parse::<u64>()
                    .context("invalid Git batch blob size")?;
                let item = if len <= MAX_SCAN_BYTES {
                    blob.resize(len as usize, 0);
                    stdout
                        .read_exact(&mut blob)
                        .context("reading Git batch blob")?;
                    let text = std::str::from_utf8(&blob)
                        .ok()
                        .filter(|_| !blob.contains(&0));
                    let lines = text.map_or(0, |text| {
                        newlines(text.as_bytes())
                            + u64::from(!text.is_empty() && !text.ends_with('\n'))
                    });
                    (text, lines)
                } else {
                    (
                        None,
                        large_blob_lines(&mut stdout, len)
                            .context("reading large Git batch blob")?,
                    )
                };
                let mut terminator = [0];
                stdout
                    .read_exact(&mut terminator)
                    .context("reading Git batch blob terminator")?;
                ensure!(terminator == [b'\n'], "invalid Git batch blob terminator");
                item
            } else {
                (None, 0)
            };
            visit(TreeFile {
                path,
                text,
                lines,
                regular_blob,
            })?;
        }
        Ok(skipped)
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().context("waiting for git cat-file --batch")?;
    let skipped = result?;
    ensure!(status.success(), "git cat-file --batch failed: {status}");
    Ok(skipped)
}

fn newlines(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|&&b| b == b'\n').count() as u64
}

fn large_blob_lines(mut input: impl Read, mut len: u64) -> Result<u64> {
    const CHUNK: usize = 64 * 1024;
    // Preserve up to three bytes of a partial UTF-8 character across chunk boundaries.
    let mut buf = [0; CHUNK + 3];
    let mut carried = 0;
    let mut lines = 0;
    let mut last = b'\n';
    let mut binary = false;
    while len != 0 {
        let size = len.min(CHUNK as u64) as usize;
        let end = carried + size;
        let new = &mut buf[carried..end];
        input.read_exact(new)?;
        len -= size as u64;
        if binary || new.contains(&0) {
            binary = true;
            continue; // Drain the rest without decoding; the next blob shares stdout.
        }
        lines += newlines(new);
        last = buf[end - 1];
        carried = match std::str::from_utf8(&buf[..end]) {
            Ok(_) => 0,
            Err(err) if err.error_len().is_none() => {
                buf.copy_within(err.valid_up_to()..end, 0);
                end - err.valid_up_to()
            }
            Err(_) => {
                binary = true;
                0
            }
        };
    }
    Ok(if binary || carried != 0 {
        0
    } else {
        lines + u64::from(last != b'\n')
    })
}

const NUMSTAT: &[&str] = &[
    "--numstat",
    "-z",
    "--no-renames",
    "--no-textconv",
    "--no-ext-diff",
    "--ignore-submodules=none",
    "--no-relative",
    "--no-color",
    "--diff-algorithm=myers",
    "--indent-heuristic",
];

struct Stat<'a> {
    path: &'a [u8],
    added: u64,
    removed: u64,
    binary: bool,
}

impl Stat<'_> {
    fn file(&self, path: &str) -> FileChange {
        FileChange {
            path: path.to_owned(),
            added: self.added,
            removed: self.removed,
            binary: self.binary,
        }
    }
}

fn numstat(record: &[u8]) -> Result<Stat<'_>> {
    let mut fields = record.splitn(3, |&b| b == b'\t');
    let added = fields.next().context("missing Git numstat additions")?;
    let removed = fields.next().context("missing Git numstat deletions")?;
    let path = fields.next().context("missing Git numstat path")?;
    ensure!(!path.is_empty(), "unexpected empty Git numstat path");
    let binary = added == b"-" && removed == b"-";
    let parse = |count| -> Result<u64> {
        std::str::from_utf8(count)
            .context("invalid Git numstat count")?
            .parse()
            .context("invalid Git numstat count")
    };
    Ok(Stat {
        path,
        added: if binary { 0 } else { parse(added)? },
        removed: if binary { 0 } else { parse(removed)? },
        binary,
    })
}

/// Aggregate non-merge commit diffs (including roots) from all reachable branches.
/// This avoids double-counting merged branch changes; merge-only edits aren't counted.
/// Both windows include their endpoints. Binary changes count as touches with no churn.
pub(crate) fn activity(repo: &Path, rev: &Revision) -> Result<Activity> {
    check_oid(&rev.commit)?;
    let day_7 = rev.committed_at.saturating_sub(7 * DAY);
    let day_30 = rev.committed_at.saturating_sub(30 * DAY);
    // Read attributes from the requested commit, not the user's index/worktree.
    let attrs = format!("--attr-source={}", rev.commit);
    let cutoff = format!("--since-as-filter=@{day_30}");
    let mut args = vec![
        attrs.as_str(),
        "log",
        "--format=%x00%H%x00%ct%x00%s",
        "--date-order",
        "--encoding=UTF-8",
        "--no-notes",
        "--no-show-signature",
        "--no-decorate",
        "--no-follow",
        "--no-merges",
        "--root",
    ];
    if day_30 > 0 {
        // Unlike --since, this won't prune an old commit's potentially newer parents.
        args.push(&cutoff);
    }
    args.extend_from_slice(NUMSTAT);
    args.extend_from_slice(&[&rev.commit, "--"]);
    let bytes = DiffContext::new(repo, &rev.commit, &rev.shallow_boundary)?.output(&args)?;
    let mut records = records(&bytes)?;
    let mut current = None;
    let mut commits: Vec<ActivityCommit> = Vec::new();
    let mut days_7 = BTreeMap::new();
    let mut days_30 = BTreeMap::new();
    let mut skipped_non_utf8_paths = 0;
    while let Some(record) = records.next() {
        if record.is_empty() {
            let oid = std::str::from_utf8(records.next().context("missing Git log commit ID")?)
                .context("Git log commit ID is not UTF-8")?;
            check_oid(oid)?;
            let time = std::str::from_utf8(records.next().context("missing Git log timestamp")?)
                .context("Git log timestamp is not UTF-8")?
                .parse::<i64>()
                .context("invalid Git log timestamp")?;
            let subject =
                String::from_utf8_lossy(records.next().context("missing Git log subject")?);
            current = None;
            // Git treats a shallow boundary as a root; it cannot tell us what
            // changed there relative to the missing parent.
            if (day_30..=rev.committed_at).contains(&time)
                && rev
                    .shallow_boundary
                    .binary_search_by(|boundary| boundary.as_str().cmp(oid))
                    .is_err()
            {
                current = Some(commits.len());
                commits.push(ActivityCommit {
                    commit: oid.to_owned(),
                    committed_at: time,
                    subject: subject.into_owned(),
                    files: Vec::new(),
                });
            }
            continue;
        }
        let Some(index) = current else { continue };
        let commit = &mut commits[index];
        // Git separates a log header from its first diff record with an LF.
        let stat = numstat(record.strip_prefix(b"\n").unwrap_or(record))?;
        let Ok(path) = std::str::from_utf8(stat.path) else {
            skipped_non_utf8_paths += 1;
            continue;
        };
        touch(&mut days_30, path, &stat, commit);
        if commit.committed_at >= day_7 {
            touch(&mut days_7, path, &stat, commit);
        }
        commit.files.push(stat.file(path));
    }
    Ok(Activity {
        days_7: days_7.into_values().collect(),
        days_30: days_30.into_values().collect(),
        commits,
        shallow: !rev.shallow_boundary.is_empty(),
        skipped_non_utf8_paths,
    })
}

fn touch<'a>(
    entries: &mut BTreeMap<&'a str, ActivityEntry>,
    path: &'a str,
    stat: &Stat<'_>,
    commit: &ActivityCommit,
) {
    let entry = entries.entry(path).or_insert_with(|| ActivityEntry {
        path: path.to_owned(),
        commits: 0,
        added: 0,
        removed: 0,
        last_commit: commit.commit.clone(),
        last_committed_at: commit.committed_at,
        last_subject: commit.subject.clone(),
    });
    entry.commits += 1;
    entry.added += stat.added;
    entry.removed += stat.removed;
    // --date-order visits descendants before ancestors even when timestamps tie.
    if commit.committed_at > entry.last_committed_at {
        entry.last_committed_at = commit.committed_at;
        entry.last_commit.clone_from(&commit.commit);
        entry.last_subject.clone_from(&commit.subject);
    }
}

/// Compare two explicit immutable commit IDs; renames appear as delete/add.
pub(crate) fn changes(repo: &Path, base: &str, head: &str) -> Result<(Vec<FileChange>, u64)> {
    check_oid(base)?;
    check_oid(head)?;
    let attrs = format!("--attr-source={head}");
    let mut args = vec![attrs.as_str(), "diff-tree", "--no-commit-id", "-r"];
    args.extend_from_slice(NUMSTAT);
    args.extend_from_slice(&[base, head, "--"]);
    let bytes = DiffContext::new(repo, head, &[])?.output(&args)?;
    let mut changes = Vec::new();
    let mut skipped = 0;
    for record in records(&bytes)? {
        let stat = numstat(record)?;
        if let Ok(path) = std::str::from_utf8(stat.path) {
            changes.push(stat.file(path));
        } else {
            skipped += 1;
        }
    }
    changes.sort_unstable_by(|lhs, rhs| lhs.path.cmp(&rhs.path));
    Ok((changes, skipped))
}

/// Both ranges describe actual edited lines, not surrounding context. A pure
/// insertion has an empty old range; a pure deletion has an empty new range.
pub(crate) struct LineHunk {
    pub old: std::ops::Range<u64>,
    pub new: std::ops::Range<u64>,
}

pub(crate) struct LineDiff(DiffContext);

impl LineDiff {
    pub(crate) fn new(repo: &Path, commit: &str) -> Result<Self> {
        Ok(Self(DiffContext::new(repo, commit, &[])?))
    }

    /// Compare regular text blobs whose size was already bounded by the scanner.
    pub(crate) fn hunks(&self, old: &str, new: &str) -> Result<Vec<LineHunk>> {
        check_oid(old)?;
        check_oid(new)?;
        let patch = self.0.output(&[
            "diff",
            "--unified=0",
            "--inter-hunk-context=0",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-indent-heuristic",
            "--diff-algorithm=myers",
            "--text",
            old,
            new,
            "--",
        ])?;
        std::str::from_utf8(&patch)
            .context("Git line diff is not UTF-8")?
            .lines()
            .filter_map(|line| line.strip_prefix("@@ "))
            .map(|header| {
                let mut fields = header.split_ascii_whitespace();
                Ok(LineHunk {
                    old: line_range(fields.next(), '-')?,
                    new: line_range(fields.next(), '+')?,
                })
            })
            .collect()
    }
}

fn line_range(field: Option<&str>, sign: char) -> Result<std::ops::Range<u64>> {
    let span = field
        .and_then(|s| s.strip_prefix(sign))
        .context("missing Git hunk line range")?;
    let (start, count) = span.split_once(',').unwrap_or((span, "1"));
    let start = start.parse::<u64>().context("invalid Git hunk start")?;
    let count = count.parse::<u64>().context("invalid Git hunk length")?;
    // Git denotes an empty span by the preceding line (0 at the beginning).
    let start = start
        .checked_add(u64::from(count == 0))
        .context("Git hunk start overflow")?;
    let end = start.checked_add(count).context("Git hunk end overflow")?;
    Ok(start..end)
}
