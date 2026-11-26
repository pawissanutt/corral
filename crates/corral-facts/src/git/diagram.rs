//! Exact tree lookup and bounded source excerpts for diagram evidence.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, ensure};
use corral_types::SourceExcerpt;

use super::{check_oid, command, records};
use crate::Revision;

const MAX_PATH: usize = 4096;
const MAX_BLOB: usize = 1024 * 1024;
const MAX_LINES: usize = 200;
const MAX_JSON_LINES: usize = 4096;

/// A regular file entry in an immutable commit's tree. Its contents need not be
/// available locally or readable as text.
#[derive(Debug, Clone)]
pub struct CommittedFile {
    path: String,
    commit: String,
    blob: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommittedExcerpt {
    /// Actual contiguous lines at the requested start, possibly with a clipped last line.
    Resolved {
        excerpt: SourceExcerpt,
        truncated: bool,
    },
    /// There is no line at the requested start.
    Missing,
    /// The blob cannot be read as text within the limits.
    Unavailable,
}

/// Resolve an exact local commit at a configured repository root. Do not let Git
/// discover a containing workspace repository when the project path is not a root.
pub fn resolve_local_commit(repo: &Path, commit: &str) -> Result<Revision> {
    check_oid(commit)?;
    let prefix = bounded_output(repo, &["rev-parse", "--show-prefix"], None, MAX_PATH)?;
    ensure!(prefix == b"\n", "not the root of a Git repository");
    let revision = super::revision(repo, commit)?;
    ensure!(revision.commit() == commit, "not the exact commit object");
    Ok(revision)
}

/// Look up a literal repository-root-relative path in the resolved commit. Only
/// regular and executable blob entries count; absence is not a Git error.
pub fn committed_file(repo: &Path, rev: &Revision, path: &str) -> Result<Option<CommittedFile>> {
    ensure!(
        path.len() <= MAX_PATH
            && !path.contains('\0')
            && !path.split('/').any(|part| matches!(part, "" | "." | "..")),
        "expected a bounded repository-relative Git path"
    );
    let tree = rev.tree();
    check_oid(tree)?;
    let abbrev = format!("--abbrev={}", tree.len());
    let args = [
        "--literal-pathspecs",
        "ls-tree",
        "--full-tree",
        "-z",
        &abbrev,
        tree,
        "--",
        path,
    ];
    let bytes = bounded_output(repo, &args, None, path.len() + 128)?;
    let mut entries = records(&bytes)?;
    let Some(entry) = entries.next() else {
        return Ok(None);
    };
    let mut fields = entry.splitn(2, |&byte| byte == b'\t');
    let meta = fields.next().context("missing Git tree metadata")?;
    ensure!(
        entries.next().is_none() && fields.next() == Some(path.as_bytes()),
        "unexpected Git tree path"
    );
    let mut meta = std::str::from_utf8(meta)?.split_ascii_whitespace();
    let mode = meta.next().context("missing Git tree mode")?;
    let kind = meta.next().context("missing Git tree kind")?;
    let blob = meta.next().context("missing Git tree object ID")?;
    check_oid(blob)?;
    ensure!(
        meta.next().is_none() && blob.len() == tree.len(),
        "unexpected Git tree metadata"
    );
    Ok(
        (matches!(mode, "100644" | "100755") && kind == "blob").then(|| CommittedFile {
            path: path.to_owned(),
            commit: rev.commit().to_owned(),
            blob: blob.to_owned(),
        }),
    )
}

impl CommittedFile {
    /// Read a one-based inclusive span, or leading lines when no span is supplied.
    /// At most 1 MiB of blob bytes and 200 excerpt lines are accepted, and the
    /// displayed `lines` JSON array is bounded to 4 KiB even with escaping.
    pub fn excerpt(&self, repo: &Path, span: Option<(u64, u64)>) -> Result<CommittedExcerpt> {
        let (first, last) = span.unwrap_or((1, u64::MAX));
        if first == 0 || last < first {
            return Ok(CommittedExcerpt::Missing);
        }
        let Ok(bytes) = self.read_blob(repo) else {
            return Ok(CommittedExcerpt::Unavailable);
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Ok(CommittedExcerpt::Unavailable);
        };
        if bytes.contains(&0) {
            return Ok(CommittedExcerpt::Unavailable);
        }
        if first > text.len() as u64 {
            return Ok(CommittedExcerpt::Missing);
        }

        let mut lines = Vec::new();
        let mut remaining = MAX_JSON_LINES - 2; // Array brackets.
        let mut truncated = false;
        let requested = last - first + 1;
        for line in text.lines().skip((first - 1) as usize) {
            if lines.len() as u64 == requested {
                break;
            }
            // Two quotes and (conservatively) one comma per line.
            if lines.len() == MAX_LINES || remaining < 3 {
                truncated = true;
                break;
            }
            remaining -= 3;
            let mut end = 0;
            for ch in line.chars() {
                let cost = match ch {
                    '"' | '\\' => 2,
                    '\0'..='\u{001f}' => 6,
                    _ => ch.len_utf8(),
                };
                if cost > remaining {
                    break;
                }
                remaining -= cost;
                end += ch.len_utf8();
            }
            if end != 0 || line.is_empty() {
                lines.push(line[..end].to_owned());
            }
            if end != line.len() {
                truncated = true;
                break;
            }
        }
        if lines.is_empty() {
            return Ok(CommittedExcerpt::Missing);
        }
        Ok(CommittedExcerpt::Resolved {
            truncated: truncated || (span.is_some() && (lines.len() as u64) < requested),
            excerpt: SourceExcerpt {
                path: self.path.clone(),
                commit: self.commit.clone(),
                line: first,
                first,
                lines,
            },
        })
    }

    fn read_blob(&self, repo: &Path) -> Result<Vec<u8>> {
        // Ask for type and size before asking for contents, so Git is not told
        // to emit an oversized blob. Only a full object ID enters this protocol.
        let request = format!("{}\n", self.blob);
        let info = bounded_output(repo, &["cat-file", "--batch-check"], Some(&request), 128)?;
        let info = std::str::from_utf8(&info)?;
        let mut fields = info.split_ascii_whitespace();
        ensure!(
            fields.next() == Some(self.blob.as_str()) && fields.next() == Some("blob"),
            "Git object is not a readable blob"
        );
        let size = fields
            .next()
            .context("missing Git blob size")?
            .parse::<usize>()?;
        ensure!(
            fields.next().is_none() && size <= MAX_BLOB,
            "Git blob exceeds the size limit or returned invalid metadata"
        );
        let bytes = bounded_output(repo, &["cat-file", "blob", &self.blob], None, size)?;
        ensure!(bytes.len() == size, "incomplete Git blob");
        Ok(bytes)
    }
}

fn bounded_output(repo: &Path, args: &[&str], input: Option<&str>, cap: usize) -> Result<Vec<u8>> {
    let mut git = command(repo);
    if input.is_some() {
        git.stdin(Stdio::piped());
    }
    let mut child = git
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning git in {}: {args:?}", repo.display()))?;
    let result = (|| {
        if let Some(input) = input {
            // One small request fits in the pipe; drop stdin to signal EOF.
            child
                .stdin
                .take()
                .context("Git stdin")?
                .write_all(input.as_bytes())?;
        }
        let mut bytes = Vec::with_capacity(cap);
        let mut stdout = child.stdout.take().context("Git stdout")?;
        (&mut stdout).take(cap as u64).read_to_end(&mut bytes)?;
        if bytes.len() == cap {
            ensure!(
                stdout.read(&mut [0])? == 0,
                "Git output exceeds the size limit"
            );
        }
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().context("waiting for Git")?;
    let bytes = result?;
    ensure!(
        status.success(),
        "git in {} {args:?} failed ({status})",
        repo.display()
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::git::run;
    use crate::resolve_revision;

    fn fixture_git(repo: &Path, args: &[&str]) {
        let mut git = command(repo);
        git.env_remove("GIT_INDEX_FILE");
        run(git, repo, args).unwrap();
    }

    fn repo(object_format: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(
            dir.path(),
            &[
                "init",
                "--template=",
                "-q",
                &format!("--object-format={object_format}"),
            ],
        );
        fixture_git(dir.path(), &["config", "user.name", "Fixture"]);
        fixture_git(
            dir.path(),
            &["config", "user.email", "fixture@example.test"],
        );
        dir
    }

    fn commit(repo: &Path) -> Revision {
        fixture_git(repo, &["commit", "-qm", "fixture"]);
        resolve_revision(repo, "HEAD").unwrap()
    }

    #[test]
    fn exact_root_path_and_commit_ignore_checkout_and_replace_refs() {
        let dir = repo("sha1");
        let repo = dir.path();
        let subdir = repo.join("nested");
        fs::create_dir(&subdir).unwrap();
        fs::write(repo.join("source"), "zero\r\none\r\n\r\nthree").unwrap();
        fs::write(subdir.join("source"), "decoy\n").unwrap();
        fs::write(subdir.join("[source]"), "literal\n").unwrap();
        fixture_git(repo, &["add", "--all"]);
        let first = commit(repo);
        fixture_git(repo, &["config", "core.abbrev", "4"]);
        assert!(
            committed_file(&subdir, &first, "nested/[source]")
                .unwrap()
                .is_some()
        );
        fs::write(repo.join("source"), "new head\n").unwrap();
        fixture_git(repo, &["add", "source"]);
        let head = commit(repo);
        let replacement = committed_file(repo, &head, "source").unwrap().unwrap();
        let file = committed_file(&subdir, &first, "source").unwrap().unwrap();
        fixture_git(repo, &["replace", &file.blob, &replacement.blob]);
        fs::write(repo.join("source"), "dirty\n").unwrap();
        assert_eq!(
            file.excerpt(repo, Some((2, 3))).unwrap(),
            CommittedExcerpt::Resolved {
                excerpt: SourceExcerpt {
                    path: "source".into(),
                    commit: first.commit().into(),
                    line: 2,
                    first: 2,
                    lines: vec!["one".into(), "".into()],
                },
                truncated: false,
            }
        );
        assert!(committed_file(repo, &first, "sourc*").unwrap().is_none());
        assert!(
            committed_file(repo, &first, ":(glob)source")
                .unwrap()
                .is_none()
        );
        assert!(committed_file(repo, &first, "nested").unwrap().is_none());
        for path in ["", "/source", "./source", "nested/../source", "x\0"] {
            assert!(committed_file(repo, &first, path).is_err(), "{path:?}");
        }
    }

    #[test]
    fn only_regular_and_executable_entries_count() {
        let dir = repo("sha256");
        let repo = dir.path();
        fs::write(repo.join("target"), "contents").unwrap();
        fixture_git(repo, &["add", "target"]);
        let base = commit(repo);
        let id = committed_file(repo, &base, "target").unwrap().unwrap().blob;
        for entry in [
            format!("120000,{id},symlink"),
            format!("160000,{},gitlink", base.commit()),
            format!("100755,{id},executable"),
            format!("100644,{id},--option"),
        ] {
            fixture_git(repo, &["update-index", "--add", "--cacheinfo", &entry]);
        }
        let rev = commit(repo);
        for path in [
            "symlink",
            "gitlink",
            "symlink/target",
            "gitlink/target",
            "missing",
        ] {
            assert!(
                committed_file(repo, &rev, path).unwrap().is_none(),
                "{path}"
            );
        }
        let file = committed_file(repo, &rev, "executable").unwrap().unwrap();
        assert_eq!(file.blob.len(), 64);
        assert!(matches!(
            file.excerpt(repo, None).unwrap(),
            CommittedExcerpt::Resolved {
                truncated: false,
                ..
            }
        ));
        assert!(committed_file(repo, &rev, "--option").unwrap().is_some());
    }

    #[test]
    fn missing_unavailable_and_truncated_excerpts_are_distinct() {
        let dir = repo("sha1");
        let repo = dir.path();
        let escaped = "é\u{001f}\"\\".repeat(2048);
        for (path, bytes) in [
            ("empty", Vec::new()),
            ("nul", b"text\0here".to_vec()),
            ("invalid-utf8", vec![0xff]),
            ("large", vec![b'a'; MAX_BLOB + 1]),
            ("at-limit", vec![b'b'; MAX_BLOB]),
            ("escaped", format!("{escaped}\ntail\n").into_bytes()),
            ("many", "x\n".repeat(MAX_LINES + 1).into_bytes()),
        ] {
            fs::write(repo.join(path), bytes).unwrap();
        }
        fixture_git(repo, &["add", "--all"]);
        let rev = commit(repo);
        let get = |path| committed_file(repo, &rev, path).unwrap().unwrap();
        for path in ["nul", "invalid-utf8", "large"] {
            assert_eq!(
                get(path).excerpt(repo, None).unwrap(),
                CommittedExcerpt::Unavailable
            );
        }
        assert_eq!(
            get("empty").excerpt(repo, None).unwrap(),
            CommittedExcerpt::Missing
        );
        let file = get("many");
        assert_eq!(
            file.excerpt(repo, Some((500, 502))).unwrap(),
            CommittedExcerpt::Missing
        );
        let CommittedExcerpt::Resolved { excerpt, truncated } = file.excerpt(repo, None).unwrap()
        else {
            panic!("expected leading lines");
        };
        assert!(truncated);
        assert_eq!(excerpt.lines.len(), MAX_LINES);
        for path in ["at-limit", "escaped"] {
            let CommittedExcerpt::Resolved { excerpt, truncated } =
                get(path).excerpt(repo, None).unwrap()
            else {
                panic!("expected bounded prefix");
            };
            assert!(truncated);
            assert_eq!(excerpt.lines.len(), 1);
            assert!(serde_json::to_vec(&excerpt.lines).unwrap().len() <= MAX_JSON_LINES);
            if path == "escaped" {
                assert!(escaped.starts_with(&excerpt.lines[0]));
            }
        }
        let file = get("many");
        let (prefix, tail) = file.blob.split_at(2);
        fs::remove_file(repo.join(".git/objects").join(prefix).join(tail)).unwrap();
        assert!(committed_file(repo, &rev, "many").unwrap().is_some());
        assert_eq!(
            file.excerpt(repo, None).unwrap(),
            CommittedExcerpt::Unavailable
        );
    }
}
