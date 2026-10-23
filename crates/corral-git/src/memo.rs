//! Persistent memo of Git facts about immutable full-SHA pairs.
//!
//! A fact is keyed by the repository (canonical common Git directory and object format), a
//! command-semantics version, the fact kind and two full lowercase object IDs. Short SHAs,
//! branches and other revspecs are never memoized: they are computed directly every time.
//!
//! Storage is one append-only file per repository. Every record is framed as `\n<record>\n`,
//! carries its own checksum and is written with a single `O_APPEND` write, so concurrent writers
//! never interleave inside a record and a torn record never merges into a neighbour. A file is
//! loaded once per process; unparsable, mismatched or corrupt records are ignored, and deleting
//! the file is always safe (facts are recomputed).

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail};

use crate::history::{git_utf8, is_full_oid, nul_records};
use crate::{changed_paths_between, changed_paths_with_renames, merge_base};

/// Bumped whenever the Git command or parsing behind any fact kind changes.
const SEMANTICS: &str = "1";
/// Memo files larger than this are neither read nor extended.
const FILE_CAP: u64 = 64 * 1024 * 1024;
/// Facts whose encoded payload exceeds this are computed every time instead of stored.
const PAYLOAD_CAP: usize = 1024 * 1024;
/// Bound on the raw `git diff` output read for [`Memo::landed_paths`].
const OUTPUT_CAP: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    /// `changed_paths_between`: sorted paths, renames as delete/add.
    Paths,
    /// `changed_paths_with_renames`: sorted paths, both rename endpoints.
    Renames,
    /// `merge_base`.
    MergeBase,
    /// Rename-detected destination paths of an unvalidated object pair.
    Landed,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Paths => "paths",
            Kind::Renames => "renames",
            Kind::MergeBase => "merge-base",
            Kind::Landed => "landed",
        }
    }
}

/// Identity of one repository: memo file key and object format.
#[derive(Clone, PartialEq, Eq)]
struct RepoId {
    key: String,
    format: String,
}

type Facts = HashMap<String, Vec<u8>>;

fn files() -> &'static Mutex<HashMap<PathBuf, Option<Facts>>> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, Option<Facts>>>> = OnceLock::new();
    FILES.get_or_init(Mutex::default)
}

fn repos() -> &'static Mutex<HashMap<PathBuf, Option<RepoId>>> {
    static REPOS: OnceLock<Mutex<HashMap<PathBuf, Option<RepoId>>>> = OnceLock::new();
    REPOS.get_or_init(Mutex::default)
}

/// A memo rooted at one directory, or a disabled memo that always computes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memo {
    dir: Option<PathBuf>,
}

impl Memo {
    /// A memo storing `<dir>/<repo-key>.memo` files.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
        }
    }

    /// A memo that never reads or writes and computes every fact.
    pub fn disabled() -> Self {
        Self { dir: None }
    }

    /// The memo file used for `repo`, when the memo is enabled and the repository resolves.
    pub fn file_for(&self, repo: &Path) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let id = repo_id(repo)?;
        Some(dir.join(format!("{}.memo", id.key)))
    }

    /// Memoized [`changed_paths_between`].
    ///
    /// # Errors
    /// The same errors as [`changed_paths_between`]; memo I/O never fails the call.
    pub fn changed_paths_between(
        &self,
        repo: &Path,
        older: &str,
        newer: &str,
    ) -> Result<Vec<String>> {
        self.paths(repo, Kind::Paths, older, newer, changed_paths_between)
    }

    /// Memoized [`changed_paths_with_renames`].
    ///
    /// # Errors
    /// The same errors as [`changed_paths_with_renames`]; memo I/O never fails the call.
    pub fn changed_paths_with_renames(
        &self,
        repo: &Path,
        older: &str,
        newer: &str,
    ) -> Result<Vec<String>> {
        self.paths(
            repo,
            Kind::Renames,
            older,
            newer,
            changed_paths_with_renames,
        )
    }

    /// Memoized [`merge_base`].
    ///
    /// # Errors
    /// The same errors as [`merge_base`]; memo I/O never fails the call.
    pub fn merge_base(&self, repo: &Path, base: &str, head: &str) -> Result<String> {
        let compute = || merge_base(repo, base, head);
        let Some(slot) = self.slot(repo, Kind::MergeBase, base, head) else {
            return compute();
        };
        if let Some(sha) = slot
            .get()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .filter(|sha| is_full_oid(sha))
        {
            return Ok(sha);
        }
        let sha = compute()?;
        slot.put(sha.as_bytes());
        Ok(sha)
    }

    /// Destination paths of `git diff --name-status -M` between two objects, as raw bytes in Git's
    /// output order, without commit validation (a failing diff is an error and is not stored).
    ///
    /// # Errors
    /// Returns an error when Git fails, its output exceeds 64 MiB, or the output is malformed.
    pub fn landed_paths(&self, repo: &Path, older: &str, newer: &str) -> Result<Vec<Vec<u8>>> {
        let raw = match self.slot(repo, Kind::Landed, older, newer) {
            Some(slot) => match slot.get() {
                Some(raw) => raw,
                None => {
                    let raw = landed_raw(repo, older, newer)?;
                    slot.put(&raw);
                    raw
                }
            },
            None => landed_raw(repo, older, newer)?,
        };
        destinations(&raw)
    }

    fn paths(
        &self,
        repo: &Path,
        kind: Kind,
        older: &str,
        newer: &str,
        compute: fn(&Path, &str, &str) -> Result<Vec<String>>,
    ) -> Result<Vec<String>> {
        let Some(slot) = self.slot(repo, kind, older, newer) else {
            return compute(repo, older, newer);
        };
        if let Some(paths) = slot.get().and_then(|bytes| decode_paths(&bytes)) {
            return Ok(paths);
        }
        let paths = compute(repo, older, newer)?;
        let mut bytes = Vec::new();
        for path in &paths {
            bytes.extend_from_slice(path.as_bytes());
            bytes.push(0);
        }
        slot.put(&bytes);
        Ok(paths)
    }

    /// The memo location for one fact, or `None` when it must not be memoized.
    pub(crate) fn slot(&self, repo: &Path, kind: Kind, a: &str, b: &str) -> Option<Slot> {
        let dir = self.dir.as_ref()?;
        if !is_full_oid(a) || !is_full_oid(b) || a.len() != b.len() {
            return None;
        }
        let id = repo_id(repo)?;
        let width = match id.format.as_str() {
            "sha1" => 40,
            "sha256" => 64,
            _ => return None,
        };
        if a.len() != width {
            return None;
        }
        Some(Slot {
            file: dir.join(format!("{}.memo", id.key)),
            prefix: format!(
                "{SEMANTICS} {} {} {} {} {}",
                id.key,
                id.format,
                kind.tag(),
                a.to_ascii_lowercase(),
                b.to_ascii_lowercase()
            ),
        })
    }
}

pub(crate) struct Slot {
    file: PathBuf,
    /// Every record field before the payload; also the in-memory key.
    pub(crate) prefix: String,
}

impl Slot {
    pub(crate) fn get(&self) -> Option<Vec<u8>> {
        let mut files = files().lock().ok()?;
        let facts = files
            .entry(self.file.clone())
            .or_insert_with(|| load(&self.file));
        facts.as_ref()?.get(&self.prefix).cloned()
    }

    /// Best effort: a fact that cannot be stored is simply recomputed by the next process.
    fn put(&self, payload: &[u8]) {
        let record = encode_record(&self.prefix, payload);
        if record.len() > PAYLOAD_CAP {
            return;
        }
        let Ok(mut files) = files().lock() else {
            return;
        };
        let Some(Some(facts)) = files.get_mut(&self.file) else {
            return;
        };
        if append(&self.file, &record).is_ok() {
            facts.insert(self.prefix.clone(), payload.to_vec());
        }
    }
}

fn append(file: &Path, record: &[u8]) -> Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut handle = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .with_context(|| format!("opening {}", file.display()))?;
    let meta = handle.metadata()?;
    if !meta.is_file() || meta.len() > FILE_CAP {
        bail!("memo file is not a bounded regular file");
    }
    let written = handle.write(record)?;
    if written != record.len() {
        bail!("short memo write");
    }
    Ok(())
}

/// Facts in one memo file; `None` disables the file (special file or over the size cap).
/// A missing file is an empty memo.
pub(crate) fn load(file: &Path) -> Option<Facts> {
    let mut facts = Facts::new();
    let Ok(handle) = fs::File::open(file) else {
        return Some(facts);
    };
    let meta = handle.metadata().ok()?;
    if !meta.is_file() || meta.len() > FILE_CAP {
        return None;
    }
    let mut bytes = Vec::new();
    handle.take(FILE_CAP + 1).read_to_end(&mut bytes).ok()?;
    if u64::try_from(bytes.len()).ok()? > FILE_CAP {
        return None;
    }
    // A final record without its terminating newline may be torn; drop it.
    let complete = match bytes.iter().rposition(|b| *b == b'\n') {
        Some(end) => &bytes[..end],
        None => &[][..],
    };
    for line in complete.split(|b| *b == b'\n') {
        if let Some((prefix, payload)) = decode_record(line) {
            facts.entry(prefix).or_insert(payload);
        }
    }
    Some(facts)
}

/// `\n<prefix> <escaped payload> <checksum>\n`.
pub(crate) fn encode_record(prefix: &str, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(prefix.len() + payload.len() + 2);
    body.extend_from_slice(prefix.as_bytes());
    body.push(b' ');
    for byte in payload {
        match byte {
            b'\\' => body.extend_from_slice(b"\\\\"),
            b'\n' => body.extend_from_slice(b"\\n"),
            other => body.push(*other),
        }
    }
    let mut record = vec![b'\n'];
    record.extend_from_slice(&body);
    record.extend_from_slice(format!(" {:016x}\n", checksum(&body)).as_bytes());
    record
}

pub(crate) fn decode_record(line: &[u8]) -> Option<(String, Vec<u8>)> {
    let split = line.iter().rposition(|b| *b == b' ')?;
    let (body, sum) = (&line[..split], &line[split + 1..]);
    if sum.len() != 16 || std::str::from_utf8(sum).ok()? != format!("{:016x}", checksum(body)) {
        return None;
    }
    let mut fields = body.splitn(7, |b| *b == b' ');
    let mut prefix = Vec::new();
    for _ in 0..6 {
        let field = fields.next()?;
        if !prefix.is_empty() {
            prefix.push(b' ');
        }
        prefix.extend_from_slice(field);
    }
    if !prefix.starts_with(format!("{SEMANTICS} ").as_bytes()) {
        return None;
    }
    let mut payload = Vec::new();
    let mut escaped = fields.next()?.iter();
    while let Some(byte) = escaped.next() {
        payload.push(match byte {
            b'\\' => match escaped.next()? {
                b'\\' => b'\\',
                b'n' => b'\n',
                _ => return None,
            },
            other => *other,
        });
    }
    Some((String::from_utf8(prefix).ok()?, payload))
}

/// FNV-1a 64: detects torn and corrupted records; not a security boundary.
pub(crate) fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn decode_paths(bytes: &[u8]) -> Option<Vec<String>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let paths: Vec<String> = nul_records(text)
        .ok()?
        .into_iter()
        .map(str::to_owned)
        .collect();
    paths.iter().all(|p| !p.is_empty()).then_some(paths)
}

/// Canonical common Git directory and object format of `repo`, resolved once per process.
fn repo_id(repo: &Path) -> Option<RepoId> {
    let mut repos = repos().lock().ok()?;
    repos
        .entry(repo.to_path_buf())
        .or_insert_with(|| resolve_repo(repo))
        .clone()
}

fn resolve_repo(repo: &Path) -> Option<RepoId> {
    let out = git_utf8(
        repo,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--show-object-format",
        ],
    )
    .ok()?;
    let mut lines = out.lines();
    let common = fs::canonicalize(lines.next()?).ok()?;
    let format = lines.next()?.to_owned();
    if !matches!(format.as_str(), "sha1" | "sha256") {
        return None;
    }
    let mut identity = common.as_os_str().as_encoded_bytes().to_vec();
    identity.push(0);
    identity.extend_from_slice(format.as_bytes());
    Some(RepoId {
        key: format!("{:016x}", checksum(&identity)),
        format,
    })
}

fn landed_raw(repo: &Path, older: &str, newer: &str) -> Result<Vec<u8>> {
    crate::work::record();
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--name-status",
        "-M",
        "-z",
        older,
        newer,
        "--",
    ];
    let mut command = Command::new("git");
    crate::scrub_env(&mut command, std::env::vars_os().map(|(name, _)| name));
    let mut child = command
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning git diff in {}", repo.display()))?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .context("git diff stdout missing")?
        .take(u64::try_from(OUTPUT_CAP + 1)?)
        .read_to_end(&mut bytes);
    if bytes.len() > OUTPUT_CAP || read.is_err() {
        child.kill().context("stopping bounded git diff")?;
    }
    let status = child.wait().context("waiting for git diff")?;
    read.context("reading git diff")?;
    if bytes.len() > OUTPUT_CAP {
        bail!("git diff output exceeds {OUTPUT_CAP} bytes");
    }
    if !status.success() {
        bail!("git diff {older} {newer} failed ({status})");
    }
    destinations(&bytes)?;
    Ok(bytes)
}

/// The final path of each `--name-status -z` record (the destination of a rename or copy).
fn destinations(raw: &[u8]) -> Result<Vec<Vec<u8>>> {
    let Some(raw) = raw.strip_suffix(&[0]) else {
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        bail!("unterminated git NUL output");
    };
    let mut records = raw.split(|b| *b == 0);
    let mut paths = Vec::new();
    while let Some(status) = records.next() {
        let mut path = records.next().context("missing git diff path")?;
        if matches!(status.first(), Some(b'R' | b'C')) {
            path = records.next().context("missing git diff rename endpoint")?;
        }
        paths.push(path.to_vec());
    }
    Ok(paths)
}
