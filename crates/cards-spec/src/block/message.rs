//! The block messages the user sends (§3.3, §3.5), built and read here so
//! the desk, the fold and the memo agree on every byte.
//!
//! ```text
//! [block plan edited r4] Close the refresh race → order fix, test, deploy, bandaid · skip bandaid · stop before deploy
//! {"of":"bab3b9916631463b1935420c710c9287","order":{"steps":["fix","test","deploy","bandaid"]},"set":{"steps.bandaid.state":"skipped","steps.deploy.stop":true}}
//! [block tests v3 dismissed]
//! ```
//!
//! **A message** is a body, then a **trailer** of 0 to 2 notice lines.
//! [`split`] finds the trailer: the last line, if it is a notice line after
//! a `\n`, then the line before it, if it is one of the other kind. Nothing
//! else is stripped: a line that only looks like a notice, a third notice
//! line, a notice not at the end, a line with a `\r`, all stay in the body,
//! byte for byte. The memo matches an edit by its body (§3.7), and every
//! send builds its trailer afresh (§3.5).
//!
//! **Wrappers stay whole** (§3.5): the fold reads only the user's own text
//! (the host's rule, `fold::events`). Of what that leaves, an `<event …>`
//! payload ends `…</event>` on its last line and a compaction summary with
//! the host's own sentence, so neither ends in a notice line.
//!
//! **A notice line** is exactly
//! `^\[block ([a-z][a-z0-9_-]{0,23} v[1-9][0-9]{0,8}(, [a-z][a-z0-9_-]{0,23} v[1-9][0-9]{0,8}){0,7}) (dismissed|shown)\]$`.
//! An id named twice in a message's notices, in one line or across both,
//! applies neither ([`Trailer::named_twice`]); this module never builds
//! one.
//!
//! **The edit** is the whole body, two lines (§3.3):
//! - line 1, for people: `[block <id> edited r<N>] `, the block's title (or
//!   its id), ` → `, then the summary: `back to r<M>: ` for a restore (§3.6),
//!   then the pieces joined with ` · `, each one line cut at 40 characters,
//!   and the line cut at 200. Reading it, only the prefix counts (the id, the
//!   base revision `N` from 1 to 9999): the rest must be there, one line
//!   with no control character, and is display only;
//! - line 2, for machines: `{"of":"<tag>"` then the user's parts, in byte
//!   order (`append`, `order`, `set`), compact, at most 8 KiB. **Reading:**
//!   §3.3's prose lists "set, append and order, in schema key order", but
//!   its first example writes `order` before `set`: byte order is what all
//!   four examples do. That is how [`build`] writes it, always the same
//!   way. Read back, line 2 is read for what it says, not how it is
//!   spelled (§3.3, amended at B1): any JSON spelling of the same patch
//!   (keys in another order, spaces between tokens, another escape) is the
//!   same edit. It is one line with no control character, at most 8 KiB as
//!   written, an object of `of` and the user's parts, no key twice. The
//!   memo matches a message by its body's own bytes ([`Read::body`]), not
//!   by this spelling. Its paths must be user paths of some kind (§3.4),
//!   and pass the overlap rule; [`Edit::admit`] checks them against the
//!   block's kind where the message lands.
//!
//! **A card's block** (§3.3, amended at B1). `replace card:<id>` gives a
//! resolved card its first block incarnation (§1.5), a block like any
//! other, so the user edits it as `[block card:<id> edited rN]`: line 1's
//! id is a [`Ref`]. The grammar can't tell whether the card has a block,
//! and doesn't pretend to: the fold (B2) admits an edit only where `of` is
//! the tag of the id's live incarnation (§3.3's admission), and an open or
//! unconverted card has no incarnation, so no tag names it and the edit
//! applies nothing, as on a removed block. [`Edit::card`] names the card
//! so B2 can say so in the edit's outcome. Answering a card is never a
//! block edit: that is the card's own message (`[card <id>] …`, design §3).

use super::*;
use crate::sanitize::{cut_chars, ui_sanitize};
use crate::{MESSAGE_LINE_MAX, SUMMARY_PIECE_MAX, decode_json};

/// Line 1 of an edit, in characters (as a card answer's, design §3.3).
pub const EDIT_LINE_MAX: usize = MESSAGE_LINE_MAX;
/// Line 2 of an edit, in bytes.
pub const EDIT_PATCH_MAX_BYTES: usize = 8 * 1024;
/// A notice's version: 1 to 999,999,999 (§3.5, §7).
pub const VERSION_MAX: u32 = 999_999_999;
/// Ids in one notice line.
pub const NOTICE_IDS_MAX: usize = 8;

/// What a notice tells the agent (§3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Visibility {
    Dismissed,
    Shown,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Dismissed => "dismissed",
            Visibility::Shown => "shown",
        }
    }
}

/// One id's notice: `tests v3`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Notice {
    pub id: BlockId,
    pub version: u32,
}

/// One notice line: `[block tests v3, plan v4 dismissed]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeLine {
    pub visibility: Visibility,
    pub notices: Vec<Notice>,
}

impl NoticeLine {
    /// A line for these notices: 1 to 8, each id once, each version from 1
    /// to [`VERSION_MAX`]; `None` otherwise.
    pub fn new(visibility: Visibility, notices: Vec<Notice>) -> Option<Self> {
        let ok = (1..=NOTICE_IDS_MAX).contains(&notices.len())
            && notices.iter().all(|n| (1..=VERSION_MAX).contains(&n.version))
            && notices.iter().enumerate().all(|(i, n)| notices[..i].iter().all(|m| m.id != n.id));
        ok.then_some(NoticeLine { visibility, notices })
    }

    /// One line, exactly the grammar (module docs); `None` otherwise.
    pub fn parse(line: &str) -> Option<Self> {
        let inner = line.strip_prefix("[block ")?.strip_suffix(']')?;
        let (list, visibility) = match inner.strip_suffix(" dismissed") {
            Some(list) => (list, Visibility::Dismissed),
            None => (inner.strip_suffix(" shown")?, Visibility::Shown),
        };
        let mut notices = Vec::new();
        for entry in list.split(", ") {
            let (id, version) = entry.split_once(" v")?;
            let id = BlockId::parse(id)?;
            let (version, rest) = decimal(version)?;
            if !rest.is_empty() {
                return None;
            }
            notices.push(Notice { id, version });
            if notices.len() > NOTICE_IDS_MAX {
                return None;
            }
        }
        Some(NoticeLine { visibility, notices })
    }
}

impl std::fmt::Display for NoticeLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[block ")?;
        for (i, n) in self.notices.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{} v{}", n.id, n.version)?;
        }
        write!(f, " {}]", self.visibility.as_str())
    }
}

/// A message's trailer: its notice lines, in the order they stand, at most
/// one of each kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trailer {
    pub lines: Vec<NoticeLine>,
}

impl Trailer {
    /// A trailer of these lines: at most two, of different kinds, no id in
    /// both. The desk builds dismissals first, then shows.
    pub fn new(lines: Vec<NoticeLine>) -> Option<Self> {
        let t = Trailer { lines };
        let ok =
            t.lines.len() <= 2 && !(t.lines.len() == 2 && t.lines[0].visibility == t.lines[1].visibility) && t.named_twice().is_empty();
        ok.then_some(t)
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Ids named more than once across the lines (§3.5): none of their
    /// notices applies, and the fold marks them.
    pub fn named_twice(&self) -> Vec<BlockId> {
        let all: Vec<&BlockId> = self.lines.iter().flat_map(|l| l.notices.iter().map(|n| &n.id)).collect();
        let mut out: Vec<BlockId> = Vec::new();
        for (i, id) in all.iter().enumerate() {
            if all[..i].contains(id) && !out.contains(id) {
                out.push((*id).clone());
            }
        }
        out
    }

    /// The notices that apply, in the order they stand: every one whose id
    /// is named once.
    pub fn notices(&self) -> Vec<(Visibility, &Notice)> {
        let twice = self.named_twice();
        self.lines.iter().flat_map(|l| l.notices.iter().map(move |n| (l.visibility, n))).filter(|(_, n)| !twice.contains(&n.id)).collect()
    }

    /// What the desk appends to a message: `\n` and a line, for each line.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| format!("\n{l}")).collect()
    }
}

/// `text` as its body and its trailer (module docs). The body is `text`'s
/// own bytes, up to the `\n` before the trailer.
pub fn split(text: &str) -> (&str, Trailer) {
    let mut body = text;
    let mut lines: Vec<NoticeLine> = Vec::new();
    while lines.len() < 2 {
        let Some((rest, last)) = body.rsplit_once('\n') else { break };
        let Some(line) = NoticeLine::parse(last) else { break };
        if lines.iter().any(|l| l.visibility == line.visibility) {
            break;
        }
        lines.insert(0, line);
        body = rest;
    }
    (body, Trailer { lines })
}

/// A user's edit of one block (§3.3): what line 1 names and line 2 says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// A block, or a card's block (`card:<id>`, module docs).
    pub id: Ref,
    /// `rN`: the revision of incarnation `of` the user edited from.
    pub base: u32,
    /// The incarnation's tag.
    pub of: Tag,
    /// The user's parts: `append`, `order` and `set`; never `remove`.
    pub parts: Parts,
}

impl Serialize for Edit {
    /// Line 2: `of`, then the parts.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("of", &self.of)?;
        self.parts.write(&mut m)?;
        m.end()
    }
}

impl Edit {
    /// An edit of `id`, from revision `base` of incarnation `of`: `parts`
    /// (the user's `append`, `order` and `set`, as JSON) validated as
    /// [`parse`] reads line 2 (user paths of some kind, the overlap rule),
    /// and line 2 within [`EDIT_PATCH_MAX_BYTES`]. So what [`build`] writes
    /// from it reads back. The desk's hand-back (D8) and restore (§3.6)
    /// build here.
    pub fn new(id: impl Into<Ref>, base: u32, of: Tag, parts: &Value) -> Result<Self, Error> {
        let id = id.into();
        if !(1..=REVISIONS_MAX).contains(&base) {
            return Err(Error::new("base", format!("must be a revision from 1 to {REVISIONS_MAX}")));
        }
        let Some(o) = parts.as_object() else {
            return Err(Error::new("parts", "must be an object {append?, order?, set?}"));
        };
        known(o, &["append", "order", "set"], "", "an edit takes append, order and set")?;
        let edit = Edit { id, base, of, parts: parse_parts(o, "", Author::User)? };
        let n = edit.patch_text().len();
        if n > EDIT_PATCH_MAX_BYTES {
            return Err(Error::new("parts", format!("over {EDIT_PATCH_MAX_BYTES} bytes (got {n}); hand back fewer changes at once")));
        }
        Ok(edit)
    }

    /// The card, when this edits a card's block: B2 admits it only if
    /// `replace card:<id>` made one, and `of` is its live incarnation's tag.
    pub fn card(&self) -> Option<&CardId> {
        match &self.id {
            Ref::Card(id) => Some(id),
            Ref::Block(_) => None,
        }
    }

    /// Line 2, compact.
    pub fn patch_text(&self) -> String {
        serde_json::to_string(self).expect("an edit always serializes")
    }

    /// Admission's check of the paths (§3.3), against the kind of the
    /// block the edit lands on: every path a user path of `kind`, its value
    /// one the user may write. The incarnation, the base and the guards
    /// (§3.4) are the fold's.
    pub fn admit(&self, kind: Kind) -> Result<(), Error> {
        self.parts.fits(kind, Author::User)
    }
}

/// An edit's line 1: who it's for, and what changed, for people.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary<'a> {
    /// The block's title: line 1 names the block by its id without one.
    pub title: Option<&'a str>,
    /// A restore (§3.6): `back to r<M>: ` leads the pieces.
    pub back_to: Option<u32>,
    /// What the user did, a piece each: `skip bandaid`.
    pub pieces: Vec<&'a str>,
}

/// The edit message for `edit` (module docs). `edit` must hold the user's
/// parts as [`parse`] would read them (this writes, it doesn't check).
pub fn build(edit: &Edit, summary: &Summary) -> String {
    let one = |s: &str| ui_sanitize(s, Lines::Single);
    let name = summary.title.map(one).filter(|t| !t.is_empty()).unwrap_or_else(|| edit.id.to_string());
    let pieces: Vec<String> = summary.pieces.iter().map(|p| cut_chars(&one(p), SUMMARY_PIECE_MAX)).collect();
    let lead = summary.back_to.map(|r| format!("back to r{r}: ")).unwrap_or_default();
    let line = format!("[block {} edited r{}] {name} → {lead}{}", edit.id, edit.base, pieces.join(" · "));
    format!("{}\n{}", cut_chars(&line, EDIT_LINE_MAX), edit.patch_text())
}

/// `body` (a message's text without its trailer) read as an edit: `None`
/// unless all of it is the grammar (module docs).
pub fn parse(body: &str) -> Option<Edit> {
    let (line1, line2) = body.split_once('\n')?;
    if line2.len() > EDIT_PATCH_MAX_BYTES || line2.chars().any(char::is_control) {
        return None;
    }
    if line1.chars().count() > EDIT_LINE_MAX || line1.chars().any(char::is_control) {
        return None;
    }
    let (id, rest) = line1.strip_prefix("[block ")?.split_once(" edited r")?;
    let id = Ref::parse(id)?;
    let (base, rest) = decimal(rest)?;
    let rest = rest.strip_prefix("] ")?;
    if base > REVISIONS_MAX || rest.is_empty() {
        return None;
    }
    let v = decode_json(line2).ok()?;
    let o = v.as_object()?;
    if first_key(o, |k, _| !["of", "append", "order", "set"].contains(&k)).is_some() {
        return None;
    }
    let of = Tag::parse(o.get("of")?.as_str()?)?;
    let parts = parse_parts(o, "", Author::User).ok()?;
    Some(Edit { id, base, of, parts })
}

/// A genuine user message's text, read (§1.7 step 3): its edit, if all of
/// its body is one, then its notices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read<'a> {
    /// The text without its trailer: what the memo matches (§3.7).
    pub body: &'a str,
    pub edit: Option<Edit>,
    pub trailer: Trailer,
}

pub fn read(text: &str) -> Read<'_> {
    let (body, trailer) = split(text);
    Read { body, edit: parse(body), trailer }
}
