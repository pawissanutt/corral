//! Content-Length framing for protocol v1 (docs/extensions-protocol.md, "Framing").

use super::envelope::Envelope;
use super::error::{ErrorCode, ExtError};
use std::io::{ErrorKind, Read, Write};

/// Maximum frame body size (**proposal**, plan section 11). A typed response is one frame.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Longest header line accepted; `Content-Length: 1048576` is 23 bytes.
const MAX_HEADER_LINE: usize = 64;
const CONTENT_LENGTH: &str = "Content-Length";

/// Reads frames from an extension's stdout. Memory per frame is bounded by the limit: the
/// length is checked before the body buffer is allocated. Wrap pipes in a `BufReader`.
pub struct FrameReader<R> {
    inner: R,
    limit: usize,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self::with_limit(inner, MAX_FRAME_BYTES)
    }

    pub fn with_limit(inner: R, limit: usize) -> Self {
        Self { inner, limit }
    }

    /// Next frame body, or `None` on a clean end of stream before any header byte.
    pub fn read_frame(&mut self) -> Result<Option<String>, ExtError> {
        let Some(first) = self.header_line(true)? else {
            return Ok(None);
        };
        let len = self.content_length(&first)?;
        match self.header_line(false)? {
            Some(line) if line.is_empty() => {}
            _ => {
                return Err(ExtError::protocol_violation(
                    "unexpected_header",
                    "only one Content-Length header is allowed before the blank line",
                ));
            }
        }
        let mut body = vec![0u8; len];
        let mut filled = 0;
        while filled < len {
            let n = self.read_some(&mut body[filled..])?;
            if n == 0 {
                return Err(ExtError::protocol_violation(
                    "truncated_body",
                    format!("stream ended after {filled} of {len} body bytes"),
                ));
            }
            filled += n;
        }
        String::from_utf8(body).map(Some).map_err(|e| {
            ExtError::protocol_violation(
                "invalid_utf8",
                format!(
                    "frame body is not UTF-8 at byte {}",
                    e.utf8_error().valid_up_to()
                ),
            )
        })
    }

    /// Next frame decoded as an envelope, or `None` on a clean end of stream.
    pub fn read_envelope(&mut self) -> Result<Option<Envelope>, ExtError> {
        self.read_frame()?
            .map(|body| Envelope::decode(&body))
            .transpose()
    }

    /// Confirms the stream ends here; any further byte is `trailing_bytes`.
    pub fn finish(&mut self) -> Result<(), ExtError> {
        let mut byte = [0u8; 1];
        match self.read_some(&mut byte)? {
            0 => Ok(()),
            _ => Err(ExtError::protocol_violation(
                "trailing_bytes",
                "bytes follow the final frame",
            )),
        }
    }

    fn content_length(&self, line: &[u8]) -> Result<usize, ExtError> {
        let malformed = || {
            ExtError::protocol_violation(
                "unexpected_header",
                "the first header line must be `Content-Length: <decimal length>`",
            )
        };
        let line = std::str::from_utf8(line).map_err(|_| malformed())?;
        let (name, value) = line.split_once(':').ok_or_else(malformed)?;
        if !name.eq_ignore_ascii_case(CONTENT_LENGTH) {
            return Err(malformed());
        }
        let value = value.trim_start_matches(' ');
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed());
        }
        let len = value.parse::<u64>().unwrap_or(u64::MAX);
        if len > self.limit as u64 {
            return Err(ExtError::protocol_violation(
                "frame_too_large",
                format!("frame of {len} bytes exceeds the {}-byte limit", self.limit),
            ));
        }
        Ok(len as usize)
    }

    fn header_line(&mut self, eof_ok: bool) -> Result<Option<Vec<u8>>, ExtError> {
        let mut line = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            if self.read_some(&mut byte)? == 0 {
                if line.is_empty() && eof_ok {
                    return Ok(None);
                }
                return Err(ExtError::protocol_violation(
                    "truncated_header",
                    "stream ended inside the frame header",
                ));
            }
            if byte[0] == b'\n' {
                if line.pop() == Some(b'\r') {
                    return Ok(Some(line));
                }
                return Err(ExtError::protocol_violation(
                    "unexpected_header",
                    "header lines must end with CRLF",
                ));
            }
            line.push(byte[0]);
            if line.len() > MAX_HEADER_LINE {
                return Err(ExtError::protocol_violation(
                    "header_too_long",
                    format!("header line exceeds {MAX_HEADER_LINE} bytes"),
                ));
            }
        }
    }

    fn read_some(&mut self, buf: &mut [u8]) -> Result<usize, ExtError> {
        loop {
            match self.inner.read(buf) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    return Err(ExtError::new(
                        ErrorCode::Internal,
                        format!("reading extension output: {e}"),
                    ));
                }
            }
        }
    }
}

/// Writes one frame. A body above `limit` is `limit_exceeded` and nothing is written.
pub fn write_frame<W: Write>(out: &mut W, body: &str, limit: usize) -> Result<(), ExtError> {
    if body.len() > limit {
        return Err(ExtError::new(
            ErrorCode::LimitExceeded,
            format!(
                "frame of {} bytes exceeds the {limit}-byte limit",
                body.len()
            ),
        ));
    }
    let header = format!("{CONTENT_LENGTH}: {}\r\n\r\n", body.len());
    out.write_all(header.as_bytes())
        .and_then(|()| out.write_all(body.as_bytes()))
        .and_then(|()| out.flush())
        .map_err(|e| ExtError::new(ErrorCode::Internal, format!("writing frame: {e}")))
}

/// Encodes and writes one envelope under the default frame limit.
pub fn write_envelope<W: Write>(out: &mut W, envelope: &Envelope) -> Result<(), ExtError> {
    write_frame(out, &envelope.encode()?, MAX_FRAME_BYTES)
}
