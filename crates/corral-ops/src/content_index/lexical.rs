//! Complete-source candidate terms, independent of the bounded snippet/scoring cache.
//! A pathological vocabulary or word falls back to streaming, never to a false negative.
use std::collections::BTreeSet;
use std::io::Read;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{DOCUMENT_BYTES, extract_bytes};

const TERM_BYTES: usize = 128 * 1024;
const TERM_COUNT: usize = 8192;
const WORD_BYTES: usize = 8192;

#[derive(Serialize, Deserialize)]
pub(super) struct Lexical {
    pub prefix: String,
    pub complete: bool,
    pub binary: bool,
    pub terms: Option<Vec<String>>,
    pub fallback: Option<Vec<u64>>,
}

pub(super) fn extract(mut reader: impl Read) -> Result<Lexical> {
    let mut prefix = Vec::with_capacity(DOCUMENT_BYTES + 1);
    let mut terms = Some(BTreeSet::new());
    let mut term_bytes = 0;
    let mut word = String::new();
    let mut utf8 = Vec::new();
    let mut fallback = super::grams::empty();
    let mut rolling = super::grams::Rolling::default();
    let mut chunk = [0; 64 * 1024];
    loop {
        let n = reader
            .read(&mut chunk)
            .context("indexing complete search text")?;
        if n == 0 {
            break;
        }
        let keep = n.min((DOCUMENT_BYTES + 1).saturating_sub(prefix.len()));
        prefix.extend_from_slice(&chunk[..keep]);
        utf8.extend_from_slice(&chunk[..n]);
        // Invalid source bytes must not consume a partial valid scalar at the chunk end.
        let mut usable = utf8.len();
        for back in 1..=utf8.len().min(4) {
            let start = utf8.len() - back;
            if utf8[start] < 0x80 {
                break;
            }
            if utf8[start] >= 0xc0 {
                if std::str::from_utf8(&utf8[start..])
                    .is_err_and(|error| error.error_len().is_none())
                {
                    usable = start;
                }
                break;
            }
        }
        let decoded = String::from_utf8_lossy(&utf8[..usable]);
        super::grams::add(&mut fallback, &mut rolling, &decoded);
        for c in decoded.chars() {
            if terms.is_none() {
                break;
            }
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
                if word.len() > WORD_BYTES {
                    terms = None;
                    word.clear();
                    break;
                }
            } else {
                insert(&mut terms, &mut term_bytes, &mut word);
                if terms.is_none() {
                    break;
                }
            }
        }
        utf8.drain(..usable);
    }
    // A trailing incomplete UTF-8 sequence decodes to a separator (replacement character).
    insert(&mut terms, &mut term_bytes, &mut word);
    let extracted = extract_bytes(&prefix);
    Ok(Lexical {
        prefix: extracted.text,
        complete: extracted.complete,
        binary: extracted.binary,
        fallback: terms.is_none().then_some(fallback),
        terms: terms.map(|terms| terms.into_iter().collect()),
    })
}

fn insert(terms: &mut Option<BTreeSet<String>>, bytes: &mut usize, word: &mut String) {
    if word.is_empty() {
        return;
    }
    if let Some(terms) = terms {
        let term = word.to_lowercase();
        if !terms.contains(&term) {
            *bytes += term.len();
            terms.insert(term);
        }
        if terms.len() <= TERM_COUNT && *bytes <= TERM_BYTES {
            word.clear();
            return;
        }
    }
    *terms = None;
    word.clear();
}
