//! Persistent memo of Git facts about immutable full-SHA pairs.
//!
//! A fact is keyed by the repository (canonical common Git directory and object format), a
//! fingerprint of the effective Git configuration, a command-semantics version, the fact kind
//! and two full lowercase object IDs. Short SHAs, branches and other revspecs are never
//! memoized: they are computed directly every time. Repositories where one full SHA can name
//! different history (shallow clones, grafts, replace refs, reftable) are never memoized.
//! A process re-resolves the repository identity when a config or history file it read changes.
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
const SEMANTICS: &str = "2";
/// Record fields before the payload: semantics, repo key, object format, config, kind, two SHAs.
const PREFIX_FIELDS: usize = 7;
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
    /// `git diff --name-only` paths of an unvalidated object pair, as configured.
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
    /// Fingerprint of the effective Git configuration the facts were computed under.
    config: String,
}

/// A resolution of one repository path and the state that invalidates it.
struct Resolved {
    id: Option<RepoId>,
    watched: Vec<(PathBuf, Option<Stamp>)>,
    env: u64,
}

type Facts = HashMap<String, Vec<u8>>;

fn files() -> &'static Mutex<HashMap<PathBuf, Option<Facts>>> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, Option<Facts>>>> = OnceLock::new();
    FILES.get_or_init(Mutex::default)
}

fn repos() -> &'static Mutex<HashMap<PathBuf, Resolved>> {
    static REPOS: OnceLock<Mutex<HashMap<PathBuf, Resolved>>> = OnceLock::new();
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

    /// Paths printed by `git diff --no-ext-diff --no-textconv --name-only -z <older> <newer> --`,
    /// as raw bytes in Git's order. Effective configuration (rename detection, submodule
    /// ignores) applies as it would to that command; no commit validation is performed and a
    /// failing diff is an error that is not stored.
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
        names(&raw)
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
                "{SEMANTICS} {} {} {} {} {} {}",
                id.key,
                id.format,
                id.config,
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
    let mut fields = body.splitn(PREFIX_FIELDS + 1, |b| *b == b' ');
    let mut prefix = Vec::new();
    for _ in 0..PREFIX_FIELDS {
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

/// Identity of `repo` as Git currently reads it. Resolved once per process and re-resolved when
/// a watched config or history file, or a config-related environment variable, changes.
fn repo_id(repo: &Path) -> Option<RepoId> {
    let mut repos = repos().lock().ok()?;
    let env = config_env();
    let fresh = repos.get(repo).is_some_and(|known| {
        known.env == env
            && known
                .watched
                .iter()
                .all(|(path, seen)| stamp(path) == *seen)
    });
    if !fresh {
        repos.insert(repo.to_path_buf(), resolve_repo(repo, env));
    }
    repos.get(repo)?.id.clone()
}

/// Variables that can change effective config or object reachability for Git subprocesses.
fn config_env() -> u64 {
    let mut vars: Vec<_> = std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            name.starts_with("GIT_") || name == "HOME" || name == "XDG_CONFIG_HOME"
        })
        .collect();
    vars.sort();
    let mut bytes = Vec::new();
    for (name, value) in vars {
        bytes.extend_from_slice(name.as_encoded_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(value.as_encoded_bytes());
        bytes.push(0);
    }
    checksum(&bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    inode: (u64, i64, i64),
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::symlink_metadata(path).ok()?;
    Some(Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
        #[cfg(unix)]
        inode: {
            use std::os::unix::fs::MetadataExt;
            (meta.ino(), meta.ctime(), meta.ctime_nsec())
        },
    })
}

fn resolve_repo(repo: &Path, env: u64) -> Resolved {
    let mut resolved = Resolved {
        id: None,
        watched: Vec::new(),
        env,
    };
    let Ok(out) = git_utf8(
        repo,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--show-object-format",
            "--absolute-git-dir",
            "--is-shallow-repository",
        ],
    ) else {
        return resolved;
    };
    let lines: Vec<_> = out.lines().collect();
    let [common, format, git_dir, shallow] = lines[..] else {
        return resolved;
    };
    let Ok(common) = fs::canonicalize(common) else {
        return resolved;
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    let mut paths = vec![
        common.join("config"),
        Path::new(git_dir).join("config.worktree"),
        common.join("shallow"),
        common.join("info/grafts"),
        common.join("refs/replace"),
        common.join("packed-refs"),
        PathBuf::from("/etc/gitconfig"),
    ];
    paths.extend(home.map(|h| h.join(".gitconfig")));
    paths.extend(xdg.map(|x| x.join("git/config")));
    for name in ["GIT_CONFIG_SYSTEM", "GIT_CONFIG_GLOBAL"] {
        paths.extend(std::env::var_os(name).map(PathBuf::from));
    }
    let watch = |paths: Vec<PathBuf>, watched: &mut Vec<(PathBuf, Option<Stamp>)>| {
        for path in paths {
            if !watched.iter().any(|(seen, _)| *seen == path) {
                let stamp = stamp(&path);
                watched.push((path, stamp));
            }
        }
    };
    watch(paths, &mut resolved.watched);
    let Some((config, origins)) = effective_config(repo) else {
        return resolved;
    };
    watch(origins, &mut resolved.watched);
    if shallow != "false"
        || !matches!(format, "sha1" | "sha256")
        || mutable_history(&common, &config)
    {
        return resolved;
    }
    let mut identity = common.as_os_str().as_encoded_bytes().to_vec();
    identity.push(0);
    identity.extend_from_slice(format.as_bytes());
    resolved.id = Some(RepoId {
        key: format!("{:016x}", checksum(&identity)),
        format: format.to_owned(),
        config: format!("{:016x}", checksum(&config)),
    });
    resolved
}

/// Effective `git config --list` entries (without origins) and the files they came from.
fn effective_config(repo: &Path) -> Option<(Vec<u8>, Vec<PathBuf>)> {
    let args = ["config", "--list", "--show-origin", "-z"];
    let out = crate::git_raw(repo, &args).ok()?;
    if !out.status.success() {
        return None;
    }
    let mut entries = Vec::new();
    let mut origins = Vec::new();
    let mut records = out.stdout.split(|b| *b == 0);
    while let Some(origin) = records.next().filter(|origin| !origin.is_empty()) {
        let entry = records.next()?;
        entries.extend_from_slice(entry);
        entries.push(0);
        if let Some(file) = origin.strip_prefix(b"file:") {
            let file = std::str::from_utf8(file).ok()?;
            origins.push(repo.join(file));
        }
    }
    Some((entries, origins))
}

/// True when the same full SHAs can denote different history: replace refs, grafts, or a ref
/// store this check cannot inspect. Such repositories are never memoized.
fn mutable_history(common: &Path, config: &[u8]) -> bool {
    let reftable = config.split(|b| *b == 0).any(|entry| {
        entry
            .to_ascii_lowercase()
            .starts_with(b"extensions.refstorage\n")
    });
    let replace_dir = fs::read_dir(common.join("refs/replace"))
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    let packed_replace = match fs::File::open(common.join("packed-refs")) {
        Ok(file) => {
            let mut bytes = Vec::new();
            let read = file.take(FILE_CAP + 1).read_to_end(&mut bytes);
            read.is_err()
                || bytes.len() as u64 > FILE_CAP
                || bytes.windows(14).any(|w| w == b" refs/replace/")
        }
        Err(_) => false,
    };
    reftable || replace_dir || packed_replace || common.join("info/grafts").exists()
}

fn landed_raw(repo: &Path, older: &str, newer: &str) -> Result<Vec<u8>> {
    crate::work::record();
    let args = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--name-only",
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
    names(&bytes)?;
    Ok(bytes)
}

/// The records of `--name-only -z` output.
fn names(raw: &[u8]) -> Result<Vec<Vec<u8>>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let raw = raw
        .strip_suffix(&[0])
        .context("unterminated git NUL output")?;
    Ok(raw.split(|b| *b == 0).map(<[u8]>::to_vec).collect())
}
