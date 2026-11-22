//! The browser open has its own bounded transfer state, never a retained detail.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use corral_protocol::detail::{SESSION_HTML_CHUNK_BYTES, SESSION_HTML_MAX_BYTES, SessionFileChunk};
use sha2::{Digest, Sha256};

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

// The opaque continuation token is `<selector/inode stamp>.<content digest>`; both
// parts are lowercase SHA256 hex. Keep echoing the token verbatim, not just the digest.
fn revision_digest(revision: &str) -> Option<&str> {
    fn sha256_hex(s: &str) -> bool {
        s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }
    let (stamp, digest) = revision.split_once('.')?;
    (sha256_hex(stamp) && sha256_hex(digest)).then_some(digest)
}

#[derive(Default)]
pub(crate) struct HtmlRead {
    offset: u64,
    version: Option<(u64, String)>,
    utf8_tail: Vec<u8>,
    hash: Sha256,
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
        let digest = revision_digest(&chunk.revision).ok_or_else(invalid)?;
        if chunk.offset != self.offset
            || chunk.size > SESSION_HTML_MAX_BYTES
            || chunk.size < self.offset
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
        let last = end == chunk.size;
        self.text(&bytes, last)?;
        self.hash.update(&bytes);
        // Verify even an empty transfer before reporting completion. This checks equality
        // with the daemon's initial read, not authenticity or whether the HTML is safe to run.
        if last && format!("{:x}", self.hash.clone().finalize()) != digest {
            return Err("The HTML snapshot could not be verified. Try opening it again.".into());
        }
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

    fn revision(bytes: &[u8]) -> String {
        format!("{}.{:x}", "1".repeat(64), Sha256::digest(bytes))
    }

    #[test]
    fn file_open_needs_an_ordered_complete_utf8_snapshot() {
        let n = SESSION_HTML_CHUNK_BYTES;
        let mut first = vec![b'a'; n];
        first[n - 1] = 0xf0;
        let tail = &[0x9f, 0x92, 0xbb];
        let rev = revision(&[first.as_slice(), tail].concat());
        let mut read = HtmlRead::default();
        assert!(read.accept(chunk(0, n + 3, &rev, &first)).is_ok());
        assert_eq!(read.cursor(), (n as u64, Some(rev.clone())));
        let other_rev = format!("2{}", &rev[1..]);
        for bad in [
            chunk(n - 1, n + 3, &rev, tail),
            chunk(n, n + 3, &other_rev, tail),
            chunk(n, n + 4, &rev, tail),
            chunk(n, n + 3, &rev, &[]),
            chunk(n, n + 3, &rev, &[0, b'o', b'k']),
        ] {
            assert!(read.accept(bad).is_err());
        }
        assert_eq!(
            read.accept(chunk(n, n + 3, &rev, tail)),
            Ok((tail.to_vec(), 100))
        );
        assert!(read.accept(chunk(n + 3, n + 3, &rev, &[])).is_err());
        assert!(
            HtmlRead::default()
                .accept(chunk(0, 1, &revision(&[0xf0]), &[0xf0]))
                .is_err()
        );
        assert!(
            HtmlRead::default()
                .accept(chunk(0, SESSION_HTML_MAX_BYTES as usize + 1, &rev, &[]))
                .is_err()
        );
    }

    #[test]
    fn digest_verifies_earlier_chunks_even_when_revision_and_size_match() {
        let n = SESSION_HTML_CHUNK_BYTES;
        let mut bytes = vec![b'a'; n + 1];
        let rev = revision(&bytes);
        bytes[0] = b'b';
        let mut read = HtmlRead::default();
        assert!(read.accept(chunk(0, n + 1, &rev, &bytes[..n])).is_ok());
        assert!(
            read.accept(chunk(n, n + 1, &rev, &bytes[n..]))
                .unwrap_err()
                .contains("could not be verified")
        );
    }

    #[test]
    fn empty_html_requires_a_well_formed_matching_digest() {
        let rev = revision(&[]);
        assert_eq!(
            HtmlRead::default().accept(chunk(0, 0, &rev, &[])),
            Ok((vec![], 100))
        );
        for bad in [
            revision(b"not empty"),
            "1".repeat(64),
            rev.to_uppercase(),
            format!("{rev}.extra"),
        ] {
            assert!(HtmlRead::default().accept(chunk(0, 0, &bad, &[])).is_err());
        }
    }
}
