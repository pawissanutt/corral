//! Static responses from a dedicated, trusted Corral web bundle.
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags, open, openat, statat};
use rustix::io::Errno;

const INDEX: &str = "index.html";
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC);

pub(super) fn serve(
    out: &mut impl Write,
    root: &Path,
    request: &httparse::Request<'_, '_>,
) -> io::Result<()> {
    let Some(encoded) = request.path.and_then(|target| target.split('?').next()) else {
        return error(out, 400);
    };
    let Some(decoded) = decode_path(encoded) else {
        return error(out, 400);
    };
    let relative = decoded.trim_matches('/');
    if relative.split('/').next() == Some("v1") {
        return error(out, 404);
    }
    let head = match request.method {
        Some("GET") => false,
        Some("HEAD") => true,
        Some(_) => return error(out, 405),
        None => return error(out, 400),
    };

    // Open the configured directory anew: the deployment link may have moved.
    // Require a usable shell before exposing any files, and use the same root
    // handle for all lookups within the request.
    let Ok(root) = open(root, DIRECTORY, Mode::empty()) else {
        return error(out, 503);
    };
    let Ok(index) = open_asset(&root, INDEX) else {
        return error(out, 503);
    };
    let trailing_slash = decoded.ends_with('/');
    let (name, asset) = match relative {
        "" => (INDEX, index),
        INDEX if !trailing_slash => (INDEX, index),
        _ => match open_asset(&root, relative) {
            Ok(asset) if !trailing_slash => (relative, asset),
            Err(LookupError::Missing) if is_navigation(encoded, request) => (INDEX, index),
            Err(LookupError::Unavailable) => return error(out, 503),
            _ => return error(out, 404),
        },
    };
    write_head(out, 200, "OK", mime_type(name), asset.length)?;
    if !head && io::copy(&mut asset.file.take(asset.length), out)? != asset.length {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "static file truncated during response",
        ));
    }
    Ok(())
}

pub(super) fn error(out: &mut impl Write, status: u16) -> io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unknown HTTP error",
            ));
        }
    };
    write_head(out, status, reason, "text/plain; charset=utf-8", 0)
}

fn write_head(
    out: &mut impl Write,
    status: u16,
    reason: &str,
    content_type: &str,
    length: u64,
) -> io::Result<()> {
    let allow = if status == 405 {
        "Allow: GET, HEAD\r\n"
    } else {
        ""
    };
    write!(
        out,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {length}\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Vary: Accept\r\n\
         {allow}\r\n",
    )
}

fn decode_path(encoded: &str) -> Option<String> {
    if encoded.contains('#') {
        return None;
    }
    let mut rest = encoded.strip_prefix('/')?.as_bytes();
    let mut decoded = Vec::with_capacity(rest.len());
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        let byte = if byte == b'%' {
            let (hex, after) = rest.split_at_checked(2)?;
            rest = after;
            (hex_digit(hex[0])? << 4) | hex_digit(hex[1])?
        } else {
            byte
        };
        if byte.is_ascii_control() || byte == b'\\' {
            return None;
        }
        decoded.push(byte);
    }
    let decoded = String::from_utf8(decoded).ok()?;
    if decoded.split('/').any(|part| matches!(part, "." | "..")) {
        return None;
    }
    Some(decoded)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_navigation(encoded_path: &str, request: &httparse::Request<'_, '_>) -> bool {
    // The app percent-encodes dots in record IDs. Use the original URL when
    // deciding whether this missing path looked like an asset.
    let leaf = encoded_path.trim_end_matches('/').rsplit('/').next();
    !leaf.is_some_and(|part| part.contains('.'))
        && request
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("Accept"))
            .filter_map(|header| std::str::from_utf8(header.value).ok())
            .flat_map(|value| value.split(','))
            .any(|item| {
                let mut parts = item.split(';');
                parts
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("text/html"))
                    && parts.all(|param| {
                        let (key, value) = param.split_once('=').unwrap_or((param, ""));
                        !key.trim().eq_ignore_ascii_case("q")
                            || value
                                .trim()
                                .parse::<f64>()
                                .is_ok_and(|q| q > 0.0 && q <= 1.0)
                    })
            })
}

struct Asset {
    file: File,
    length: u64,
}

enum LookupError {
    Missing,
    NotAFile,
    Unavailable,
}

impl From<Errno> for LookupError {
    fn from(error: Errno) -> Self {
        match error {
            Errno::NOENT => Self::Missing,
            Errno::NOTDIR | Errno::LOOP | Errno::ISDIR | Errno::NAMETOOLONG | Errno::NXIO => {
                Self::NotAFile
            }
            _ => Self::Unavailable,
        }
    }
}

fn open_asset(root: &OwnedFd, relative: &str) -> Result<Asset, LookupError> {
    let mut dir = None;
    let mut parts = relative
        .split('/')
        .filter(|part| !part.is_empty())
        .peekable();
    while let Some(part) = parts.next() {
        let parent = dir.as_ref().unwrap_or(root);
        if parts.peek().is_none() {
            return open_regular(parent, part);
        }
        dir = Some(openat(
            parent,
            part,
            DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
    }
    Err(LookupError::NotAFile)
}

fn open_regular(dir: &OwnedFd, name: &str) -> Result<Asset, LookupError> {
    // Avoid opening known special files. Keep NOFOLLOW and NONBLOCK on open
    // and check the retained handle, in case the entry changed after statat.
    let stat = statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(LookupError::NotAFile);
    }
    let flags =
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY;
    let file = File::from(openat(dir, name, flags, Mode::empty())?);
    let meta = file.metadata().map_err(|_| LookupError::Unavailable)?;
    if !meta.is_file() {
        return Err(LookupError::NotAFile);
    }
    let length = meta.len();
    if length > MAX_FILE_BYTES {
        return Err(LookupError::Unavailable);
    }
    Ok(Asset { file, length })
}

fn mime_type(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("wasm") => "application/wasm",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("otf") => "font/otf",
        Some("ttf") => "font/ttf",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[path = "static_site_tests.rs"]
mod tests;
