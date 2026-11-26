//! Blocks, the editable transcript (`docs/blocks.md` v1.3): the shared types
//! and validation (B1). The `cards` extension's `block` tool (B3) and the
//! desk's board (B2) both run this module, so both read a `cards_block` call,
//! an edit and a notice alike.
//!
//! - [`parse_call`]: a call's input, validated and normalized into a
//!   [`Batch`] of [`Op`]s (§1.3–§1.5), or refused with one teaching line.
//! - [`Receipt`]: the success result, `ok 3 ops (blocks/1 #…)`, and the
//!   binding (§1.6).
//! - [`Path`] and [`Parts`]: the path grammar, patches and the overlap rule
//!   (§1.4); [`set_spec`] and friends, what each kind's paths take.
//! - [`Generation`] and [`Tag`]: the incarnation tag (§1.2).
//! - [`message`]: the edit message and the notice lines (§3.3, §3.5).
//!
//! The fold (B2, `cards_spec::board`) is not here. Everything that needs a
//! board (whether an id is live, the 32-block cap, guards, item-id reuse,
//! revisions, visibility versions) is its job. This module only states the
//! fold's numbers ([`LIVE_MAX`], [`ITEM_IDS_MAX`], [`REVISIONS_MAX`], …) so
//! both sides name one constant.
//!
//! **Normalized form.** Every op is a struct with its keys in schema order;
//! every map (a patch's `set`, `append` and `order`) is a `BTreeMap`, keys in
//! byte order; defaults are filled (a step's `state`, a put's `target`), an
//! empty optional string is absent, strings are sanitized, numbers are
//! canonical ([`Num`]). serde_json's `preserve_order` never touches it: no
//! `Map` is written, so the bytes are the same in both builds. The digest
//! hashes that form, and normalizing is a fixed point (a normalized batch
//! parses back to itself).
//!
//! **Readings.** Where `docs/blocks.md` leaves a choice open, this module
//! takes one, and says so where it does (`Reading:` in the docs below):
//! - an op's depth (§7's "depth ≤ 6") is counted per op, scalars counted, as
//!   cards count depth, so a call nests at most [`CALL_MAX_DEPTH`];
//! - `replace` needs `kind`: a stateless validator can't check or normalize
//!   a body without it;
//! - a patch's parts are in byte order (`append`, `order`, `remove`, `set`),
//!   as all four of §3.3's line-2 examples are, after `of`;
//! - `stop` is never the agent's, not even `"stop":false` (§1.5);
//! - the remaining ones are on [`Op`], [`Body`], [`Parts`], [`Tag`] and in
//!   [`message`].

use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

use crate::digest::Digest;
use crate::doc::{quote, string};
use crate::sanitize::Lines;
use crate::{CardId, Error, deeper_than, first_key, json_len_within, key_path};

mod body;
pub mod message;
mod op;
mod patch;
mod tag;

pub use body::*;
pub use op::*;
pub use patch::*;
pub use tag::*;

/// The catalog this module implements: `blocks/1` in a result (§1.1): the
/// six kinds, the seven ops, the two targets and the two rail slots. A change
/// to any of them, or to the normalizer, is a new catalog.
pub const CATALOG: u32 = 1;

/// The tool's input schema (`schema-blocks.json`), shallow by design (§2.6):
/// the kinds' bodies are described, not spelled out. Validation is
/// [`parse_call`], which enforces it and more.
pub const SCHEMA: &str = include_str!("../schema-blocks.json");

/// The tool's names: the API name, then the runtime name (§1.1).
pub const NAMES: [&str; 2] = ["cards_block", "cards:block"];

// ── Bounds (§7). Each is tested at N and N+1. ────────────────────────────
// Character bounds count Unicode scalar values of the raw string (after
// `\r\n` / `\r` become `\n`), as cards' do.

pub const OPS_MIN: usize = 1;
pub const OPS_MAX: usize = 16;
/// A call's input, serialized compactly. Refused, never truncated.
pub const CALL_MAX_BYTES: usize = 8 * 1024;
/// One op's nesting, scalars counted. **Reading:** §7's "depth ≤ 6" counted
/// over the whole call would refuse a proposed diff, whose lines sit 6 deep
/// in its op (`op` → `body` → `hunks` → a hunk → `lines` → a line); counted
/// per op, it admits exactly the kinds §1.5 defines.
pub const OP_MAX_DEPTH: usize = 6;
/// `{"ops":[…]}` around ops of at most [`OP_MAX_DEPTH`].
pub const CALL_MAX_DEPTH: usize = OP_MAX_DEPTH + 2;
/// `^[a-z][a-z0-9_-]{0,23}$`
pub const BLOCK_ID_MAX: usize = 24;
/// `^[a-z0-9][a-z0-9_-]{0,23}$`
pub const ITEM_ID_MAX: usize = 24;
/// A path's field name: `[a-z_]{1,16}`.
pub const FIELD_NAME_MAX: usize = 16;
/// A block's title, top-level on every kind (§1.5).
pub const TITLE_MAX: usize = 80;
/// **Reading:** `why` (on `replace` and `remove`) and a run's `summary` are
/// one line of at most 80 characters, as a run title is (§1.8).
pub const WHY_MAX: usize = 80;
pub const SUMMARY_MAX: usize = 80;
/// The ids one `collapse`, `remove` or `merge` names (§1.3).
pub const IDS_MAX: usize = 8;
/// The largest body of any kind (a diff's, §1.5).
pub const BODY_MAX_BYTES: usize = 6 * 1024;
/// A success result, in bytes (§1.6).
pub const RESULT_MAX_BYTES: usize = 64;

// The fold's numbers (B2), named here so the extension, the board and the
// desk agree on one constant each.

/// Live blocks per conversation, groups and members included (§1.7).
pub const LIVE_MAX: usize = 32;
/// A group's members (§1.3).
pub const GROUP_MIN: usize = 2;
pub const GROUP_MAX: usize = 8;
/// Item ids an incarnation may ever name, live and removed (§1.4).
pub const ITEM_IDS_MAX: usize = 64;
/// Revisions per incarnation (§1.4): also the largest `rN` of an edit.
pub const REVISIONS_MAX: u32 = 9_999;

/// A block's id (§1.2), the agent's choice: `^[a-z][a-z0-9_-]{0,23}$`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(String);

impl BlockId {
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        let ok = !b.is_empty() && b.len() <= BLOCK_ID_MAX && b[0].is_ascii_lowercase() && b[1..].iter().all(|&c| id_char(c));
        ok.then(|| BlockId(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn id_char(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-'
}

/// What an op names (§1.2, §1.5): a block, or a card as `card:<card id>`.
/// The `:` can't be in a block id, so the two never collide.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ref {
    Block(BlockId),
    Card(CardId),
}

impl Ref {
    pub fn parse(s: &str) -> Option<Self> {
        match s.strip_prefix("card:") {
            Some(card) => CardId::parse(card).map(Ref::Card),
            None => BlockId::parse(s).map(Ref::Block),
        }
    }

    pub fn block(&self) -> Option<&BlockId> {
        match self {
            Ref::Block(id) => Some(id),
            Ref::Card(_) => None,
        }
    }
}

/// An item's id in a list (§1.4): `^[a-z0-9][a-z0-9_-]{0,23}$`, one item
/// for the whole incarnation. `u-` ids are the user's.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(String);

/// The user's item ids: `u-` and 6 of RFC 4648's base32, lowercase.
pub const USER_ID_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
pub const USER_ID_CHARS: usize = 6;

impl ItemId {
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        let ok = !b.is_empty()
            && b.len() <= ITEM_ID_MAX
            && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
            && b[1..].iter().all(|&c| id_char(c));
        ok.then(|| ItemId(s.to_string()))
    }

    /// A step the user adds (§1.4): `u-` and the low 30 bits of `random`
    /// (from the OS's random source; this crate does no I/O) as 6 base32
    /// characters, most significant first: `u-k3x7qa`.
    pub fn mint(random: u32) -> Self {
        let mut s = String::from("u-");
        for i in (0..USER_ID_CHARS).rev() {
            s.push(USER_ID_ALPHABET[((random >> (5 * i)) & 31) as usize] as char);
        }
        ItemId(s)
    }

    /// Whether the id has the user's prefix: the agent can't mint one.
    pub fn is_user(&self) -> bool {
        self.0.starts_with("u-")
    }

    /// Whether it is one [`ItemId::mint`] could make: `^u-[a-z2-7]{6}$`.
    pub fn is_minted(&self) -> bool {
        self.0.strip_prefix("u-").is_some_and(|r| r.len() == USER_ID_CHARS && r.bytes().all(|c| USER_ID_ALPHABET.contains(&c)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! display_as_str {
    ($($t:ty),*) => {$(
        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    )*};
}
display_as_str!(BlockId, ItemId, Kind, Target, Slot);

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ref::Block(id) => f.write_str(id.as_str()),
            Ref::Card(id) => write!(f, "card:{id}"),
        }
    }
}

impl Serialize for Ref {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// A block's kind (§1.5). `group` is made only by `merge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Plan,
    Status,
    Diff,
    Note,
    Metric,
    File,
    Group,
}

impl Kind {
    /// Every kind, in the design's order.
    pub const ALL: [Kind; 7] = [Kind::Plan, Kind::Status, Kind::Diff, Kind::Note, Kind::Metric, Kind::File, Kind::Group];
    /// The kinds an agent writes with `put` and `replace`.
    pub const AGENT: [Kind; 6] = [Kind::Plan, Kind::Status, Kind::Diff, Kind::Note, Kind::Metric, Kind::File];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Plan => "plan",
            Kind::Status => "status",
            Kind::Diff => "diff",
            Kind::Note => "note",
            Kind::Metric => "metric",
            Kind::File => "file",
            Kind::Group => "group",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The body's cap, its normalized compact JSON in bytes (§1.5).
    pub fn max_bytes(self) -> usize {
        match self {
            Kind::Plan => PLAN_MAX_BYTES,
            Kind::Status => STATUS_MAX_BYTES,
            Kind::Diff => DIFF_MAX_BYTES,
            Kind::Note => NOTE_MAX_BYTES,
            Kind::Metric => METRIC_MAX_BYTES,
            Kind::File => FILE_MAX_BYTES,
            Kind::Group => GROUP_MAX_BYTES,
        }
    }

    /// The list `state` sugar ticks (§1.3): a plan's steps, a diff's hunks.
    pub fn main_list(self) -> Option<&'static str> {
        match self {
            Kind::Plan => Some("steps"),
            Kind::Diff => Some("hunks"),
            _ => None,
        }
    }
}

/// Where a block draws (§1.2): placement, not identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Transcript,
    Rail,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Transcript => "transcript",
            Target::Rail => "rail",
        }
    }
}

/// A rail block's slot (§1.2, §4): `upper` between Prompt cache and Files,
/// `end` after Files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Upper,
    End,
}

impl Slot {
    pub fn as_str(self) -> &'static str {
        match self {
            Slot::Upper => "upper",
            Slot::End => "end",
        }
    }
}

/// A validated, normalized `cards_block` call: 1 to 16 ops, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Batch {
    pub ops: Vec<Op>,
}

impl Batch {
    /// What the digest hashes: `{"ops":[…]}`, the normalized ops, compact.
    pub fn preimage(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a batch always serializes")
    }

    /// The first 16 hex digits of SHA-256 over [`Batch::preimage`] (§1.6):
    /// the construction of a card's digest.
    pub fn digest(&self) -> Digest {
        Digest::of(&self.preimage())
    }

    /// The success result the extension returns for this batch.
    pub fn receipt(&self) -> Receipt {
        Receipt { ops: self.ops.len(), catalog: CATALOG, digest: self.digest() }
    }
}

/// Validate and normalize one `cards_block` input (§1.3–§1.5, §7): the
/// extension runs it on the call, and the desk on the same call in the
/// transcript, to bind it (§1.6).
///
/// Checks run cheapest first: depth, then bytes (counted, never built),
/// then each op in array order; the first failure is the refusal. Within an
/// op, keys are checked in schema order, and a map's entries in byte order,
/// so every build names the same failure.
pub fn parse_call(input: &Value) -> Result<Batch, Error> {
    if deeper_than(input, CALL_MAX_DEPTH) {
        return Err(Error::new("input", format!("nested too deep: an op nests at most {OP_MAX_DEPTH} levels")));
    }
    if json_len_within(input, CALL_MAX_BYTES).is_none() {
        return Err(Error::new("input", format!("over {CALL_MAX_BYTES} bytes; split the ops over calls or patch less")));
    }
    let Some(obj) = input.as_object() else {
        return Err(Error::new("input", "must be a JSON object {\"ops\":[…]}"));
    };
    if let Some(k) = first_key(obj, |k, _| k != "ops") {
        return Err(Error::new(key_path(k), "unknown parameter; a call takes ops only"));
    }
    let Some(ops) = obj.get("ops") else {
        return Err(Error::new("ops", format!("required: a list of {OPS_MIN} to {OPS_MAX} ops")));
    };
    let Some(items) = ops.as_array() else {
        return Err(Error::new("ops", format!("must be a list of {OPS_MIN} to {OPS_MAX} ops")));
    };
    let n = items.len();
    if n < OPS_MIN {
        return Err(Error::new("ops", "at least 1 op"));
    }
    if n > OPS_MAX {
        return Err(Error::new("ops", format!("at most {OPS_MAX} (got {n}); split them over calls")));
    }
    let ops = items.iter().enumerate().map(|(i, v)| op::parse(v, &format!("ops[{i}]"))).collect::<Result<Vec<_>, _>>()?;
    Ok(Batch { ops })
}

/// A success result's line (§1.6): `ok 3 ops (blocks/1 #3f9a1c2e4b5d6f70)`,
/// at most [`RESULT_MAX_BYTES`]. "ok" means valid, not applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Receipt {
    pub ops: usize,
    pub catalog: u32,
    pub digest: Digest,
}

impl Receipt {
    /// The receipt on a result's first line, if it is exactly one: `ok`,
    /// the op count and the catalog in decimal (no leading zero), the
    /// digest in 16 lowercase hex digits. A cut or reworded line has none,
    /// so its call is unbound (§1.6). The catalog is read whatever it is;
    /// [`Receipt::binds`] says whether this build knows it.
    pub fn parse(result: &str) -> Option<Self> {
        let line = result.split('\n').next().unwrap_or_default();
        if line.len() > RESULT_MAX_BYTES {
            return None;
        }
        let (ops, rest) = decimal(line.strip_prefix("ok ")?)?;
        let (catalog, rest) = decimal(rest.strip_prefix(" ops (blocks/")?)?;
        let (hex, rest) = rest.strip_prefix(" #")?.split_at_checked(16)?;
        let digest = Digest::parse(hex)?;
        (rest == ")").then_some(Receipt { ops: usize::try_from(ops).ok()?, catalog, digest })
    }

    /// Whether this is the receipt of exactly `batch` (§1.6): this build's
    /// catalog, its op count, and the digest of the call's own input as
    /// [`parse_call`] normalizes it. Anything else draws as a plain row,
    /// and none of its ops apply.
    pub fn binds(&self, batch: &Batch) -> bool {
        self.catalog == CATALOG && self.ops == batch.ops.len() && self.digest == batch.digest()
    }
}

impl std::fmt::Display for Receipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ok {} ops (blocks/{} #{})", self.ops, self.catalog, self.digest)
    }
}

/// A decimal number with no leading zero (`0` itself is not one: counts and
/// catalogs start at 1), at most 9 digits, and the rest.
pub(crate) fn decimal(s: &str) -> Option<(u32, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, rest) = s.split_at(n);
    if digits.is_empty() || digits.len() > 9 || digits.starts_with('0') {
        return None;
    }
    Some((digits.parse().ok()?, rest))
}

// ── Shared validation helpers ────────────────────────────────────────────

pub(crate) fn join(path: &str, seg: &str) -> String {
    if path.is_empty() { seg.to_string() } else { format!("{path}.{seg}") }
}

pub(crate) fn object<'a>(v: &'a Value, path: &str, shape: &str) -> Result<&'a Map<String, Value>, Error> {
    v.as_object().ok_or_else(|| Error::new(path, format!("must be an object {shape}")))
}

/// The first key of `o` (in byte order) not in `keys`, refused.
pub(crate) fn known(o: &Map<String, Value>, keys: &[&str], path: &str, teach: &str) -> Result<(), Error> {
    match first_key(o, |k, _| !keys.contains(&k)) {
        Some(k) => Err(Error::new(join(path, &key_path(k)), format!("unknown key; {teach}"))),
        None => Ok(()),
    }
}

/// A one-line string the agent may leave out; an empty one is absent.
pub(crate) fn optional(o: &Map<String, Value>, key: &str, path: &str, max: usize) -> Result<Option<String>, Error> {
    match o.get(key) {
        Some(v) => Ok(Some(string(v, &join(path, key), Lines::Single, 0, max, "")?).filter(|s| !s.is_empty())),
        None => Ok(None),
    }
}

/// `true` or `false`, absent as `false`.
pub(crate) fn flag(o: &Map<String, Value>, key: &str, path: &str) -> Result<bool, Error> {
    match o.get(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(Error::new(join(path, key), "must be true or false")),
    }
}

/// A whole number in `min..=max`, given as a JSON integer.
pub(crate) fn whole(v: &Value, path: &str, min: u32, max: u32) -> Result<u32, Error> {
    match v.as_u64() {
        Some(n) if n >= u64::from(min) && n <= u64::from(max) => Ok(n as u32),
        _ => Err(Error::new(path, format!("must be a whole number from {min} to {max} (got {})", crate::doc::json_quote(v)))),
    }
}

/// `v` as a block id; `what` says what it names, for the refusal.
pub(crate) fn block_id(v: Option<&Value>, path: &str) -> Result<BlockId, Error> {
    let Some(v) = v else {
        return Err(Error::new(path, "required"));
    };
    let Some(s) = v.as_str() else {
        return Err(Error::new(path, "must be a block id"));
    };
    if let Some(id) = BlockId::parse(s) {
        return Ok(id);
    }
    if matches!(Ref::parse(s), Some(Ref::Card(_))) {
        return Err(Error::new(path, format!("{} names a card; only a block can be here", quote(s))));
    }
    Err(Error::new(
        path,
        format!("{} must match ^[a-z][a-z0-9_-]{{0,23}}$: a lowercase letter, then up to 23 lowercase letters, digits, _ or -", quote(s)),
    ))
}

/// `v` as a block id or `card:<card id>`.
pub(crate) fn reference(v: Option<&Value>, path: &str) -> Result<Ref, Error> {
    let Some(v) = v else {
        return Err(Error::new(path, "required"));
    };
    let Some(s) = v.as_str() else {
        return Err(Error::new(path, "must be a block id"));
    };
    Ref::parse(s).ok_or_else(|| {
        Error::new(path, format!("{} must be a block id (^[a-z][a-z0-9_-]{{0,23}}$) or card:<card id> for a finished card", quote(s)))
    })
}

/// `ids`: 1 to [`IDS_MAX`] refs, none twice, each read by `one`.
pub(crate) fn id_list<T: PartialEq>(
    v: &Value,
    path: &str,
    one: impl Fn(Option<&Value>, &str) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let Some(items) = v.as_array() else {
        return Err(Error::new(path, format!("must be a list of 1 to {IDS_MAX} ids")));
    };
    if items.is_empty() {
        return Err(Error::new(path, "at least 1 id"));
    }
    if items.len() > IDS_MAX {
        return Err(Error::new(path, format!("at most {IDS_MAX} (got {}); use another op for the rest", items.len())));
    }
    let mut out: Vec<T> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let p = format!("{path}[{i}]");
        let id = one(Some(item), &p)?;
        if out.contains(&id) {
            return Err(Error::new(p, format!("{} is listed twice", crate::doc::json_quote(item))));
        }
        out.push(id);
    }
    Ok(out)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `N` bytes from `2N` lowercase hex digits.
pub(crate) fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != 2 * N || !b.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let nibble = |b: u8| if b.is_ascii_digit() { b - b'0' } else { b - b'a' + 10 };
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (nibble(b[2 * i]) << 4) | nibble(b[2 * i + 1]);
    }
    Some(out)
}

#[cfg(test)]
mod tests;
