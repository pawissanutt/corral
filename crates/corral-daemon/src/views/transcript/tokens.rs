use std::io::{self, Write};
use std::sync::LazyLock;

use anyhow::{Context, Result, ensure};
use corral_protocol::detail::{SessionItem, SessionItemPage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::devices::hex;

const FRAGMENT_BYTES: usize = 4096;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct Source {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) index: usize,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Token {
    pub(super) source: Source,
    pub(super) pair: Option<Source>,
    offset: usize,
    digest: String,
}

fn secret() -> Result<&'static [u8; 32]> {
    static SECRET: LazyLock<Result<[u8; 32], getrandom::Error>> = LazyLock::new(|| {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes)?;
        Ok(bytes)
    });
    SECRET
        .as_ref()
        .map_err(|error| anyhow::anyhow!("creating transcript token key: {error}"))
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    ensure!(text.len().is_multiple_of(2), "invalid replay token");
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| (byte as char).to_digit(16).context("invalid replay token");
            Ok((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
        })
        .collect()
}

fn signature(id: &str, payload: &[u8]) -> Result<String> {
    // HMAC-SHA256: prefix the session id with its byte length to bind the token's scope.
    let mut inner = [0x36; 64];
    let mut outer = [0x5c; 64];
    for (index, byte) in secret()?.iter().enumerate() {
        inner[index] ^= byte;
        outer[index] ^= byte;
    }
    let mut hash = Sha256::new();
    hash.update(inner);
    hash.update((id.len() as u64).to_be_bytes());
    hash.update(id.as_bytes());
    hash.update(payload);
    let mut mac = Sha256::new();
    mac.update(outer);
    mac.update(hash.finalize());
    Ok(hex(&mac.finalize()))
}

impl Token {
    fn encode(&self, id: &str) -> Result<String> {
        let bytes = serde_json::to_vec(self).context("encoding replay item token")?;
        Ok(format!("{}.{}", hex(&bytes), signature(id, &bytes)?))
    }

    pub(super) fn decode(id: &str, text: &str) -> Result<Self> {
        ensure!(text.len() <= 2048, "invalid replay token");
        let (payload, mac) = text.split_once('.').context("invalid replay token")?;
        let bytes = unhex(payload)?;
        let expected = signature(id, &bytes)?;
        ensure!(
            mac.len() == expected.len()
                && mac
                    .bytes()
                    .zip(expected.bytes())
                    .fold(0, |diff, (a, b)| diff | (a ^ b))
                    == 0,
            "invalid replay token"
        );
        serde_json::from_slice(&bytes).context("invalid replay token payload")
    }

    pub(super) fn fragment(mut self, id: &str, item: &SessionItem) -> Result<SessionItemPage> {
        let mut encoding = ItemEncoding {
            start: Some(self.offset),
            fragment: Vec::with_capacity(FRAGMENT_BYTES),
            ..ItemEncoding::default()
        };
        serde_json::to_writer(&mut encoding, item).context("encoding replay item fragment")?;
        ensure!(
            encoding.len > self.offset && hex(&encoding.hash.finalize()) == self.digest,
            "transcript item changed; reload the session"
        );
        let start = self.offset;
        // A bounded writer can stop midway through the final UTF-8 character.
        if let Err(error) = std::str::from_utf8(&encoding.fragment) {
            ensure!(
                error.error_len().is_none(),
                "invalid replay continuation offset"
            );
            encoding.fragment.truncate(error.valid_up_to());
        }
        let json = String::from_utf8(encoding.fragment).context("decoding replay item fragment")?;
        ensure!(!json.is_empty(), "invalid replay continuation offset");
        self.offset += json.len();
        Ok(SessionItemPage {
            json,
            offset: start as u64,
            bytes: encoding.len as u64,
            next: if self.offset < encoding.len {
                Some(self.encode(id)?)
            } else {
                None
            },
        })
    }
}

pub(super) fn begin(
    id: &str,
    source: Source,
    pair: Option<Source>,
    item: &SessionItem,
) -> Result<String> {
    let mut encoding = ItemEncoding::default();
    serde_json::to_writer(&mut encoding, item).context("hashing complete replay item")?;
    Token {
        source,
        pair,
        offset: 0,
        digest: hex(&encoding.hash.finalize()),
    }
    .encode(id)
}

/// Authenticate the full item without buffering it solely to return one fragment.
#[derive(Default)]
struct ItemEncoding {
    hash: Sha256,
    len: usize,
    start: Option<usize>,
    fragment: Vec<u8>,
}

impl Write for ItemEncoding {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.hash.update(bytes);
        if let Some(start) = self.start {
            let from = start.saturating_sub(self.len).min(bytes.len());
            let to = start
                .saturating_add(FRAGMENT_BYTES)
                .saturating_sub(self.len)
                .min(bytes.len());
            self.fragment.extend_from_slice(&bytes[from..to]);
        }
        self.len += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
