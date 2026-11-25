//! Framing for the one HTTP response. Connection: close and EOF rule out extra responses
//! or trailing octets after an explicit end of body, including those in a later read.
use anyhow::{Context, Result, ensure};
use std::{borrow::Cow, collections::HashSet};

use super::sse::MAX_SSE;

const MAX_HEADERS: usize = 8_192;
const MAX_CHUNK_OVERHEAD: usize = 65_536;
pub(super) const MAX_WIRE: usize = MAX_HEADERS + MAX_SSE + MAX_CHUNK_OVERHEAD;

pub(super) fn body(raw: &[u8]) -> Result<Cow<'_, [u8]>> {
    let head_length = raw[..raw.len().min(MAX_HEADERS)]
        .windows(4)
        .position(|s| s == b"\r\n\r\n")
        .map(|n| n + 4)
        .context("fixture HTTP headers missing or too large")?;
    let (head, rest) = raw.split_at(head_length);
    ensure!(
        (head.starts_with(b"HTTP/1.1 ") || head.starts_with(b"HTTP/1.0 "))
            && head.split_inclusive(|b| *b == b'\n').all(|line| {
                line.strip_suffix(b"\r\n").is_some_and(|content| {
                    content
                        .iter()
                        .all(|&b| b == b'\t' || (b' '..=b'~').contains(&b))
                })
            }),
        "invalid fixture HTTP header framing"
    );

    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Response::new(&mut headers);
    ensure!(
        matches!(parsed.parse(head), Ok(httparse::Status::Complete(n)) if n == head_length)
            && matches!(parsed.version, Some(0 | 1))
            && parsed.code == Some(200),
        "invalid fixture HTTP response status or headers"
    );
    let mut seen = HashSet::new();
    let mut sse = false;
    let mut size = None;
    let mut chunked = false;
    for header in parsed.headers {
        let name = header.name.to_ascii_lowercase();
        ensure!(seen.insert(name.clone()), "duplicate fixture HTTP header");
        // ASCII was checked above. Trim only the grammar's optional whitespace.
        let value = std::str::from_utf8(header.value)?.trim_matches([' ', '\t']);
        match name.as_str() {
            "content-type" => {
                ensure!(
                    value.eq_ignore_ascii_case("text/event-stream"),
                    "invalid fixture HTTP content type"
                );
                sse = true;
            }
            "content-length" => {
                ensure!(
                    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
                    "invalid fixture HTTP length"
                );
                // Parse errors carry no peer bytes.
                size = Some(
                    value
                        .parse::<usize>()
                        .context("invalid fixture HTTP length")?,
                );
            }
            "transfer-encoding" => {
                ensure!(
                    parsed.version == Some(1) && value.eq_ignore_ascii_case("chunked"),
                    "unsupported fixture HTTP transfer encoding"
                );
                chunked = true;
            }
            "content-encoding" | "trailer" => {
                anyhow::bail!("unexpected fixture HTTP response encoding")
            }
            _ => (),
        }
    }
    ensure!(
        sse && !(chunked && size.is_some()),
        "missing SSE or conflicting fixture HTTP framing"
    );
    if chunked {
        return Ok(Cow::Owned(unchunk(rest)?));
    }
    ensure!(
        rest.len() <= MAX_SSE && size.is_none_or(|n| n == rest.len()),
        "invalid fixture HTTP body length"
    );
    Ok(Cow::Borrowed(rest))
}

fn unchunk(mut rest: &[u8]) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut overhead = 0;
    loop {
        // A size is at most six hex digits in this profile; no extensions or trailers.
        let end = rest[..rest.len().min(8)]
            .windows(2)
            .position(|s| s == b"\r\n")
            .context("invalid fixture HTTP chunk size line")?;
        let size = &rest[..end];
        ensure!(
            !size.is_empty() && size.iter().all(u8::is_ascii_hexdigit),
            "invalid fixture HTTP chunk size"
        );
        let size = usize::from_str_radix(std::str::from_utf8(size)?, 16)
            .context("invalid fixture HTTP chunk size")?;
        ensure!(
            body.len() + size <= MAX_SSE,
            "fixture HTTP chunk body too large"
        );
        rest = &rest[end + 2..];
        overhead += end + 4;
        ensure!(
            overhead <= MAX_CHUNK_OVERHEAD,
            "fixture HTTP chunk overhead too large"
        );
        if size == 0 {
            ensure!(
                rest == b"\r\n",
                "unexpected fixture HTTP chunk trailer or trailing bytes"
            );
            return Ok(body);
        }
        ensure!(
            rest.get(size..size + 2) == Some(b"\r\n"),
            "incomplete fixture HTTP chunk"
        );
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

#[cfg(test)]
mod tests;
