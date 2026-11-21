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
    let (id, path, filter) = match &query.target {
        DetailRef::SessionFiles { id, path, filter } => {
            (id, path, Some(filter.as_deref().unwrap_or("")))
        }
        DetailRef::SessionFile { id, path } => (id, path, None),
        _ => return Ok(None),
    };
    ensure!(
        query.page.is_none(),
        "invalid: worktree previews do not paginate"
    );
    let body = platform_read(shared, engine, id, path, filter)?;
    let etag = fingerprint(&(&query.target, &body))?;
    details::reply(query, engine.store.latest_seq()?, etag, body, None).map(Some)
}

#[cfg(not(target_os = "linux"))]
fn platform_read(_: &Shared, _: &Engine, _: &str, _: &str, _: Option<&str>) -> Result<DetailBody> {
    anyhow::bail!("invalid: safe worktree reads are not supported on this daemon platform")
}

#[cfg(target_os = "linux")]
fn platform_read(
    shared: &Shared,
    engine: &Engine,
    id: &str,
    path: &str,
    filter: Option<&str>,
) -> Result<DetailBody> {
    linux::read(shared, engine, id, path, filter)
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        fs::File,
        io::Read,
        os::fd::{AsRawFd, OwnedFd},
        path::{Component, Path, PathBuf},
    };

    use anyhow::{Context, bail};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use corral_protocol::detail::{
        SessionFile, SessionFileEntry, SessionFileKind as Kind, SessionFiles,
    };
    use rustix::fs::{Dir, FileType, Mode, OFlags, ResolveFlags, fstat, open, openat2};
    use serde::Deserialize;

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

    pub(super) fn read(
        shared: &Shared,
        engine: &Engine,
        id: &str,
        path: &str,
        filter: Option<&str>,
    ) -> Result<DetailBody> {
        ensure!(
            path.len() <= PATH_BYTES
                && (filter.is_some() && path.is_empty() || path.split('/').all(visible)),
            "invalid: expected a visible worktree-relative path"
        );
        ensure!(
            filter.is_none_or(|value| value.len() <= 128 && !value.chars().any(char::is_control)),
            "invalid: search filter is too long or contains control characters"
        );
        let checkout = worktree(shared, engine, id)?;
        let repo = checkout
            .parent()
            .and_then(Path::parent)
            .context(NotFound("session worktree is unavailable"))?;
        let local = checkout.strip_prefix(repo)?;
        let root = openat2(root(repo)?, local, DIR, Mode::empty(), CONFINED)
            .map_err(|_| NotFound("session worktree is unavailable"))?;
        if let Some(filter) = filter {
            list(&root, path, filter).map(DetailBody::SessionFiles)
        } else {
            preview(&root, path).map(DetailBody::SessionFile)
        }
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

    fn regular(root: &OwnedFd, path: &str) -> Result<(File, u64)> {
        let missing = NotFound("worktree file is missing or unsafe");
        // O_PATH classifies the pinned inode before any device or FIFO can be opened for I/O.
        let pinned = openat2(
            root,
            path,
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        )
        .map_err(|_| missing)?;
        let stat = fstat(&pinned)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
            bail!(missing);
        }
        let file =
            File::open(format!("/proc/self/fd/{}", pinned.as_raw_fd())).map_err(|_| missing)?;
        // If the inode was unlinked/replaced during acquisition, refuse rather than read it.
        ensure!(
            file.metadata()?.is_file() && fstat(&file)?.st_nlink == 1,
            missing
        );
        Ok((file, u64::try_from(stat.st_size).unwrap_or(0)))
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
