//! Read-only commit graph pages: a ref's tip, commits with every parent, and the refs at them.
//! Nothing here writes refs, the index, the object store or the working tree.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Read};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::bounded::{Bounded, Limits, read_bounded};
use crate::history::is_full_oid;
use crate::validate_branch_name;

/// Browsing must neither fetch missing objects into a partial clone (`GIT_NO_LAZY_FETCH`,
/// Git 2.44+) nor walk a history rewritten in place: replacement objects and grafts would
/// show parents the commits do not record.
const READ_ONLY: &[(&str, &str)] = &[
    ("GIT_NO_LAZY_FETCH", "1"),
    ("GIT_NO_REPLACE_OBJECTS", "1"),
    ("GIT_GRAFT_FILE", "/dev/null"),
];

/// Longest any one Git read of a page may run.
const DEADLINE: Duration = Duration::from_secs(10);
/// Ref labels kept per commit; the rest are counted.
pub const LABELS_MAX: usize = 16;
/// Bytes of ref listing read for one page; past it the labels are marked incomplete.
const LABEL_BYTES: u64 = 1 << 20;
/// Bytes of a commit object's header read; a longer header is refused.
const HEADER_BYTES: usize = 64 * 1024;
/// Bytes of a page's commit objects read in all; past it the page is unavailable.
const OBJECT_BYTES: u64 = 64 << 20;
/// Bytes of a one-line Git answer (an object ID, a type, a path).
const SMALL_BYTES: u64 = 64 * 1024;
/// Bytes of a shallow file read: one line per boundary commit.
const SHALLOW_BYTES: u64 = 16 << 20;
/// Longest subject or author name kept, in characters.
const TEXT_MAX: usize = 300;

/// A Git call whose whole answer is a line or so, under the same deadline as every read.
fn small(repo: &Path, args: &[&str]) -> Result<Bounded<Vec<u8>>> {
    let out = read_bounded(repo, args, &limits(&[], SMALL_BYTES), |out| {
        let mut bytes = Vec::new();
        out.read_to_end(&mut bytes).context("read git output")?;
        Ok(bytes)
    })?;
    if out.exceeded {
        bail!(
            "`git {}` answered more than {SMALL_BYTES} bytes",
            args.join(" ")
        );
    }
    Ok(out)
}

fn limits(input: &[u8], max_bytes: u64) -> Limits<'_> {
    Limits {
        env: READ_ONLY,
        input,
        max_bytes,
        deadline: DEADLINE,
    }
}

/// One commit as its object records it, with full IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCommit {
    pub sha: String,
    /// First parent first, as the commit object records them.
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    /// Strict ISO 8601 author and committer dates.
    pub authored: String,
    pub committed: String,
}

/// The ref labels of a page's commits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefLabels {
    /// At most `LABELS_MAX` per commit.
    pub labels: BTreeMap<String, Vec<String>>,
    /// Labels found past `LABELS_MAX`, per commit.
    pub omitted: BTreeMap<String, u64>,
    /// The listing was cut at its byte bound: any commit may have labels not counted.
    pub incomplete: bool,
}

/// The fully qualified ref for `branch` (or `HEAD`) and the commit it names now; `None` when
/// `HEAD` is unborn or the branch does not exist.
///
/// # Errors
/// Returns an error for an invalid branch name or a Git failure.
pub fn resolve_tip(repo: &Path, branch: Option<&str>) -> Result<(String, Option<String>)> {
    let reference = match branch {
        Some(name) => {
            validate_branch_name(name)?;
            format!("refs/heads/{name}")
        }
        None => "HEAD".to_owned(),
    };
    let peeled = format!("{reference}^{{commit}}");
    let args = [
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &peeled,
    ];
    let out = small(repo, &args)?;
    if !out.success() {
        if out.code == Some(1) && out.value.is_empty() {
            return Ok((reference, None));
        }
        bail!("git could not resolve {reference}");
    }
    let sha = String::from_utf8_lossy(&out.value).trim_end().to_owned();
    if !is_full_oid(&sha) {
        bail!("malformed rev-parse output for {reference}");
    }
    Ok((reference, Some(sha)))
}

/// At most `limit` commits reachable from the full commit ID `tip`, after skipping `skip`,
/// children before parents (`--topo-order`). The order is a function of the tip alone, so
/// pages from the same tip never overlap or leave gaps.
///
/// # Errors
/// Returns an error for an invalid tip, a Git failure, malformed output, or a repository
/// whose walk differs from the parents its commit objects record.
pub fn log_page(repo: &Path, tip: &str, skip: usize, limit: usize) -> Result<Vec<GraphCommit>> {
    if !is_full_oid(tip) {
        bail!("expected a full commit ID, got {tip:?}");
    }
    let kind = small(repo, &["cat-file", "-t", tip])?;
    if !kind.success() || kind.value != b"commit\n" {
        bail!("expected an existing full commit ID, got {tip:?}");
    }
    let skip = format!("--skip={skip}");
    let count = format!("--max-count={limit}");
    let args = [
        "log",
        "--topo-order",
        "--no-color",
        "--no-show-signature",
        "-z",
        "--format=%H%x1f%P%x1f%<(100,trunc)%an%x1f%aI%x1f%cI%x1f%<(300,trunc)%s",
        &skip,
        &count,
        tip,
        "--",
    ];
    // Git truncates the free-text fields, so a page's size is known; parents add the rest.
    let max_bytes = 64 * 1024 + 2048 * limit as u64;
    let out = read_bounded(repo, &args, &limits(&[], max_bytes), |out| {
        let mut bytes = Vec::new();
        out.read_to_end(&mut bytes).context("read git log")?;
        Ok(bytes)
    })?;
    if out.exceeded {
        bail!("git log page exceeded {max_bytes} bytes");
    }
    if !out.success() {
        bail!("git log failed from {tip}");
    }
    let mut commits = Vec::new();
    for record in out.value.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let record = String::from_utf8_lossy(record);
        let fields: Vec<&str> = record.splitn(6, '\x1f').collect();
        let [sha, parents, author, authored, committed, subject] = fields[..] else {
            bail!("malformed git log record");
        };
        let parents: Vec<String> = parents.split_whitespace().map(str::to_owned).collect();
        if !is_full_oid(sha) || !parents.iter().all(|p| is_full_oid(p)) {
            bail!("malformed git log commit ID");
        }
        commits.push(GraphCommit {
            sha: sha.to_owned(),
            parents,
            subject: clip(subject.trim_end()),
            author: clip(author.trim_end()),
            authored: authored.to_owned(),
            committed: committed.to_owned(),
        });
    }
    recorded_parents(repo, &mut commits)?;
    Ok(commits)
}

/// Check every commit's walked parents against its object. A shallow clone's boundary commits
/// show none in the walk, yet record some that are simply not here: those are taken from the
/// object, so none is drawn as a root. Any other difference means the walk is not the
/// recorded history, which is refused rather than drawn.
fn recorded_parents(repo: &Path, commits: &mut [GraphCommit]) -> Result<()> {
    if commits.is_empty() {
        return Ok(());
    }
    let input: String = commits.iter().map(|c| format!("{}\n", c.sha)).collect();
    // Only headers are kept; long messages are streamed past, within a byte bound and the
    // deadline (a cut object fails its declared size below).
    let recorded = read_bounded(
        repo,
        &["cat-file", "--batch"],
        &limits(input.as_bytes(), OBJECT_BYTES),
        |out| object_parents(out, commits.len()),
    )?;
    if !recorded.success() {
        bail!("git cat-file could not read the page's commits");
    }
    let mut shallow: Option<BTreeSet<String>> = None;
    for (commit, parents) in commits.iter_mut().zip(recorded.value) {
        if commit.parents == parents {
            continue;
        }
        if commit.parents.is_empty() {
            let declared = match &shallow {
                Some(declared) => declared,
                None => shallow.insert(shallow_commits(repo)?),
            };
            if !declared.contains(&commit.sha) {
                bail!(
                    "this repository overrides the parents of {} locally, so its recorded \
                     history cannot be shown",
                    commit.sha
                );
            }
            commit.parents = parents;
            continue;
        }
        bail!(
            "this repository overrides the parents of {} locally, so its recorded history \
             cannot be shown",
            commit.sha
        );
    }
    Ok(())
}

/// The boundary commits this repository declares shallow; empty when it is not shallow.
fn shallow_commits(repo: &Path) -> Result<BTreeSet<String>> {
    let shallow = small(repo, &["rev-parse", "--is-shallow-repository"])?;
    if !shallow.success() || shallow.value != b"true\n" {
        return Ok(BTreeSet::new());
    }
    let path = small(repo, &["rev-parse", "--git-path", "shallow"])?;
    if !path.success() {
        bail!("git could not name the shallow file");
    }
    let path = repo.join(String::from_utf8_lossy(&path.value).trim_end());
    let file = std::fs::File::open(&path).context("open the shallow file")?;
    let mut text = String::new();
    file.take(SHALLOW_BYTES + 1)
        .read_to_string(&mut text)
        .context("read the shallow file")?;
    if text.len() as u64 > SHALLOW_BYTES {
        bail!("shallow file longer than {SHALLOW_BYTES} bytes");
    }
    Ok(text
        .lines()
        .filter(|line| is_full_oid(line))
        .map(str::to_owned)
        .collect())
}

/// The `parent` lines of `count` commit objects from `git cat-file --batch`, reading at most
/// `HEADER_BYTES` of each header and skipping the rest of each object.
fn object_parents(out: &mut dyn BufRead, count: usize) -> Result<Vec<Vec<String>>> {
    let mut all = Vec::with_capacity(count);
    let mut line = Vec::new();
    for _ in 0..count {
        line.clear();
        (&mut *out)
            .take(256)
            .read_until(b'\n', &mut line)
            .context("read object header")?;
        let header = String::from_utf8_lossy(&line);
        let mut fields = header.split_whitespace();
        let (Some(_), Some("commit"), Some(size)) = (fields.next(), fields.next(), fields.next())
        else {
            bail!("not a commit object: {}", header.trim_end());
        };
        let size: u64 = size.parse().context("object size")?;
        let mut body = (&mut *out).take(size);
        let mut parents = Vec::new();
        let mut seen = 0;
        loop {
            line.clear();
            let n = (&mut body)
                .take((HEADER_BYTES - seen) as u64)
                .read_until(b'\n', &mut line)
                .context("read commit header")?;
            seen += n;
            if n == 0 || line == b"\n" {
                break;
            }
            if seen >= HEADER_BYTES {
                bail!("commit header longer than {HEADER_BYTES} bytes");
            }
            if let Some(parent) = line.strip_prefix(b"parent ") {
                let parent = String::from_utf8_lossy(parent).trim_end().to_owned();
                if !is_full_oid(&parent) {
                    bail!("malformed parent in commit object");
                }
                parents.push(parent);
            }
        }
        std::io::copy(&mut body, &mut std::io::sink()).context("skip commit message")?;
        if body.limit() != 0 {
            bail!("commit object shorter than its declared size");
        }
        line.clear();
        (&mut *out)
            .take(1)
            .read_until(b'\n', &mut line)
            .context("read object end")?;
        if line != b"\n" {
            bail!("commit object not followed by its newline");
        }
        all.push(parents);
    }
    Ok(all)
}

/// The local branches (`main`), tags (`tag: v1`) and `HEAD` pointing at each of `shas`, at
/// most `LABELS_MAX` each, read within a byte bound and a deadline.
///
/// # Errors
/// Returns an error for a Git failure.
pub fn ref_labels(repo: &Path, shas: &BTreeSet<String>) -> Result<RefLabels> {
    let mut found = RefLabels::default();
    if shas.is_empty() {
        return Ok(found);
    }
    let add = |found: &mut RefLabels, sha: &str, label: String| {
        let list = found.labels.entry(sha.to_owned()).or_default();
        if list.len() < LABELS_MAX {
            list.push(label);
        } else {
            *found.omitted.entry(sha.to_owned()).or_default() += 1;
        }
    };
    let head = small(repo, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    if head.success() {
        let head = String::from_utf8_lossy(&head.value).trim_end().to_owned();
        if shas.contains(&head) {
            add(&mut found, &head, "HEAD".into());
        }
    }
    let points: Vec<String> = shas
        .iter()
        .map(|sha| format!("--points-at={sha}"))
        .collect();
    let mut args = vec![
        "for-each-ref",
        "--format=%(objectname) %(*objectname) %(refname)",
    ];
    args.extend(points.iter().map(String::as_str));
    args.extend(["refs/heads", "refs/tags"]);
    let listed = read_bounded(repo, &args, &limits(&[], LABEL_BYTES), |out| {
        let mut line = Vec::new();
        loop {
            line.clear();
            if out.read_until(b'\n', &mut line).context("read refs")? == 0 {
                break;
            }
            // A line the byte bound cut short is not a whole ref.
            let Some(whole) = line.strip_suffix(b"\n") else {
                break;
            };
            // A ref name need not be UTF-8; it is only a label here.
            let text = String::from_utf8_lossy(whole);
            let mut fields = text.splitn(3, ' ');
            let (Some(object), Some(peeled), Some(name)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let target = if peeled.is_empty() { object } else { peeled };
            if !shas.contains(target) {
                continue;
            }
            let label = if let Some(branch) = name.strip_prefix("refs/heads/") {
                branch.to_owned()
            } else if let Some(tag) = name.strip_prefix("refs/tags/") {
                format!("tag: {tag}")
            } else {
                continue;
            };
            add(&mut found, target, label);
        }
        Ok(())
    })?;
    if !listed.exceeded && !listed.success() {
        bail!("git for-each-ref failed");
    }
    found.incomplete = listed.exceeded;
    Ok(found)
}

fn clip(text: &str) -> String {
    text.chars().take(TEXT_MAX).collect()
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
