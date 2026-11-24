//! The digest of a validated card and the result's envelope line (§1.5).
//!
//! A success result's first line is the envelope the desk binds to:
//!
//! ```text
//! Card c4f9x2a is showing (cards/1 #3f9a1c2e4b5d6f70): Which approach?
//! ```
//!
//! `cards/1` is [`CATALOG`]. `#…` is the first 16 hex digits of SHA-256
//! over [`preimage`]: the normalized card as this crate writes it, compact,
//! keys in schema order, `\n` line endings, defaults materialized (`wait`
//! too), string options as `{label}`. The extension writes it for the card
//! it validated, after any hook rewrote the input. The desk parses the call
//! in the transcript with the same [`crate::parse_input`] and binds only
//! when the digests match ([`Envelope::binds`], §2.2): a rewritten input
//! is a plain row, never a different card. It is a 64-bit checksum against
//! accidental divergence, not an identity or a signature (§5.2).
//!
//! The line is at most [`ENVELOPE_MAX_BYTES`] of UTF-8: only the question
//! is cut to fit, ending in `…` ([`Envelope::showing`]), and it is display
//! only, so a cut question still binds.
//!
//! A close result is one line, `Card c4f9x2a closed (cards/1).`.
//!
//! SHA-256 is implemented here (FIPS 180-4), so the crate needs nothing but
//! serde; the tests pin it to the standard's vectors.

use crate::sanitize::cut_bytes;
use crate::{CATALOG, CardDoc, CardId, ENVELOPE_MAX_BYTES};

/// The first 8 bytes of a card's SHA-256, written as 16 lowercase hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Digest([u8; 8]);

impl Digest {
    /// 16 lowercase hex digits.
    pub fn parse(hex: &str) -> Option<Self> {
        let b = hex.as_bytes();
        if b.len() != 16 || !b.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let nibble = |b: u8| if b.is_ascii_digit() { b - b'0' } else { b - b'a' + 10 };
        let mut out = [0u8; 8];
        for (o, [hi, lo]) in out.iter_mut().zip(b.as_chunks::<2>().0) {
            *o = (nibble(*hi) << 4) | nibble(*lo);
        }
        Some(Digest(out))
    }

    pub fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// What the digest hashes: the doc's compact JSON (a struct, so its keys
/// are in schema order in every build) with `"wait"` last, where the
/// schema puts it. `wait` is in it because the desk draws it too (NEEDS
/// YOU): a hook that flipped it would otherwise still bind.
pub fn preimage(doc: &CardDoc, wait: bool) -> Vec<u8> {
    let mut out = serde_json::to_vec(doc).expect("a doc always serializes");
    // A doc always has its question, so it is never `{}`.
    assert_eq!(out.pop(), Some(b'}'));
    out.extend_from_slice(if wait { b",\"wait\":true}" } else { b",\"wait\":false}" });
    out
}

/// The digest of a validated card (`Input::Create`'s `doc` and `wait`).
pub fn digest(doc: &CardDoc, wait: bool) -> Digest {
    let h = sha256(&preimage(doc, wait));
    let mut out = [0u8; 8];
    out.copy_from_slice(&h[..8]);
    Digest(out)
}

/// A result's first line (§1.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Envelope {
    /// `Card <id> is showing (cards/<catalog> #<digest>): <question>`
    Showing { card: CardId, catalog: u32, digest: Digest, question: String },
    /// `Card <id> closed (cards/<catalog>).`
    Closed { card: CardId, catalog: u32 },
}

impl Envelope {
    /// The envelope of a card the extension shows, its question cut so the
    /// line is at most [`ENVELOPE_MAX_BYTES`] (§1.5).
    pub fn showing(card: CardId, doc: &CardDoc, wait: bool) -> Self {
        let digest = digest(doc, wait);
        let fixed = format!("Card {card} is showing (cards/{CATALOG} #{digest}): ").len();
        let question = cut_bytes(&doc.question, ENVELOPE_MAX_BYTES - fixed);
        Envelope::Showing { card, catalog: CATALOG, digest, question }
    }

    pub fn closed(card: CardId) -> Self {
        Envelope::Closed { card, catalog: CATALOG }
    }

    pub fn card(&self) -> &CardId {
        match self {
            Envelope::Showing { card, .. } | Envelope::Closed { card, .. } => card,
        }
    }

    /// The envelope on a result's first line, if it has one. A line cut
    /// short, over [`ENVELOPE_MAX_BYTES`], or with anything around it, has
    /// none. The catalog is read whatever it is; [`Envelope::binds`] says
    /// whether this build knows it.
    pub fn parse(result: &str) -> Option<Self> {
        let line = result.split('\n').next().unwrap_or_default();
        if line.len() > ENVELOPE_MAX_BYTES {
            return None;
        }
        let (id, rest) = line.strip_prefix("Card ")?.split_at_checked(7)?;
        let card = CardId::parse(id)?;
        if let Some(rest) = rest.strip_prefix(" is showing (cards/") {
            let (catalog, rest) = catalog(rest)?;
            let (hex, rest) = rest.strip_prefix(" #")?.split_at_checked(16)?;
            let digest = Digest::parse(hex)?;
            let question = rest.strip_prefix("): ")?;
            if question.is_empty() || question.chars().any(char::is_control) {
                return None;
            }
            return Some(Envelope::Showing { card, catalog, digest, question: question.to_string() });
        }
        let (catalog, rest) = catalog(rest.strip_prefix(" closed (cards/")?)?;
        (rest == ").").then_some(Envelope::Closed { card, catalog })
    }

    /// Whether this is the envelope of exactly this card: one this build's
    /// catalog shows, with the digest of `doc` and `wait` (§2.2 step 3).
    /// The question on the line is display only; the doc is the truth.
    pub fn binds(&self, doc: &CardDoc, wait: bool) -> bool {
        matches!(self, Envelope::Showing { catalog, digest: d, .. } if *catalog == CATALOG && *d == digest(doc, wait))
    }
}

impl std::fmt::Display for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Envelope::Showing { card, catalog, digest, question } => {
                write!(f, "Card {card} is showing (cards/{catalog} #{digest}): {question}")
            }
            Envelope::Closed { card, catalog } => write!(f, "Card {card} closed (cards/{catalog})."),
        }
    }
}

/// A catalog number: decimal, no leading zero.
fn catalog(s: &str) -> Option<(u32, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, rest) = s.split_at(n);
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return None;
    }
    Some((digits.parse().ok()?, rest))
}

// ── SHA-256 (FIPS 180-4 §6.2) ────────────────────────────────────────────

/// The first 32 bits of the fractional parts of the cube roots of the
/// first 64 primes (§4.2.2).
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
    0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
    0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
    0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The first 32 bits of the fractional parts of the square roots of the
/// first 8 primes (§5.3.3).
const H0: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = H0;
    let (blocks, rest) = data.as_chunks::<64>();
    for block in blocks {
        compress(&mut h, block);
    }
    // Padding (§5.1.1): 0x80, zeros to 56 mod 64, the bit length.
    let mut tail = rest.to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_be_bytes());
    for block in tail.as_chunks::<64>().0 {
        compress(&mut h, block);
    }
    let mut out = [0u8; 32];
    for (o, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *o = word.to_be_bytes();
    }
    out
}

fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (wi, c) in w.iter_mut().zip(block.as_chunks::<4>().0) {
        *wi = u32::from_be_bytes(*c);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for (k, wi) in K.iter().zip(w) {
        let t1 = hh
            .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
            .wrapping_add((e & f) ^ (!e & g))
            .wrapping_add(*k)
            .wrapping_add(wi);
        let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

#[cfg(test)]
mod tests;
