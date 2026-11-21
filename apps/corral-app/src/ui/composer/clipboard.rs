//! NSH 0.6.1's terminal host only reads text. Query an explicitly local Linux desktop for
//! actual clipboard formats, never treating plain text as a file list.

use std::env;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use corral_protocol::attachment::{
    ATTACHMENT_IMAGE_MAX, ATTACHMENT_TEXT_MAX, ATTACHMENTS_PER_MESSAGE,
};
use nsh_core::{PasteEvent, PastedFile};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;

const DEADLINE: Duration = Duration::from_millis(500);
const MIME_LIMIT: usize = 16 * 1024;
const URI_LIMIT: usize = 16 * 1024;
pub(super) const TEXT_LIMIT: usize = ATTACHMENT_TEXT_MAX as usize;
const IMAGE_LIMIT: usize = ATTACHMENT_IMAGE_MAX as usize;
const URI_LIST: &str = "text/uri-list";
const GNOME_FILES: &str = "x-special/gnome-copied-files";
const TEXT_TYPES: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain;charset=UTF-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];
const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

#[derive(Clone)]
pub(super) enum Helper {
    Wayland(PathBuf),
    Xclip(PathBuf),
    Xsel(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadError {
    Timeout,
    TooLarge,
}

fn executable(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?).find_map(|dir| {
        // Use an actual PATH entry, not an accidental program in the worktree.
        if !dir.is_absolute() {
            return None;
        }
        let path = dir.join(name);
        let meta = path.metadata().ok()?;
        (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then_some(path)
    })
}

fn local_x11(display: &str) -> bool {
    let Some((transport, number)) = display.split_once(':') else {
        return false;
    };
    let number = number.split('.').next().unwrap_or_default();
    matches!(transport, "" | "unix" | "unix/")
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// No extra shortcut when the host has no safe display reader. NSH keeps its native text
/// shortcut in this case (including its OSC 52 clipboard cache over SSH).
pub(super) fn helpers() -> Vec<Helper> {
    if ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .iter()
        .any(|name| env::var_os(name).is_some())
    {
        return Vec::new();
    }
    let mut helpers = Vec::new();
    if env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty())
        && let Some(path) = executable("wl-paste")
    {
        helpers.push(Helper::Wayland(path));
    }
    if env::var("DISPLAY").is_ok_and(|display| local_x11(&display)) {
        if let Some(path) = executable("xclip") {
            helpers.push(Helper::Xclip(path));
        }
        if let Some(path) = executable("xsel") {
            helpers.push(Helper::Xsel(path));
        }
    }
    helpers
}

pub(super) fn read(helpers: Vec<Helper>) -> Result<Option<PasteEvent>, ReadError> {
    // Wayland and its X11 bridge (and all representations) share one budget.
    let deadline = Instant::now() + DEADLINE;
    for helper in helpers {
        if let Some(paste) = helper.read(deadline)? {
            return Ok(Some(paste));
        }
    }
    Ok(None)
}

impl Helper {
    fn read(&self, deadline: Instant) -> Result<Option<PasteEvent>, ReadError> {
        let targets = match self {
            Self::Wayland(path) => output(path, &["--list-types"], MIME_LIMIT, deadline)?,
            Self::Xclip(path) => output(
                path,
                &["-selection", "clipboard", "-out", "-target", "TARGETS"],
                MIME_LIMIT,
                deadline,
            )?,
            Self::Xsel(path) => {
                return output(path, &["--clipboard", "--output"], TEXT_LIMIT, deadline)
                    .map(|bytes| bytes.and_then(text_paste));
            }
        };
        let Some(targets) = targets.and_then(|bytes| String::from_utf8(bytes).ok()) else {
            return Ok(None);
        };
        read_formats(&targets, |mime, limit| match self {
            Self::Wayland(path) => output(path, &["--no-newline", "--type", mime], limit, deadline),
            Self::Xclip(path) => output(
                path,
                &["-selection", "clipboard", "-out", "-target", mime],
                limit,
                deadline,
            ),
            Self::Xsel(_) => Ok(None),
        })
    }
}

fn read_formats(
    targets: &str,
    mut get: impl FnMut(&str, usize) -> Result<Option<Vec<u8>>, ReadError>,
) -> Result<Option<PasteEvent>, ReadError> {
    let offered = |mime| targets.lines().any(|line| line == mime);
    for mime in [URI_LIST, GNOME_FILES] {
        if offered(mime)
            && let Some(bytes) = get(mime, URI_LIMIT)?
        {
            let uris = file_list(mime, &bytes);
            if !uris.is_empty() {
                let mut event = PasteEvent::default();
                event.uris = uris;
                return Ok(Some(event));
            }
        }
    }
    // Like the native composer, prefer real text when an office app copies an image of it too.
    for &mime in TEXT_TYPES {
        if offered(mime)
            && let Some(paste) = get(mime, TEXT_LIMIT)?.and_then(text_paste)
        {
            return Ok(Some(paste));
        }
    }
    for &mime in IMAGE_TYPES {
        if offered(mime)
            && let Some(bytes) = get(mime, IMAGE_LIMIT)?
            && !bytes.is_empty()
        {
            let mut event = PasteEvent::default();
            event.files.push(PastedFile::image(bytes));
            return Ok(Some(event));
        }
    }
    Ok(None)
}

fn text_paste(bytes: Vec<u8>) -> Option<PasteEvent> {
    let text = String::from_utf8(bytes).ok()?;
    if text.is_empty() {
        return None;
    }
    let mut event = PasteEvent::default();
    event.text = text;
    Some(event)
}

fn file_list(mime: &str, bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    if mime == GNOME_FILES && !matches!(lines.next(), Some("copy" | "cut")) {
        return Vec::new();
    }
    let mut uris = Vec::new();
    for uri in lines {
        if uri.contains(['\0', '?', '#']) || uri.contains("%00") {
            continue;
        }
        if let Some(path) = uri
            .get(..7)
            .filter(|scheme| scheme.eq_ignore_ascii_case("file://"))
            .map(|_| &uri[7..])
            .map(|rest| rest.strip_prefix("localhost").unwrap_or(rest))
            .filter(|path| path.starts_with('/') && !path.starts_with("//"))
        {
            let uri = format!("file://{path}");
            if !uris.contains(&uri) {
                uris.push(uri);
                if uris.len() == ATTACHMENTS_PER_MESSAGE {
                    break;
                }
            }
        }
    }
    uris
}

fn output(
    program: &std::path::Path,
    args: &[&str],
    limit: usize,
    deadline: Instant,
) -> Result<Option<Vec<u8>>, ReadError> {
    if Instant::now() >= deadline {
        return Err(ReadError::Timeout);
    }
    let Ok(mut child) = Command::new(program)
        .args(args)
        .env_remove("WAYLAND_SOCKET")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    else {
        return Ok(None);
    };
    let result = (|| {
        let Some(mut stdout) = child.stdout.take() else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(ReadError::Timeout);
            }
            let left = deadline.duration_since(now);
            let time = Timespec {
                tv_sec: left.as_secs() as i64,
                tv_nsec: left.subsec_nanos().into(),
            };
            match poll(&mut [PollFd::new(&stdout, PollFlags::IN)], Some(&time)) {
                Ok(0) => return Err(ReadError::Timeout),
                Err(Errno::INTR) => continue,
                Err(_) => return Ok(None),
                Ok(_) => {}
            }
            let want = buf.len().min(limit + 1 - bytes.len());
            match stdout.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(got) => {
                    bytes.extend_from_slice(&buf[..got]);
                    if bytes.len() > limit {
                        return Err(ReadError::TooLarge);
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Ok(None),
            }
        }
        // EOF is not proof the helper has exited. Reap it within the same deadline.
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(status.success().then_some(bytes)),
                Ok(None) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(ReadError::Timeout);
                    }
                    std::thread::sleep(left.min(Duration::from_millis(2)));
                }
                Err(_) => return Ok(None),
            }
        }
    })();
    if !matches!(&result, Ok(Some(_))) {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn pasted_path_text_is_not_a_file_but_a_file_manager_list_is() {
        let plain = b"file:///tmp/notes%20one.txt";
        let text = read_formats("UTF8_STRING\n", |_, _| Ok(Some(plain.to_vec())))
            .unwrap()
            .unwrap();
        assert_eq!(text.text.as_bytes(), plain);
        assert!(text.files.is_empty() && text.uris.is_empty());

        let file = read_formats("x-special/gnome-copied-files\nUTF8_STRING\n", |mime, _| {
            assert_eq!(mime, GNOME_FILES);
            Ok(Some(b"copy\r\nfile://remote/home/no\r\nfile://localhost/tmp/a%20b\r\nfile:///tmp/a%20b\n".to_vec()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(file.uris, ["file:///tmp/a%20b"]);
        assert!(file.text.is_empty() && file.files.is_empty());
    }

    #[test]
    fn encoded_image_is_read_as_bytes_and_not_as_text() {
        let png = b"\x89PNG\r\n\x1a\n";
        let image = read_formats("image/png\n", |mime, limit| {
            assert_eq!(mime, "image/png");
            assert_eq!(limit, IMAGE_LIMIT);
            Ok(Some(png.to_vec()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(image.files.as_slice(), &[PastedFile::image(png.to_vec())]);
        assert!(image.text.is_empty() && image.uris.is_empty());
    }

    #[test]
    fn a_clipboard_helper_cannot_exceed_the_byte_or_time_budget() {
        let sh = Path::new("/bin/sh");
        let until = || Instant::now() + DEADLINE;
        assert_eq!(
            output(sh, &["-c", "printf '1234'"], 3, until()),
            Err(ReadError::TooLarge)
        );
        // An owner that closes stdout but never exits still uses the same budget.
        assert_eq!(
            output(
                sh,
                &["-c", "exec 1>&-; exec sleep 10"],
                4,
                Instant::now() + Duration::from_millis(50)
            ),
            Err(ReadError::Timeout)
        );
    }

    #[test]
    fn an_x11_forwarding_address_is_not_a_local_display() {
        assert!(local_x11(":0"));
        assert!(local_x11("unix:1.0"));
        assert!(!local_x11("localhost:10.0"));
        assert!(!local_x11("example.net:0"));
    }
}
