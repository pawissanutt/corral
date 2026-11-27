//! Local, read-only worktree views; no shell, Git command, host RPC or static file routes.
use anyhow::{Result, ensure};
use corral_engine::Engine;
use corral_protocol::detail::{
    DetailBody, DetailGet, DetailRef, DetailReply, SESSION_FILE_SEARCH_LIMIT_MAX,
    SESSION_FILE_SEARCH_QUERY_MAX_BYTES,
};

use super::{details, fingerprint};
use crate::state::Shared;

#[cfg(target_os = "linux")]
#[path = "session_file_search.rs"]
mod search;

pub(super) fn read(
    shared: &Shared,
    engine: &Engine,
    query: &DetailGet,
) -> Result<Option<DetailReply>> {
    if !matches!(
        query.target,
        DetailRef::SessionFiles { .. }
            | DetailRef::SessionFile { .. }
            | DetailRef::SessionFileChunk { .. }
            | DetailRef::SessionFileSearch { .. }
    ) {
        return Ok(None);
    }
    ensure!(
        query.page.is_none(),
        "invalid: worktree previews do not paginate"
    );
    if matches!(query.target, DetailRef::SessionFileChunk { .. }) {
        ensure!(
            query.etag.is_none(),
            "invalid: chunks do not accept an etag"
        );
    }
    if let DetailRef::SessionFileSearch { query, limit, .. } = &query.target {
        ensure!(
            (1..=SESSION_FILE_SEARCH_LIMIT_MAX).contains(limit),
            "invalid: file search limit must be 1..={SESSION_FILE_SEARCH_LIMIT_MAX}"
        );
        ensure!(
            !query.is_empty()
                && query.len() <= SESSION_FILE_SEARCH_QUERY_MAX_BYTES
                && !query.chars().any(char::is_control),
            "invalid: file search query is empty, too long or contains control characters"
        );
    }
    let body = platform_read(shared, engine, &query.target)?;
    let etag = fingerprint(&(&query.target, &body))?;
    details::reply(query, engine.store.latest_seq()?, etag, body, None).map(Some)
}

#[derive(Debug)]
pub(crate) struct StaleWorktreeFile;

impl std::fmt::Display for StaleWorktreeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("worktree file changed; reopen it to start a new download")
    }
}

impl std::error::Error for StaleWorktreeFile {}

#[cfg(not(target_os = "linux"))]
fn platform_read(_: &Shared, _: &Engine, _: &DetailRef) -> Result<DetailBody> {
    anyhow::bail!("invalid: safe worktree reads are not supported on this daemon platform")
}

#[cfg(target_os = "linux")]
use linux::read as platform_read;

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        fs::File,
        io::{ErrorKind, Read},
        os::{
            fd::{AsRawFd, OwnedFd},
            unix::fs::FileExt,
        },
        path::{Component, Path, PathBuf},
        time::{Duration, Instant},
    };

    use anyhow::{Context, bail};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use corral_protocol::detail::{
        SESSION_HTML_CHUNK_BYTES, SESSION_HTML_MAX_BYTES, SessionFile, SessionFileChunk,
        SessionFileEntry, SessionFileKind as Kind, SessionFiles,
    };
    use rustix::fs::{Dir, FileType, Mode, OFlags, ResolveFlags, Stat, fstat, open, openat2};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::{
        sessions,
        views::{NotFound, json_len},
    };

    const BYTES: usize = 32 * 1024;
    pub(super) const PATH_BYTES: usize = 1024;
    const SCAN: usize = 4096;
    const ROWS: usize = 200;
    const LIST_BYTES: usize = 40 * 1024;
    pub(super) const DIR: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::CLOEXEC);
    pub(super) const CONFINED: ResolveFlags = ResolveFlags::BENEATH
        .union(ResolveFlags::NO_SYMLINKS)
        .union(ResolveFlags::NO_XDEV);

    pub(super) fn read(shared: &Shared, engine: &Engine, target: &DetailRef) -> Result<DetailBody> {
        match target {
            DetailRef::SessionFiles { id, path, filter } => {
                validate_path(path, true)?;
                let filter = filter.as_deref().unwrap_or("");
                ensure!(
                    filter.len() <= 128 && !filter.chars().any(char::is_control),
                    "invalid: search filter is too long or contains control characters"
                );
                list(&checkout_root(shared, engine, id)?, path, filter)
                    .map(DetailBody::SessionFiles)
            }
            DetailRef::SessionFile { id, path } => {
                validate_path(path, false)?;
                preview(&checkout_root(shared, engine, id)?, path).map(DetailBody::SessionFile)
            }
            DetailRef::SessionFileSearch { id, query, limit } => search::read(
                &checkout_root(shared, engine, id)?,
                query,
                usize::from(*limit),
            )
            .map(DetailBody::SessionFileSearch),
            DetailRef::SessionFileChunk {
                id,
                path,
                offset,
                revision,
            } => {
                let request = ChunkRequest::new(id, path, *offset, revision.as_deref())?;
                let root = request.stale_if_missing(checkout_root(shared, engine, id))?;
                request.read(&root).map(DetailBody::SessionFileChunk)
            }
            _ => unreachable!("non-worktree detail selector"),
        }
    }

    fn validate_path(path: &str, allow_root: bool) -> Result<()> {
        ensure!(
            path.len() <= PATH_BYTES
                && (allow_root && path.is_empty() || path.split('/').all(visible)),
            "invalid: expected a visible worktree-relative path"
        );
        Ok(())
    }

    fn checkout_root(shared: &Shared, engine: &Engine, id: &str) -> Result<OwnedFd> {
        let checkout = worktree(shared, engine, id)?;
        let missing = NotFound("session worktree is unavailable");
        let repo = checkout.parent().and_then(Path::parent).context(missing)?;
        let local = checkout.strip_prefix(repo)?;
        openat2(root(repo)?, local, DIR, Mode::empty(), CONFINED).map_err(|_| missing.into())
    }

    pub(super) fn visible(name: &str) -> bool {
        // Repository internals and daemon state are never served, even by direct selector.
        !name.is_empty()
            && name.len() <= 255
            && !name.chars().any(char::is_control)
            && !name.contains(['/', '\\'])
            && ![
                ".",
                "..",
                ".git",
                ".hg",
                ".svn",
                ".corral",
                ".corral-next",
                ".worktrees",
            ]
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BuildRecord {
        id: String,
        name: String,
        workspace_root: PathBuf,
        repo_root: PathBuf,
        worktree: PathBuf,
        project: Option<String>,
        here: bool,
        #[serde(default)]
        bare: bool,
    }

    fn worktree(shared: &Shared, engine: &Engine, id: &str) -> Result<PathBuf> {
        ensure!(
            id.len() <= 128 && !sessions::is_chat(id) && !id.contains("/child/"),
            "invalid: this session has no dedicated worktree"
        );
        let missing = NotFound("session worktree is unavailable");
        if !id.starts_with("B-") {
            let run = sessions::run(engine, id)?;
            let (task_id, _) = sessions::run_target(id)?;
            ensure!(run.task == task_id, missing);
            let task = engine.store.get_task(task_id)?.ok_or(missing)?;
            let project = engine
                .config
                .effective_project(task.project.as_deref().ok_or(missing)?)
                .map_err(|_| missing)?;
            let repo = shared
                .root()
                .join(project.path)
                .canonicalize()
                .map_err(|_| missing)?;
            ensure!(
                run.worktree == repo.join(".worktrees").join(task_id),
                missing
            );
            return Ok(run.worktree);
        }
        ensure!(
            sessions::is_build(id),
            "invalid: expected a build session id"
        );
        let records = root(&shared.build_data_root()?)?;
        let (record, _) = regular(&records, &format!("build/{id}/session.json"))?;
        let mut data = Vec::new();
        record.take(64 * 1024 + 1).read_to_end(&mut data)?;
        ensure!(data.len() <= 64 * 1024, missing);
        let build: BuildRecord = serde_json::from_slice(&data).map_err(|_| missing)?;
        ensure!(
            !build.here
                && !build.bare
                && build.id == id
                && build.workspace_root.is_absolute()
                && build
                    .workspace_root
                    .canonicalize()
                    .is_ok_and(|root| root == shared.root())
                && (1..=40).contains(&build.name.len())
                && build
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            missing
        );
        // A record cannot choose an arbitrary repo or absolute root. Derive the sole allowed
        // build location from the daemon's trusted workspace/project configuration and its name.
        let repo = if let Some(project) = &build.project {
            let project = engine
                .config
                .effective_project(project)
                .map_err(|_| missing)?;
            shared.root().join(project.path)
        } else {
            shared.root().to_owned()
        };
        let repo = repo.canonicalize().map_err(|_| missing)?;
        let expected = repo
            .join(".worktrees")
            .join(format!("build-{}", build.name));
        ensure!(
            build.repo_root == repo && build.worktree == expected,
            missing
        );
        Ok(expected)
    }

    fn root(path: &Path) -> Result<OwnedFd> {
        let missing = NotFound("session worktree is unavailable");
        let relative = path.strip_prefix("/").map_err(|_| missing)?;
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            missing
        );
        let slash = open("/", DIR, Mode::empty())?;
        // Mounts on the way to the configured checkout are allowed. A browse selector cannot
        // cross a mount below the retained checkout descriptor, including bind mounts.
        openat2(
            &slash,
            relative,
            DIR,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| missing.into())
    }

    fn list(root: &OwnedFd, path: &str, filter: &str) -> Result<SessionFiles> {
        let filter = filter.to_lowercase();
        let fd = openat2(
            root,
            if path.is_empty() { "." } else { path },
            DIR,
            Mode::empty(),
            CONFINED,
        )
        .map_err(|_| NotFound("worktree directory is missing or unsafe"))?;
        let mut dir = Dir::new(fd)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        for index in 0..=SCAN {
            let Some(entry) = dir.read() else { break };
            if index == SCAN {
                truncated = true;
                break;
            }
            let entry = entry?;
            let Ok(name) = entry.file_name().to_str() else {
                continue;
            };
            if !visible(name)
                || path.len() + usize::from(!path.is_empty()) + name.len() > PATH_BYTES
                || !filter.is_empty() && !name.to_lowercase().contains(&filter)
            {
                continue;
            }
            let directory = match entry.file_type() {
                FileType::Directory => true,
                FileType::RegularFile => false,
                FileType::Unknown => {
                    // Some network filesystems do not populate d_type.
                    let stat = rustix::fs::statat(
                        dir.fd()?,
                        entry.file_name(),
                        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
                    );
                    match stat.map(|st| FileType::from_raw_mode(st.st_mode)) {
                        Ok(FileType::Directory) => true,
                        Ok(FileType::RegularFile) => false,
                        _ => continue,
                    }
                }
                _ => continue,
            };
            entries.push(SessionFileEntry {
                name: name.into(),
                directory,
            });
        }
        entries.sort_unstable_by(|a, b| (!a.directory, &a.name).cmp(&(!b.directory, &b.name)));
        let mut left = LIST_BYTES;
        let mut keep = 0;
        for entry in entries.iter().take(ROWS) {
            let need = json_len(entry)? + 1;
            if need > left {
                break;
            }
            left -= need;
            keep += 1;
        }
        truncated |= keep < entries.len();
        entries.truncate(keep);
        Ok(SessionFiles { entries, truncated })
    }

    const MISSING_FILE: NotFound = NotFound("worktree file is missing or unsafe");

    fn pin_regular(root: &OwnedFd, path: &str) -> Result<(OwnedFd, Stat)> {
        // O_PATH classifies the pinned inode before any device or FIFO can be opened for I/O.
        let pinned = openat2(
            root,
            path,
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        )
        .map_err(|_| MISSING_FILE)?;
        let stat = fstat(&pinned)?;
        ensure!(
            FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile && stat.st_nlink == 1,
            MISSING_FILE
        );
        Ok((pinned, stat))
    }

    fn regular(root: &OwnedFd, path: &str) -> Result<(File, u64)> {
        let (pinned, stat) = pin_regular(root, path)?;
        let file = File::open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
            .map_err(|_| MISSING_FILE)?;
        // If the inode was unlinked/replaced during acquisition, refuse rather than read it.
        ensure!(
            file.metadata()?.is_file() && fstat(&file)?.st_nlink == 1,
            MISSING_FILE
        );
        Ok((file, u64::try_from(stat.st_size).unwrap_or(0)))
    }

    const HTML_READ_BUDGET: Duration = Duration::from_secs(5);

    fn valid_revision(value: &str) -> bool {
        fn sha256_hex(s: &str) -> bool {
            s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
        value
            .split_once('.')
            .is_some_and(|(stamp, digest)| sha256_hex(stamp) && sha256_hex(digest))
    }

    fn check_html_deadline(deadline: Instant) -> Result<()> {
        ensure!(
            Instant::now() < deadline,
            "worktree HTML read exceeded its time budget"
        );
        Ok(())
    }

    fn read_html_bytes(
        file: &File,
        mut data: &mut [u8],
        mut offset: u64,
        deadline: Instant,
    ) -> Result<()> {
        while !data.is_empty() {
            // A regular-file syscall can itself block; do not start or retry one after the
            // deadline, and discard a result that arrives too late.
            check_html_deadline(deadline)?;
            match file.read_at(data, offset) {
                Ok(0) => bail!(StaleWorktreeFile),
                Ok(n) => {
                    offset += n as u64;
                    data = &mut data[n..];
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) => return Err(err).context("read worktree HTML chunk"),
            }
        }
        check_html_deadline(deadline)
    }

    fn html_digest(file: &File, size: u64, first: &[u8], deadline: Instant) -> Result<String> {
        // Hash the exact first chunk we will return, then stream the remainder once. This
        // binds the completed download to the byte stream observed in this pass, even if a
        // later write escapes the metadata stamp (e.g. through an already dirty mapping).
        // It does not prove the pass itself was an atomic snapshot while the file was edited.
        let mut hash = Sha256::new();
        hash.update(first);
        let mut offset = first.len() as u64;
        let mut scratch = [0; SESSION_HTML_CHUNK_BYTES];
        while offset < size {
            let len = (size - offset).min(scratch.len() as u64) as usize;
            read_html_bytes(file, &mut scratch[..len], offset, deadline)?;
            hash.update(&scratch[..len]);
            offset += len as u64;
        }
        check_html_deadline(deadline)?;
        Ok(format!("{:x}", hash.finalize()))
    }

    struct ChunkRequest<'a> {
        id: &'a str,
        path: &'a str,
        offset: u64,
        revision: Option<&'a str>,
    }

    impl<'a> ChunkRequest<'a> {
        fn new(id: &'a str, path: &'a str, offset: u64, revision: Option<&'a str>) -> Result<Self> {
            validate_path(path, false)?;
            ensure!(
                Path::new(path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(
                        |ext| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm")
                    ),
                "invalid: chunked reads are only supported for .html and .htm"
            );
            ensure!(
                offset < SESSION_HTML_MAX_BYTES
                    && offset.is_multiple_of(SESSION_HTML_CHUNK_BYTES as u64),
                "invalid: worktree HTML chunk offset is out of range or unaligned"
            );
            ensure!(
                offset == 0 || revision.is_some(),
                "invalid: missing worktree HTML revision"
            );
            ensure!(
                revision.is_none_or(valid_revision),
                "invalid: malformed worktree HTML revision"
            );
            Ok(Self {
                id,
                path,
                offset,
                revision,
            })
        }

        fn stale_if_missing<T>(&self, result: Result<T>) -> Result<T> {
            result.map_err(|err| {
                if self.revision.is_some() && err.is::<NotFound>() {
                    StaleWorktreeFile.into()
                } else {
                    err
                }
            })
        }

        // Bind continuations to their selector and the metadata of the pinned inode. ctime also
        // detects ordinary in-place edits with restored mtime. atime is intentionally ignored.
        fn stamp(&self, st: &Stat) -> String {
            let mut hash = Sha256::new();
            hash.update(self.id.as_bytes());
            hash.update([0]);
            hash.update(self.path.as_bytes());
            hash.update([0]);
            hash.update(st.st_dev.to_le_bytes());
            hash.update(st.st_ino.to_le_bytes());
            hash.update(st.st_size.to_le_bytes());
            hash.update(st.st_mtime.to_le_bytes());
            hash.update(st.st_mtime_nsec.to_le_bytes());
            hash.update(st.st_ctime.to_le_bytes());
            hash.update(st.st_ctime_nsec.to_le_bytes());
            hash.update(st.st_nlink.to_le_bytes());
            hash.update(st.st_mode.to_le_bytes());
            format!("{:x}", hash.finalize())
        }

        fn read(&self, root: &OwnedFd) -> Result<SessionFileChunk> {
            let deadline = Instant::now() + HTML_READ_BUDGET;
            let (file, _) = self.stale_if_missing(regular(root, self.path))?;
            let before = fstat(&file)?;
            let stamp = self.stamp(&before);
            ensure!(before.st_nlink == 1, StaleWorktreeFile);
            // Revisions have a fixed-width, validated stamp prefix. No content scan on resume.
            ensure!(
                self.revision.is_none_or(|value| value.starts_with(&stamp)),
                StaleWorktreeFile
            );
            let size =
                u64::try_from(before.st_size).context("invalid: negative worktree file size")?;
            ensure!(
                size <= SESSION_HTML_MAX_BYTES,
                "too_large: worktree HTML exceeds 64 MiB"
            );
            ensure!(
                self.offset < size || size == 0 && self.offset == 0,
                "invalid: worktree HTML chunk starts at or after EOF"
            );
            let len = (size - self.offset).min(SESSION_HTML_CHUNK_BYTES as u64) as usize;
            let mut data = vec![0; len];
            read_html_bytes(&file, &mut data, self.offset, deadline)?;
            ensure!(
                !data.contains(&0),
                "invalid: worktree HTML contains binary data"
            );
            let revision = match self.revision {
                Some(value) => value.to_owned(),
                None => format!("{stamp}.{}", html_digest(&file, size, &data, deadline)?),
            };
            // Check both the read descriptor and a fresh, confined lookup after all reads.
            // The latter catches a renamed parent or replaced path even if the inode is linked.
            ensure!(self.stamp(&fstat(&file)?) == stamp, StaleWorktreeFile);
            let path_stat = match pin_regular(root, self.path) {
                Ok((_, stat)) => stat,
                Err(err) if err.is::<NotFound>() => bail!(StaleWorktreeFile),
                Err(err) => return Err(err),
            };
            ensure!(self.stamp(&path_stat) == stamp, StaleWorktreeFile);
            check_html_deadline(deadline)?;
            Ok(SessionFileChunk {
                size,
                offset: self.offset,
                revision,
                data: STANDARD.encode(data),
            })
        }
    }

    fn preview(root: &OwnedFd, path: &str) -> Result<SessionFile> {
        let (file, len) = regular(root, path)?;
        if len > BYTES as u64 {
            return Ok(SessionFile {
                kind: Kind::TooLarge,
                data: String::new(),
                bytes: len,
            });
        }
        let mut data = Vec::new();
        file.take(BYTES as u64 + 1)
            .read_to_end(&mut data)
            .context("read worktree file")?;
        let bytes = data.len() as u64;
        let kind = if data.len() > BYTES {
            Kind::TooLarge
        } else {
            kind(path, &data)
        };
        let data = if matches!(kind, Kind::TooLarge | Kind::Binary) {
            String::new()
        } else {
            STANDARD.encode(data)
        };
        Ok(SessionFile { kind, data, bytes })
    }

    fn kind(path: &str, data: &[u8]) -> Kind {
        let extension = Path::new(path)
            .extension()
            .and_then(|part| part.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match extension.as_str() {
            "png"
                if data.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR")
                    && data
                        .get(16..24)
                        .is_some_and(|span| dimensions(number(&span[..4]), number(&span[4..])))
                    && !data.windows(4).any(|bytes| bytes == b"acTL") =>
            {
                Kind::Png
            }
            "jpg" | "jpeg" if jpeg_dimensions(data).is_some_and(|(w, h)| dimensions(w, h)) => {
                Kind::Jpeg
            }
            _ if !data.contains(&0) && std::str::from_utf8(data).is_ok() => {
                match extension.as_str() {
                    "md" | "markdown" => Kind::Markdown,
                    "html" | "htm" => Kind::Html,
                    _ => Kind::Text,
                }
            }
            _ => Kind::Binary,
        }
    }

    fn dimensions(w: u32, h: u32) -> bool {
        (1..=4096).contains(&w) && (1..=4096).contains(&h) && w * h <= 4_000_000
    }

    fn number(bytes: &[u8]) -> u32 {
        bytes.iter().fold(0, |n, b| n * 256 + u32::from(*b))
    }

    fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
        let mut segments = data.strip_prefix(b"\xff\xd8")?;
        while let Some(rest) = segments.strip_prefix(b"\xff") {
            let marker = *rest.first()?;
            if marker == 0xff {
                segments = rest;
                continue;
            }
            if matches!(marker, 0 | 0xd9 | 0xda) {
                return None;
            }
            let len = number(rest.get(1..3)?) as usize;
            if len < 2 {
                return None;
            }
            let payload = rest.get(3..1 + len)?;
            if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                return Some((number(payload.get(3..5)?), number(payload.get(1..3)?)));
            }
            segments = rest.get(1 + len..)?;
        }
        None
    }

    #[cfg(test)]
    mod tests {
        include!("session_files_tests.rs");
    }
}
