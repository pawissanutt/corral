//! Local, read-only worktree views; no shell, Git command, host RPC or static file routes.
use anyhow::{Result, ensure};
use corral_engine::Engine;
use corral_protocol::detail::{DetailBody, DetailGet, DetailRef, DetailReply};

use super::{details, fingerprint};
use crate::state::Shared;

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
    const PATH_BYTES: usize = 1024;
    const SCAN: usize = 4096;
    const ROWS: usize = 200;
    const LIST_BYTES: usize = 40 * 1024;
    const DIR: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::CLOEXEC);
    const CONFINED: ResolveFlags = ResolveFlags::BENEATH
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

    fn visible(name: &str) -> bool {
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
                revision.is_none_or(
                    |value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
                ),
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
            let (file, _) = self.stale_if_missing(regular(root, self.path))?;
            let before = fstat(&file)?;
            let revision = self.stamp(&before);
            ensure!(before.st_nlink == 1, StaleWorktreeFile);
            ensure!(
                self.revision.is_none_or(|value| value == revision),
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
            if let Err(err) = file.read_exact_at(&mut data, self.offset) {
                if err.kind() == ErrorKind::UnexpectedEof {
                    bail!(StaleWorktreeFile);
                }
                return Err(err).context("read worktree HTML chunk");
            }
            // Check both the read descriptor and a fresh, confined lookup. The latter catches
            // a renamed parent or replaced path even if the original inode is still linked.
            ensure!(self.stamp(&fstat(&file)?) == revision, StaleWorktreeFile);
            let path_stat = match pin_regular(root, self.path) {
                Ok((_, stat)) => stat,
                Err(err) if err.is::<NotFound>() => bail!(StaleWorktreeFile),
                Err(err) => return Err(err),
            };
            ensure!(self.stamp(&path_stat) == revision, StaleWorktreeFile);
            ensure!(
                !data.contains(&0),
                "invalid: worktree HTML contains binary data"
            );
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
        use super::*;
        use std::{fs, os::unix::fs::symlink};

        #[test]
        fn browse_untracked_without_following_links_or_listing_metadata() {
            let tmp = tempfile::tempdir().unwrap();
            let worktree = tmp.path().join("checkout");
            fs::create_dir(&worktree).unwrap();
            fs::write(worktree.join("notes.md"), "notes").unwrap();
            fs::write(worktree.join(".git"), "gitdir: elsewhere").unwrap();
            let external = tmp.path().join("outside");
            fs::write(&external, "private").unwrap();
            symlink(&external, worktree.join("symlink")).unwrap();
            symlink(tmp.path(), worktree.join("symlink-dir")).unwrap();
            fs::hard_link(&external, worktree.join("hardlink")).unwrap();
            rustix::fs::mknodat(
                rustix::fs::CWD,
                worktree.join("fifo"),
                FileType::Fifo,
                Mode::RUSR,
                0,
            )
            .unwrap();
            let fd = root(&worktree).unwrap();
            let listed = list(&fd, "", "").unwrap();
            let filtered = list(&fd, "", "NOTES").unwrap();
            assert_eq!(filtered.entries[0].name, "notes.md");
            assert_eq!(filtered.entries.len(), 1);
            let names: Vec<_> = listed.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, ["hardlink", "notes.md"]);
            assert_eq!(preview(&fd, "notes.md").unwrap().kind, Kind::Markdown);
            for path in [
                "../outside",
                "hardlink",
                "symlink",
                "symlink-dir/outside",
                "fifo",
            ] {
                assert!(preview(&fd, path).is_err(), "{path}");
            }
            assert!(root(&worktree.join("symlink-dir")).is_err());
            for name in [".git", "..", ".corral-next", "a\nb", "a/b"] {
                assert!(!visible(name));
            }
        }

        #[test]
        fn sparse_sixty_four_mib_html_stays_within_control_frames() {
            use crate::{
                app::dispatch,
                conn::{Conn, Peer},
                tests::Fixture,
                topics::Subscriptions,
            };
            use corral_protocol::{
                self as wire, app,
                frame::{CONTROL_MAX, Frame, FrameKind},
                rpc,
            };
            use serde_json::json;

            #[derive(Default)]
            struct Capture(Option<Frame>);
            impl Conn for Capture {
                fn send_frame(&mut self, frame: &Frame) -> Result<()> {
                    assert!(self.0.replace(frame.clone()).is_none());
                    Ok(())
                }
            }

            let fixture = Fixture::new();
            let repo = fixture.root().canonicalize().unwrap();
            let worktree = repo.join(".worktrees/build-walk");
            let record = repo.join("pi-data/build/B-1");
            fs::create_dir_all(&worktree).unwrap();
            fs::create_dir_all(&record).unwrap();
            fs::write(
                record.join("session.json"),
                json!({
                    "id": "B-1", "name": "walk", "workspaceRoot": repo, "repoRoot": repo,
                    "worktree": worktree, "here": false,
                })
                .to_string(),
            )
            .unwrap();
            let file = File::create(worktree.join("prototype.HTML")).unwrap();
            file.set_len(SESSION_HTML_MAX_BYTES).unwrap();
            let last = SESSION_HTML_MAX_BYTES - SESSION_HTML_CHUNK_BYTES as u64;
            let mut bytes = vec![b'a'; SESSION_HTML_CHUNK_BYTES];
            file.write_all_at(&bytes, 0).unwrap();
            bytes.fill(b'z');
            file.write_all_at(&bytes, last).unwrap();

            let shared = fixture.shared();
            let caps = vec![app::SESSION_FILES_V1.into(), app::SESSION_HTML_V1.into()];
            let welcome = dispatch::welcome(
                shared,
                wire::Hello {
                    proto: wire::Proto {
                        major: wire::PROTO_MAJOR,
                        minor: wire::PROTO_MINOR,
                    },
                    caps: caps.clone(),
                    client: wire::ClientInfo {
                        kind: "test".into(),
                        version: "1".into(),
                    },
                },
            );
            assert_eq!(welcome.caps, caps);
            let call = |peer: Peer, target: DetailRef| -> Result<DetailReply, rpc::Error> {
                let mut capture = Capture::default();
                dispatch::serve(
                    &mut capture,
                    &peer,
                    shared,
                    rpc::Request {
                        jsonrpc: rpc::JSONRPC.into(),
                        id: json!(1),
                        method: app::DETAIL_GET.into(),
                        params: serde_json::to_value(DetailGet {
                            target,
                            etag: None,
                            page: None,
                        })
                        .unwrap(),
                    },
                    None,
                    &mut Subscriptions::new(shared),
                )
                .unwrap();
                let frame = capture.0.unwrap();
                assert_eq!(frame.kind, FrameKind::Control);
                assert!(frame.payload.len() <= CONTROL_MAX.min(app::APP_CONTROL_MAX));
                let response: rpc::Response<DetailReply> =
                    serde_json::from_slice(&frame.payload).unwrap();
                match response.outcome {
                    rpc::ResponseOutcome::Result { result } => Ok(result),
                    rpc::ResponseOutcome::Error { error } => Err(error),
                }
            };
            let path = "prototype.HTML";
            let select = |offset, revision| DetailRef::SessionFileChunk {
                id: "B-1".into(),
                path: path.into(),
                offset,
                revision,
            };
            assert_eq!(
                call(Peer::Tcp("127.0.0.1:1".parse().unwrap()), select(0, None))
                    .unwrap_err()
                    .code,
                rpc::ErrorCode::Unauthenticated
            );
            let old = call(
                Peer::Unix,
                DetailRef::SessionFile {
                    id: "B-1".into(),
                    path: path.into(),
                },
            )
            .unwrap();
            let Some(DetailBody::SessionFile(old)) = old.body else {
                panic!("expected preview")
            };
            assert_eq!(old.kind, Kind::TooLarge);
            assert!(old.data.is_empty());
            assert_eq!(old.bytes, SESSION_HTML_MAX_BYTES);

            let first = call(Peer::Unix, select(0, None)).unwrap();
            assert!(first.next.is_none());
            let Some(DetailBody::SessionFileChunk(first)) = first.body else {
                panic!("expected chunk")
            };
            assert_eq!(first.offset, 0);
            assert_eq!(first.size, SESSION_HTML_MAX_BYTES);
            assert_eq!(
                STANDARD.decode(first.data).unwrap(),
                vec![b'a'; SESSION_HTML_CHUNK_BYTES]
            );
            let tail = call(Peer::Unix, select(last, Some(first.revision.clone()))).unwrap();
            let Some(DetailBody::SessionFileChunk(tail)) = tail.body else {
                panic!("expected chunk")
            };
            assert_eq!(tail.offset, last);
            assert_eq!(tail.size, first.size);
            assert_eq!(tail.revision, first.revision);
            assert_eq!(STANDARD.decode(tail.data).unwrap(), bytes);
            assert_eq!(
                call(Peer::Unix, select(last, None)).unwrap_err().code,
                rpc::ErrorCode::InvalidParams
            );
            assert_eq!(
                call(Peer::Unix, select(1, Some(first.revision.clone())))
                    .unwrap_err()
                    .code,
                rpc::ErrorCode::InvalidParams
            );

            file.set_len(SESSION_HTML_MAX_BYTES + 1).unwrap();
            assert_eq!(
                call(Peer::Unix, select(last, Some(first.revision)))
                    .unwrap_err()
                    .code,
                rpc::ErrorCode::Conflict
            );
            assert_eq!(
                call(Peer::Unix, select(0, None)).unwrap_err().code,
                rpc::ErrorCode::TooLarge
            );
        }

        #[test]
        fn chunk_reads_keep_the_same_path_confinement() {
            let tmp = tempfile::tempdir().unwrap();
            let checkout = tmp.path().join("checkout");
            fs::create_dir(&checkout).unwrap();
            let external = tmp.path().join("external.html");
            fs::write(&external, "private").unwrap();
            fs::hard_link(&external, checkout.join("hard.html")).unwrap();
            symlink(&external, checkout.join("link.html")).unwrap();
            symlink(tmp.path(), checkout.join("link-dir")).unwrap();
            rustix::fs::mknodat(
                rustix::fs::CWD,
                checkout.join("fifo.html"),
                FileType::Fifo,
                Mode::RUSR,
                0,
            )
            .unwrap();
            fs::write(checkout.join("binary.html"), b"\0not HTML").unwrap();
            let fd = root(&checkout).unwrap();
            for path in ["../external.html", ".git/config.html", "notes.md", ""] {
                assert!(ChunkRequest::new("B-1", path, 0, None).is_err(), "{path}");
            }
            for path in [
                "hard.html",
                "link.html",
                "link-dir/external.html",
                "fifo.html",
                "binary.html",
            ] {
                assert!(
                    ChunkRequest::new("B-1", path, 0, None)
                        .unwrap()
                        .read(&fd)
                        .is_err(),
                    "{path}"
                );
            }
        }

        #[test]
        fn chunk_revisions_reject_same_length_replacement_and_wrong_selector() {
            let tmp = tempfile::tempdir().unwrap();
            let fd = root(tmp.path()).unwrap();
            let path = "prototype.htm";
            fs::write(tmp.path().join(path), b"<h1>one</h1>").unwrap();
            fs::write(tmp.path().join("replacement.htm"), b"<h1>two</h1>").unwrap();
            let first = ChunkRequest::new("B-1", path, 0, None)
                .unwrap()
                .read(&fd)
                .unwrap();
            let read = |id, path| {
                ChunkRequest::new(id, path, 0, Some(&first.revision))
                    .unwrap()
                    .read(&fd)
            };
            assert!(read("B-2", path).unwrap_err().is::<StaleWorktreeFile>());
            assert!(
                read("B-1", "replacement.htm")
                    .unwrap_err()
                    .is::<StaleWorktreeFile>()
            );
            fs::rename(tmp.path().join("replacement.htm"), tmp.path().join(path)).unwrap();
            assert!(read("B-1", path).unwrap_err().is::<StaleWorktreeFile>());
            fs::remove_file(tmp.path().join(path)).unwrap();
            assert!(read("B-1", path).unwrap_err().is::<StaleWorktreeFile>());
        }

        #[test]
        fn opaque_binary_and_oversize_content_never_get_payloads() {
            let tmp = tempfile::tempdir().unwrap();
            fs::write(tmp.path().join("big.html"), vec![b'a'; BYTES + 1]).unwrap();
            fs::write(tmp.path().join("opaque.png"), b"\0not png").unwrap();
            let fd = root(tmp.path()).unwrap();
            for (path, kind) in [("big.html", Kind::TooLarge), ("opaque.png", Kind::Binary)] {
                let served = preview(&fd, path).unwrap();
                assert_eq!(served.kind, kind);
                assert!(served.data.is_empty());
            }
        }
    }
}
