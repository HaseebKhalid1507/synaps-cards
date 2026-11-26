//! The incarnation tag (§1.2): 128 bits, from the history, that every desk
//! derives alike and a hand-back names (`"of"`, §3.3).
//!
//! ```text
//! tag        = first 32 hex digits of SHA-256(
//!                "blocks/1" NUL generation NUL id NUL creation NUL tool_id NUL op_index)
//! generation = SHA-256(the history's first message: {"content":…,"role":…})
//! ```
//!
//! **The generation's preimage** is the first message's `content` and
//! `role` (nothing else of it), as compact JSON: every object's keys sorted
//! by their bytes and every key starting `_synaps_` left out, at every
//! level; strings and numbers as serde_json writes them.
//!
//! **The tag's preimage** is UTF-8 text with NUL (`\0`) separators:
//! - `blocks/1`: a fixed domain, the catalog the tag was defined in (a later
//!   catalog doesn't change the tags of calls it reads);
//! - the generation as 64 lowercase hex digits;
//! - the id as the op names it (`plan`, or `card:c4f9x2a`);
//! - the creation number, decimal: the fold's count of incarnations in the
//!   generation, 1 for its first (§1.2). It comes from the fold's state,
//!   never from a per-id counter;
//! - the creating call's tool id, as the history holds it;
//! - the op's index in its call, decimal, **from 0** (as a refusal's
//!   `ops[0]` counts). Reading: §1.2 says "the op's index in its call".
//!
//! Every field but the tool id is free of NUL, and the tool id comes
//! fourth of six, so the text splits back one way even if a tool id held a
//! NUL: the preimage is injective.

use super::*;
use crate::digest::sha256;

/// The tag's domain, the first field of its preimage.
pub const TAG_DOMAIN: &str = "blocks/1";

/// A conversation generation (§1.2): the SHA-256 of its history's first
/// message, written as 64 lowercase hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Generation([u8; 32]);

impl Generation {
    /// The generation of a history whose first message is `first`.
    pub fn of(first: &Value) -> Self {
        Generation(sha256(Self::preimage(first).as_bytes()))
    }

    /// What [`Generation::of`] hashes (see the module docs).
    pub fn preimage(first: &Value) -> String {
        let mut out = String::from("{");
        for (i, k) in ["content", "role"].iter().filter(|k| first.get(**k).is_some()).enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&Value::from(*k).to_string());
            out.push(':');
            canonical(&first[*k], &mut out);
        }
        out.push('}');
        out
    }

    pub fn parse(hex: &str) -> Option<Self> {
        unhex(hex).map(Generation)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// `v` as compact JSON, keys sorted by bytes, `_synaps_*` keys left out.
fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().filter(|k| !k.starts_with("_synaps_")).collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::from(k.as_str()).to_string());
                out.push(':');
                canonical(&o[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(v, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// An incarnation's tag (§1.2): 128 bits, written as 32 lowercase hex
/// digits. Within a generation, two incarnations hash different creation
/// numbers, so two tags collide with a chance of about 2⁻¹²⁸.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag([u8; 16]);

impl Tag {
    /// The tag of the incarnation that op `op_index` of the call `tool_id`
    /// starts under `id`, the generation's `creation`th.
    pub fn new(generation: &Generation, id: &Ref, creation: u64, tool_id: &str, op_index: usize) -> Self {
        let h = sha256(&Self::preimage(generation, id, creation, tool_id, op_index));
        let mut out = [0u8; 16];
        out.copy_from_slice(&h[..16]);
        Tag(out)
    }

    /// What [`Tag::new`] hashes (see the module docs).
    pub fn preimage(generation: &Generation, id: &Ref, creation: u64, tool_id: &str, op_index: usize) -> Vec<u8> {
        format!("{TAG_DOMAIN}\0{generation}\0{id}\0{creation}\0{tool_id}\0{op_index}").into_bytes()
    }

    /// 32 lowercase hex digits.
    pub fn parse(hex: &str) -> Option<Self> {
        unhex(hex).map(Tag)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl std::fmt::Display for Generation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl Serialize for Tag {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
