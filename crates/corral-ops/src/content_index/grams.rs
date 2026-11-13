//! Fixed-memory, no-false-negative ASCII substring filter for oversized vocabularies.
//! False positives are resolved by the normal source scanner; short/Unicode queries scan.
const BITS: usize = 128 * 1024;
const BASE: u64 = 257;
const REMOVE: u64 = BASE.pow(7);

#[derive(Default)]
pub(super) struct Rolling {
    hash: u64,
    window: [u8; 8],
    used: usize,
    next: usize,
}

impl Rolling {
    pub fn push(&mut self, byte: u8) -> Option<usize> {
        if self.used == 8 {
            self.hash = self
                .hash
                .wrapping_sub(u64::from(self.window[self.next]).wrapping_mul(REMOVE));
        } else {
            self.used += 1;
        }
        self.window[self.next] = byte;
        self.next = (self.next + 1) % 8;
        self.hash = self.hash.wrapping_mul(BASE).wrapping_add(u64::from(byte));
        (self.used == 8).then(|| {
            let mut h = self.hash;
            h ^= h >> 33;
            h = h.wrapping_mul(0xff51afd7ed558ccd);
            h ^= h >> 33;
            (h as usize) % BITS
        })
    }
}

pub(super) fn empty() -> Vec<u64> {
    vec![0; BITS / 64]
}

pub(super) fn add(bits: &mut [u64], rolling: &mut Rolling, text: &str) {
    for c in text.chars().flat_map(char::to_lowercase) {
        let mut encoded = [0; 4];
        for &byte in c.encode_utf8(&mut encoded).as_bytes() {
            if let Some(bit) = rolling.push(byte) {
                bits[bit / 64] |= 1 << (bit % 64);
            }
        }
    }
}

pub(super) fn may_contain(bits: &[u64], word: &str) -> bool {
    // Context-sensitive Unicode lowercasing can cross fragment boundaries. Restrict this
    // optional rejection filter to ASCII; complete lexical terms still handle Unicode.
    if !word.is_ascii() || word.len() < 8 || bits.len() != BITS / 64 {
        return true;
    }
    let mut rolling = Rolling::default();
    word.bytes().all(|byte| {
        rolling
            .push(byte)
            .is_none_or(|bit| bits[bit / 64] & (1 << (bit % 64)) != 0)
    })
}
