//! The browser open has its own bounded transfer state, never a retained detail.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use corral_protocol::detail::{SESSION_HTML_CHUNK_BYTES, SESSION_HTML_MAX_BYTES, SessionFileChunk};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HtmlProgress {
    Idle,
    Reading { id: String, percent: u8 },
    Launching,
    Failed(String),
}

pub(super) fn html_path(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm"))
}

pub(crate) fn openable(path: &str, bytes: u64) -> bool {
    bytes <= SESSION_HTML_MAX_BYTES && html_path(path)
}

#[derive(Default)]
pub(crate) struct HtmlRead {
    offset: u64,
    version: Option<(u64, String)>,
    utf8_tail: Vec<u8>,
}

impl HtmlRead {
    pub(crate) fn cursor(&self) -> (u64, Option<String>) {
        (
            self.offset,
            self.version.as_ref().map(|(_, rev)| rev.clone()),
        )
    }

    pub(crate) fn accept(&mut self, chunk: SessionFileChunk) -> Result<(Vec<u8>, u8), String> {
        let invalid = || "Invalid HTML chunk returned by the daemon.".to_owned();
        if chunk.offset != self.offset
            || chunk.size > SESSION_HTML_MAX_BYTES
            || chunk.size < self.offset
            || chunk.revision.is_empty()
            || self.version.as_ref().is_some_and(|(size, revision)| {
                *size != chunk.size || *revision != chunk.revision || self.offset == *size
            })
        {
            return Err(invalid());
        }
        let expected = (chunk.size - self.offset).min(SESSION_HTML_CHUNK_BYTES as u64) as usize;
        if chunk.data.len() != expected.div_ceil(3) * 4 {
            return Err(invalid());
        }
        let bytes = STANDARD.decode(chunk.data).map_err(|_| invalid())?;
        if bytes.len() != expected {
            return Err(invalid());
        }
        let end = self.offset + bytes.len() as u64;
        self.text(&bytes, end == chunk.size)?;
        self.offset = end;
        self.version = Some((chunk.size, chunk.revision));
        let percent = (end * 100).checked_div(chunk.size).unwrap_or(100) as u8;
        Ok((bytes, percent))
    }

    // A UTF-8 scalar may cross a chunk boundary. Only its unfinished tail is kept.
    fn text(&mut self, mut bytes: &[u8], last: bool) -> Result<(), String> {
        let invalid = || "Only UTF-8 HTML without NUL bytes can be opened.".to_owned();
        if bytes.contains(&0) {
            return Err(invalid());
        }
        while !self.utf8_tail.is_empty() {
            let Some((head, rest)) = bytes.split_first() else {
                return if last { Err(invalid()) } else { Ok(()) };
            };
            self.utf8_tail.push(*head);
            bytes = rest;
            match std::str::from_utf8(&self.utf8_tail) {
                Ok(_) => self.utf8_tail.clear(),
                Err(error) if error.error_len().is_none() => {}
                Err(_) => return Err(invalid()),
            }
        }
        match std::str::from_utf8(bytes) {
            Ok(_) => Ok(()),
            Err(error) if error.error_len().is_none() && !last => {
                self.utf8_tail
                    .extend_from_slice(&bytes[error.valid_up_to()..]);
                Ok(())
            }
            Err(_) => Err(invalid()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(offset: usize, size: usize, revision: &str, bytes: &[u8]) -> SessionFileChunk {
        SessionFileChunk {
            offset: offset as u64,
            size: size as u64,
            revision: revision.into(),
            data: STANDARD.encode(bytes),
        }
    }

    #[test]
    fn file_open_needs_an_ordered_complete_utf8_snapshot() {
        let n = SESSION_HTML_CHUNK_BYTES;
        let mut first = vec![b'a'; n];
        first[n - 1] = 0xf0;
        let mut read = HtmlRead::default();
        assert!(read.accept(chunk(0, n + 3, "v1", &first)).is_ok());
        assert_eq!(read.cursor(), (n as u64, Some("v1".into())));
        let tail = &[0x9f, 0x92, 0xbb];
        for bad in [
            chunk(n - 1, n + 3, "v1", tail),
            chunk(n, n + 3, "v2", tail),
            chunk(n, n + 4, "v1", tail),
            chunk(n, n + 3, "v1", &[]),
            chunk(n, n + 3, "v1", &[0, b'o', b'k']),
        ] {
            assert!(read.accept(bad).is_err());
        }
        assert_eq!(
            read.accept(chunk(n, n + 3, "v1", tail)),
            Ok((tail.to_vec(), 100))
        );
        assert!(read.accept(chunk(n + 3, n + 3, "v1", &[])).is_err());
        assert!(
            HtmlRead::default()
                .accept(chunk(0, 1, "v1", &[0xf0]))
                .is_err()
        );
        assert!(
            HtmlRead::default()
                .accept(chunk(0, SESSION_HTML_MAX_BYTES as usize + 1, "v1", &[]))
                .is_err()
        );
    }
}
