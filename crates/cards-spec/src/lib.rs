//! The agent-card spec (`docs/design.md` v1.1), shared by the `cards`
//! extension and synaps-desk: both run this crate, so both validate and hash
//! a card identically.
//!
//! - [`doc`]: [`parse_input`] and [`revalidate`] (§1.4).
//! - [`digest`]: the [`Digest`] of a validated card and the result's
//!   [`Envelope`] line (§1.5).
//! - [`answer`]: [`parse_answer`] (§1.4's answer shapes).
//! - [`message`]: the answer message, built and parsed (§3.3).
//! - [`fold`]: every card's state, a pure fold over the session's history
//!   (§2.3).
//! - [`memo`]: the desk's answers in flight until the history shows them,
//!   as pure transitions (§3.5).
//! - [`sanitize`]: the one sanitizer for every string a card renders, the
//!   refusals, and [`sanitize::fold_key`] (case folding pinned to the
//!   corpus's Unicode, never the toolchain's).
//! - [`types`]: the data and its bounds.
//! - [`block`]: blocks, the editable transcript (`docs/blocks.md` v1.3): the
//!   ops, the kinds, paths and patches, the incarnation tag, the edit and
//!   notice grammar.
//!
//! Ported from synaps-desk #75's card model
//! (`crates/synaps-client/src/surface/` at `3f61cff`), itself a mirror of
//! SynapsCLI's C1 (`6e895d27`), minus what only the dead core had: the
//! contract digest, host ids, marks, the `<ui_answer>` message and its
//! capsule. No I/O and no Synaps dependency: serde, serde_json and sha2
//! only.
//!
//! **Map order.** serde_json's `Map` sorts its keys unless `preserve_order`
//! is on, as it is in synaps-desk's build (gpui turns it on). Nothing this
//! crate writes goes through a `Map` (docs and answers are structs with a
//! fixed key order), and where a refusal names "the first" key of an
//! object, or quotes a JSON value, it sorts first ([`first_key`],
//! [`sorted_json`]). So every byte it writes is the same in both builds; CI
//! runs the tests in both (the `preserve_order` feature).

pub mod answer;
pub mod block;
pub mod board;
mod casefold;
pub mod digest;
pub mod doc;
pub mod fold;
pub mod memo;
pub mod message;
pub mod sanitize;
pub mod types;

pub use answer::parse_answer;
pub use digest::{Digest, Envelope, digest};
pub use doc::{Input, parse_input, revalidate};
pub use message::{Kind, Parsed, Reply};
pub use sanitize::ui_sanitize;
pub use types::*;

use sanitize::{Lines as L, cut_chars};

/// The Unicode version case folding is pinned to (`fixtures/casefold.json`).
pub const UNICODE_VERSION: (u8, u8, u8) = casefold::UNICODE_VERSION;

/// The input's JSON Schema (§1.4): the tool's `input_schema`. Validation is
/// [`parse_input`], which enforces it by hand and more.
pub const SCHEMA: &str = include_str!("../schema.json");

/// The longest refusal path and message, in characters (Shady #439 M1).
pub const ERROR_PATH_MAX: usize = 80;
pub const ERROR_MESSAGE_MAX: usize = 240;

/// A refusal: the offending path and one teaching line. `Display` is the
/// tool error the agent reads, e.g.
/// `options: at most 9 (got 12); ask in plain text or narrow the choices`.
///
/// Built only by [`Error::new`], which puts both parts through
/// `ui_sanitize` (single line) and bounds them, so no key or value the
/// agent wrote can carry a control, a bidi override or a newline into the
/// error, or make it long. Untrusted keys are written with [`key_path`]
/// first, so they stay readable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    path: String,
    message: String,
}

impl Error {
    pub(crate) fn new(path: impl AsRef<str>, message: impl AsRef<str>) -> Self {
        Self {
            path: cut_chars(&ui_sanitize(path.as_ref(), L::Single), ERROR_PATH_MAX),
            message: cut_chars(&ui_sanitize(message.as_ref(), L::Single), ERROR_MESSAGE_MAX),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for Error {}

/// A card's id (§1.5): `c` and 6 of `[a-z0-9]`, minted by the extension.
/// Not a credential (§5.2): it only names a card.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CardId(String);

impl CardId {
    /// `^c[a-z0-9]{6}$`
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        let ok = b.len() == 7 && b[0] == b'c' && b[1..].iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        ok.then(|| CardId(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Its heap bytes: the board's owned-byte count (§1.8).
    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

impl std::fmt::Display for CardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A key the agent or a client wrote, as a path segment: bare when it is a
/// plain identifier (`blocks`), else a JSON string with everything outside
/// printable ASCII escaped (`"\u001b[31m"`), cut to 24 characters first.
pub(crate) fn key_path(k: &str) -> String {
    let plain = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if !k.is_empty() && k.len() <= 24 && k.bytes().all(plain) {
        return k.to_string();
    }
    let mut out = String::from("\"");
    for c in k.chars().take(24) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
        }
    }
    if k.chars().count() > 24 {
        out.push('…');
    }
    out.push('"');
    out
}

/// Whether `v` nests deeper than `limit`, scalars counted as one level.
/// The recursion stops at `limit`, so a value of any depth is safe to ask
/// about (Shady #439 M2).
pub(crate) fn deeper_than(v: &serde_json::Value, limit: usize) -> bool {
    use serde_json::Value;
    if limit == 0 {
        return true;
    }
    match v {
        Value::Array(a) => a.iter().any(|v| deeper_than(v, limit - 1)),
        Value::Object(o) => o.values().any(|v| deeper_than(v, limit - 1)),
        _ => false,
    }
}

/// The compact JSON size of `v` (a value already within a depth bound),
/// counted without building it; `None` once it passes `cap`.
pub(crate) fn json_len_within(v: &impl serde::Serialize, cap: usize) -> Option<usize> {
    struct Count {
        n: usize,
        cap: usize,
    }
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.n += buf.len();
            if self.n > self.cap {
                return Err(std::io::Error::other("over the cap"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut c = Count { n: 0, cap };
    serde_json::to_writer(&mut c, v).ok().map(|()| c.n)
}

/// The first key of `obj` that `pick` takes, in sorted order, whatever the
/// build's map order (see the crate docs): every "first unknown key"
/// refusal goes through here, so every build names the same key.
pub(crate) fn first_key(
    obj: &serde_json::Map<String, serde_json::Value>,
    pick: impl Fn(&str, &serde_json::Value) -> bool,
) -> Option<&String> {
    obj.iter().filter(|(k, v)| pick(k, v)).map(|(k, _)| k).min()
}

/// `v` as compact JSON with every object's keys sorted (see [`first_key`]):
/// for values quoted in a refusal.
pub(crate) fn sorted_json(v: &serde_json::Value) -> String {
    fn sorted(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                serde_json::Value::Object(keys.into_iter().map(|k| (k.clone(), sorted(&o[k]))).collect())
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(v).to_string()
}

/// Decode JSON text that will become a card input or answer values: the
/// shared ingress (Shady #439 L2). Stricter than `serde_json::from_str`: a
/// repeated key at any level is refused, not resolved last-wins. Like
/// `serde_json` it refuses lone surrogates, numbers out of `f64` range and
/// nesting past 128; the model's own depth and byte caps apply after.
pub fn decode_json(text: &str) -> Result<serde_json::Value, Error> {
    let mut de = serde_json::Deserializer::from_str(text);
    let v = <Strict as serde::Deserialize>::deserialize(&mut de)
        .and_then(|v| de.end().map(|()| v.0))
        .map_err(|e| Error::new("json", e.to_string()))?;
    Ok(v)
}

/// A `Value` decoded with repeated keys refused.
struct Strict(serde_json::Value);

impl<'de> serde::Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> serde::de::Visitor<'de> for StrictVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("JSON")
    }
    fn visit_bool<E>(self, b: bool) -> Result<Self::Value, E> {
        Ok(b.into())
    }
    fn visit_i64<E>(self, n: i64) -> Result<Self::Value, E> {
        Ok(n.into())
    }
    fn visit_u64<E>(self, n: u64) -> Result<Self::Value, E> {
        Ok(n.into())
    }
    fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(n).map(Into::into).ok_or_else(|| E::custom("number out of range"))
    }
    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> {
        Ok(s.into())
    }
    fn visit_string<E>(self, s: String) -> Result<Self::Value, E> {
        Ok(s.into())
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = a.next_element()? {
            out.push(v);
        }
        Ok(out.into())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
        let mut out = serde_json::Map::new();
        while let Some(k) = m.next_key::<String>()? {
            let Strict(v) = m.next_value()?;
            if out.contains_key(&k) {
                return Err(serde::de::Error::custom(format!("the key {} is given twice", key_path(&k))));
            }
            out.insert(k, v);
        }
        Ok(out.into())
    }
}

#[cfg(test)]
mod tests;
