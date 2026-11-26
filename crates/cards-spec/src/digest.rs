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
//! SHA-256 is the `sha2` crate's; the tests keep the standard's vectors as
//! a check on it.

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

    /// The first 8 bytes of SHA-256 over `data`: a card's digest over its
    /// [`preimage`], a block call's over its normalized batch.
    pub(crate) fn of(data: &[u8]) -> Self {
        let mut out = [0u8; 8];
        out.copy_from_slice(&sha256(data)[..8]);
        Digest(out)
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
    Digest::of(&preimage(doc, wait))
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
    /// before its question, so that its id, catalog or digest is missing
    /// or damaged, has none; so has one over [`ENVELOPE_MAX_BYTES`] or with
    /// anything around it. The question is display only: one cut or
    /// rewritten after its first character still parses and binds (Shady
    /// #2). The catalog is read whatever it is; [`Envelope::binds`] says
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

/// SHA-256, from the `sha2` crate (§9.1). The tests pin it to FIPS 180-4's
/// vectors and its padding edges.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(data).into()
}

#[cfg(test)]
mod tests;
