//! The seven ops (§1.3), validated and normalized.
//!
//! **Normalized form**, each op's keys in schema order after `op`:
//! - `put`: `id`, `kind`, `title`, `target`, `slot`, `at`, `body`. `target`
//!   is always written (`transcript` by default) and `slot` exactly when the
//!   target is the rail (`upper` by default);
//! - `patch`: `id`, then the parts in byte order (`append`, `order`,
//!   `remove`, `set`), `state` sugar inside `set` ([`Parts`]);
//! - `replace`: `id`, `kind`, `title`, `body`, `why`;
//! - `collapse`: `id` or `ids` with `open` (always written), or `run` with
//!   its `summary`;
//! - `merge`: `into`, `title`, `ids`;
//! - `move`: `id`, `target`, `slot`, `at`, as given, except that a `slot`
//!   writes its target, the rail;
//! - `remove`: `id` or `ids`, `why`.
//!
//! An empty optional string (`title`, `why`) is absent, and a flag at its
//! default (`open:false`, `run:false`) is as if absent.
//!
//! **Readings:**
//! - `replace` needs `kind`, even to keep the block's kind (§1.3, amended
//!   at B1 from "a new kind or title if given"): the extension validates
//!   bodies without state, and a body means nothing without its kind;
//! - `merge` names 1 to 8 `ids`, since `into` may be a live group that the
//!   members join; a new group with fewer than 2 is the fold's to skip;
//! - `collapse` with `run` takes no `open`, and a `summary` only with `run`;
//! - `card:<id>` may be named by `patch`, `replace`, `collapse`, `remove`,
//!   `move` in the transcript, `at.after` and a `merge`'s `ids`. A card
//!   can't join a group (§1.5), but a member that can't is the fold's to
//!   skip, one by one (§1.3), never the call's to refuse: the other members
//!   and the other ops still land. A `put`, a merge's `into` and a `move`
//!   to the rail (cards don't go there) name blocks only.

use serde::ser::SerializeMap;

use super::*;

/// One op of a call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Put(Put),
    Patch(Patch),
    Replace(Replace),
    Collapse(Collapse),
    Merge(Merge),
    Move(Move),
    Remove(Remove),
}

/// `{"after":"<id>"}`: right after that block in its list, resolved once
/// when the op applies (§1.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct At {
    pub after: Ref,
}

/// Create only: a new incarnation at r1, or nothing on a live id (§1.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Put {
    pub id: BlockId,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub target: Target,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<Slot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<At>,
    pub body: Body,
}

/// Paths in the current incarnation (§1.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    pub id: Ref,
    pub parts: Parts,
}

impl Serialize for Patch {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("id", &self.id)?;
        self.parts.write(&mut m)?;
        m.end()
    }
}

/// A new incarnation in the same place and group (§1.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Replace {
    pub id: Ref,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub body: Body,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Fold (or with `open`, unfold) blocks: `id` or `ids`; or fold the tool
/// run this call sits in: `run` with its `summary`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Collapse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Ref>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<Ref>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Group blocks into `into`, one level deep (§1.3). `ids` keeps every
/// member as named, a card too: the fold skips the ones that can't join.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Merge {
    pub into: BlockId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub ids: Vec<Ref>,
}

/// Re-place a block (§1.2): with no target, slot or at, here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Move {
    pub id: Ref,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<Slot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<At>,
}

/// End incarnations: `id` or `ids` (§1.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Remove {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Ref>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<Ref>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

const OPS: &str = "put, patch, replace, collapse, merge, move or remove";

/// One op at `path` (`ops[2]`).
pub(crate) fn parse(v: &Value, path: &str) -> Result<Op, Error> {
    let o = object(v, path, "{op, …}")?;
    let p = join(path, "op");
    let Some(name) = o.get("op") else {
        return Err(Error::new(p, format!("required: {OPS}")));
    };
    Ok(match name.as_str() {
        Some("put") => Op::Put(put(o, path)?),
        Some("patch") => Op::Patch(patch(o, path)?),
        Some("replace") => Op::Replace(replace(o, path)?),
        Some("collapse") => Op::Collapse(collapse(o, path)?),
        Some("merge") => Op::Merge(merge(o, path)?),
        Some("move") => Op::Move(moving(o, path)?),
        Some("remove") => Op::Remove(remove(o, path)?),
        _ => return Err(Error::new(p, format!("{} is not an op; use {OPS}", crate::doc::json_quote(name)))),
    })
}

/// `kind`, which `put` and `replace` both need: `replace` names it even to
/// keep the block's (§1.3).
fn kind(o: &Map<String, Value>, path: &str, replace: bool) -> Result<Kind, Error> {
    let p = join(path, "kind");
    let kinds = "plan, status, diff, note, metric or file";
    let Some(v) = o.get("kind") else {
        let keep = if replace { ", even to keep the block's" } else { "" };
        return Err(Error::new(p, format!("required{keep}: {kinds}")));
    };
    match v.as_str().and_then(Kind::parse) {
        Some(Kind::Group) => Err(Error::new(p, "a group is made by merge")),
        Some(k) => Ok(k),
        None => Err(Error::new(p, format!("{} is not a kind; use {kinds}", crate::doc::json_quote(v)))),
    }
}

fn body(o: &Map<String, Value>, path: &str, kind: Kind) -> Result<Body, Error> {
    let p = join(path, "body");
    match o.get("body") {
        Some(v) => Body::parse(kind, v, &p),
        None => Err(Error::new(p, format!("required: the {kind}'s body"))),
    }
}

/// `target` and `slot` as given; a slot with the transcript is refused.
fn place(o: &Map<String, Value>, path: &str) -> Result<(Option<Target>, Option<Slot>), Error> {
    let target = match o.get("target") {
        None => None,
        Some(v) if v == "transcript" => Some(Target::Transcript),
        Some(v) if v == "rail" => Some(Target::Rail),
        Some(v) => {
            return Err(Error::new(join(path, "target"), format!("{} is not a target; use transcript or rail", crate::doc::json_quote(v))));
        }
    };
    let slot = match o.get("slot") {
        None => None,
        Some(v) if v == "upper" => Some(Slot::Upper),
        Some(v) if v == "end" => Some(Slot::End),
        Some(v) => return Err(Error::new(join(path, "slot"), format!("{} is not a slot; use upper or end", crate::doc::json_quote(v)))),
    };
    if slot.is_some() && target == Some(Target::Transcript) {
        return Err(Error::new(join(path, "slot"), "a slot places a block in the rail; leave it out, or use target rail"));
    }
    Ok((target, slot))
}

/// `at`: `{"after":"<id>"}`, never this op's own block.
fn at(o: &Map<String, Value>, path: &str, own: &Ref) -> Result<Option<At>, Error> {
    let p = join(path, "at");
    let Some(v) = o.get("at") else {
        return Ok(None);
    };
    if v == "top" {
        return Err(Error::new(p, "\"top\" is not in blocks/1; use {\"after\":\"<id>\"}, or leave at out to place it here"));
    }
    let a = object(v, &p, "{\"after\":\"<id>\"}")?;
    known(a, &["after"], &p, "at takes after")?;
    let after = reference(a.get("after"), &join(&p, "after"))?;
    if after == *own {
        return Err(Error::new(join(&p, "after"), "names this block itself; name another, or leave at out"));
    }
    Ok(Some(At { after }))
}

fn title(o: &Map<String, Value>, path: &str) -> Result<Option<String>, Error> {
    optional(o, "title", path, TITLE_MAX)
}

fn put(o: &Map<String, Value>, path: &str) -> Result<Put, Error> {
    known(
        o,
        &["op", "id", "kind", "title", "target", "slot", "at", "body"],
        path,
        "a put takes id, kind, title, target, slot, at and body",
    )?;
    let p = join(path, "id");
    let id = match reference(o.get("id"), &p) {
        Ok(Ref::Card(_)) => return Err(Error::new(p, "a card is made by cards_ui; replace a finished card instead")),
        _ => block_id(o.get("id"), &p)?,
    };
    let kind = kind(o, path, false)?;
    let title = title(o, path)?;
    let (target, slot) = place(o, path)?;
    let target = target.unwrap_or(if slot.is_some() { Target::Rail } else { Target::Transcript });
    let slot = (target == Target::Rail).then(|| slot.unwrap_or(Slot::Upper));
    let at = at(o, path, &Ref::Block(id.clone()))?;
    let body = body(o, path, kind)?;
    Ok(Put { id, kind, title, target, slot, at, body })
}

fn patch(o: &Map<String, Value>, path: &str) -> Result<Patch, Error> {
    known(o, &["op", "id", "set", "append", "remove", "order", "state"], path, "a patch takes id, set, append, remove, order and state")?;
    let id = reference(o.get("id"), &join(path, "id"))?;
    let parts = parse_parts(o, path, Author::Agent)?;
    Ok(Patch { id, parts })
}

fn replace(o: &Map<String, Value>, path: &str) -> Result<Replace, Error> {
    known(o, &["op", "id", "kind", "title", "body", "why"], path, "a replace takes id, kind, title, body and why")?;
    let id = reference(o.get("id"), &join(path, "id"))?;
    let kind = kind(o, path, true)?;
    let title = title(o, path)?;
    let body = body(o, path, kind)?;
    let why = optional(o, "why", path, WHY_MAX)?;
    Ok(Replace { id, kind, title, body, why })
}

/// `id` or `ids`, exactly one of them.
fn one_or_many(o: &Map<String, Value>, path: &str, what: &str) -> Result<(Option<Ref>, Vec<Ref>), Error> {
    match (o.get("id"), o.get("ids")) {
        (Some(_), Some(_)) => Err(Error::new(join(path, "ids"), "give id or ids, not both")),
        (Some(id), None) => Ok((Some(reference(Some(id), &join(path, "id"))?), Vec::new())),
        (None, Some(ids)) => Ok((None, id_list(ids, &join(path, "ids"), reference)?)),
        (None, None) => Err(Error::new(join(path, "id"), format!("required: id, or ids{what}"))),
    }
}

fn collapse(o: &Map<String, Value>, path: &str) -> Result<Collapse, Error> {
    known(o, &["op", "id", "ids", "open", "run", "summary"], path, "a collapse takes id, ids, open, run and summary")?;
    let run = flag(o, "run", path)?;
    let open = flag(o, "open", path)?;
    if run {
        if o.contains_key("id") || o.contains_key("ids") {
            return Err(Error::new(join(path, "run"), "folds this call's tool run; give it without id or ids"));
        }
        if open {
            return Err(Error::new(join(path, "open"), "opens blocks, not a run"));
        }
        let p = join(path, "summary");
        let Some(summary) = o.get("summary") else {
            return Err(Error::new(p, "required with run: the run's title"));
        };
        let summary = string(summary, &p, Lines::Single, 1, SUMMARY_MAX, "")?;
        return Ok(Collapse { id: None, ids: Vec::new(), open: None, run, summary: Some(summary) });
    }
    if o.contains_key("summary") {
        return Err(Error::new(join(path, "summary"), "titles a run; give it with run:true"));
    }
    let (id, ids) = one_or_many(o, path, ", or run:true with a summary")?;
    Ok(Collapse { id, ids, open: Some(open), run, summary: None })
}

fn merge(o: &Map<String, Value>, path: &str) -> Result<Merge, Error> {
    known(o, &["op", "into", "title", "ids"], path, "a merge takes into, title and ids")?;
    let into = block_id(o.get("into"), &join(path, "into"))?;
    let title = title(o, path)?;
    let p = join(path, "ids");
    let Some(v) = o.get("ids") else {
        return Err(Error::new(p, format!("required: the 1 to {IDS_MAX} blocks to group")));
    };
    let ids = id_list(v, &p, reference)?;
    if let Some(i) = ids.iter().position(|id| id.block() == Some(&into)) {
        return Err(Error::new(format!("{p}[{i}]"), "is into; a group can't be its own member"));
    }
    Ok(Merge { into, title, ids })
}

fn moving(o: &Map<String, Value>, path: &str) -> Result<Move, Error> {
    known(o, &["op", "id", "target", "slot", "at"], path, "a move takes id, target, slot and at")?;
    let id = reference(o.get("id"), &join(path, "id"))?;
    let (target, slot) = place(o, path)?;
    let target = target.or(slot.map(|_| Target::Rail));
    if matches!(id, Ref::Card(_)) && target == Some(Target::Rail) {
        let at = if slot.is_some() { "slot" } else { "target" };
        return Err(Error::new(join(path, at), "cards don't go to the rail"));
    }
    let at = at(o, path, &id)?;
    Ok(Move { id, target, slot, at })
}

fn remove(o: &Map<String, Value>, path: &str) -> Result<Remove, Error> {
    known(o, &["op", "id", "ids", "why"], path, "a remove takes id, ids and why")?;
    let (id, ids) = one_or_many(o, path, "")?;
    let why = optional(o, "why", path, WHY_MAX)?;
    Ok(Remove { id, ids, why })
}
