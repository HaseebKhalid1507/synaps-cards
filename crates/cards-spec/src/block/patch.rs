//! Paths and patches (§1.4): the grammar, what each kind's paths take, the
//! overlap rule, and a patch's normalized parts.
//!
//! A patch names no kind, so the extension, which keeps no state, checks it
//! against every kind: it is valid when one kind takes all of its paths with
//! their values. The fold then applies it to the block's real kind, path by
//! path ([`Parts::fits`] and the `*_spec` functions say what fits; B2 skips
//! and marks the rest).
//!
//! **Normalized parts.** `append`, `order` and `set` are maps in byte order
//! of their paths; `remove` keeps its order. `state` sugar becomes `set`
//! (`{"fix":"done"}` is `{"steps.fix.state":"done"}`, the list read from the
//! words: plan states and hunk states share none). A string is sanitized as
//! its field is, except that `text` keeps its line breaks: it is a note's
//! (many lines) or a status's (one), and only the fold knows which, so the
//! fold folds a status's. Normalizing never depends on the kind, so every
//! kind that takes a patch normalizes it alike.
//!
//! **The overlap rule** (§1.4), checked before any kind: a path named twice
//! (`state` sugar expanded, an item removed and appended, an item appended
//! twice, an item removed twice); a field set, or lines appended, on an item
//! the patch removes or appends; an id twice in one `order` list. A list
//! ordered twice can't be written: `order` is a map, and a repeated key is
//! refused when JSON text is decoded (`decode_json`).

use std::collections::{BTreeMap, BTreeSet};

use serde::ser::SerializeMap;

use super::*;
use crate::doc::json_quote;

/// Who wrote a patch: the agent (an op) or the user (an edit, §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Author {
    Agent,
    User,
}

/// A path (§1.4): `field`, `list.item` or `list.item.field`, as
/// `^[a-z_]{1,16}(\.[a-z0-9][a-z0-9_-]{0,23}(\.[a-z_]{1,16})?)?$`. There
/// are no indices: `steps.0` names the step whose id is `0`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Path(String);

fn field_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= FIELD_NAME_MAX && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

impl Path {
    pub fn parse(s: &str) -> Option<Self> {
        let mut segs = s.split('.');
        if !field_name(segs.next()?) {
            return None;
        }
        if let Some(item) = segs.next() {
            ItemId::parse(item)?;
            if let Some(field) = segs.next()
                && !field_name(field)
            {
                return None;
            }
        }
        segs.next().is_none().then(|| Path(s.to_string()))
    }

    fn of(list: &str, item: &str, field: &str) -> Self {
        Path(format!("{list}.{item}.{field}"))
    }

    /// The field, or the list.
    pub fn head(&self) -> &str {
        self.0.split('.').next().unwrap_or_default()
    }

    pub fn item(&self) -> Option<&str> {
        self.0.split('.').nth(1)
    }

    /// The item's field.
    pub fn field(&self) -> Option<&str> {
        self.0.split('.').nth(2)
    }

    /// `list.item`, for a path inside an item.
    fn item_path(&self) -> Option<String> {
        Some(format!("{}.{}", self.head(), self.item()?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Path {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

/// What `set` may give a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    /// A string of `min..=max` characters (`min` 0: `""` clears it).
    Text {
        lines: Lines,
        min: usize,
        max: usize,
    },
    /// A note's text: at most 1500 characters on at most 30 lines.
    NoteText,
    /// One of these words.
    State {
        all: &'static [&'static str],
        what: &'static str,
    },
    Flag,
    Whole {
        min: u32,
        max: u32,
    },
    /// `{"n":…,"of":…}`.
    Progress,
}

const fn one_line(min: usize, max: usize) -> Spec {
    Spec::Text { lines: Lines::Single, min, max }
}

macro_rules! words {
    ($t:ty) => {
        Spec::State { all: <$t>::ALL, what: <$t>::WHAT }
    };
}

/// The user's states for a step (a skip, an unskip) and a hunk (§3.4).
const USER_STEP_STATES: &[&str] = &["todo", "skipped"];
const USER_HUNK_STATES: &[&str] = &["kept", "dropped"];

/// What `set` may give `path` on a block of `kind`, written by `author`; `None`
/// if it may not set it at all. The agent's paths are the kinds' fields
/// (§1.5), `stop` aside; the user's are §3.4's table.
pub fn set_spec(kind: Kind, author: Author, path: &Path) -> Option<Spec> {
    use Kind::*;
    let at = (kind, path.head(), path.item().is_some(), path.field());
    match author {
        Author::Agent => Some(match at {
            (_, "title", false, None) => one_line(0, TITLE_MAX),
            (Plan, "steps", true, Some("label")) => one_line(1, STEP_LABEL_MAX),
            (Plan, "steps", true, Some("state")) => words!(StepState),
            (Plan, "steps", true, Some("note")) => one_line(0, STEP_NOTE_MAX),
            (Status, "text", false, None) => one_line(1, STATUS_TEXT_MAX),
            (Status, "detail", false, None) => one_line(0, STATUS_DETAIL_MAX),
            (Status, "progress", false, None) => Spec::Progress,
            (Status, "state", false, None) => words!(StatusState),
            (Diff, "file", false, None) => one_line(1, DIFF_FILE_MAX),
            (Diff, "state", false, None) => words!(DiffState),
            (Diff, "hunks", true, Some("at")) => one_line(0, HUNK_AT_MAX),
            (Diff, "hunks", true, Some("state")) => words!(HunkState),
            (Note, "text", false, None) => Spec::NoteText,
            (Metric, "label", false, None) => one_line(1, METRIC_LABEL_MAX),
            (Metric, "value", false, None) => one_line(1, METRIC_VALUE_MAX),
            (Metric, "was" | "unit", false, None) => one_line(0, METRIC_VALUE_MAX),
            (Metric, "state", false, None) => words!(MetricState),
            (File, "path", false, None) => one_line(1, FILE_PATH_MAX),
            (File, "line", false, None) => Spec::Whole { min: 1, max: LINE_NUMBER_MAX },
            (File, "note", false, None) => one_line(0, FILE_NOTE_MAX),
            (File, "state", false, None) => words!(FileState),
            _ => return None,
        }),
        Author::User => Some(match at {
            (Plan, "steps", true, Some("label")) => one_line(1, STEP_LABEL_MAX),
            (Plan, "steps", true, Some("state")) => Spec::State { all: USER_STEP_STATES, what: "a skip or an unskip" },
            (Plan, "steps", true, Some("stop")) => Spec::Flag,
            (Note, "text", false, None) => Spec::NoteText,
            (Note, "title", false, None) => one_line(0, TITLE_MAX),
            (Diff, "hunks", true, Some("state")) => Spec::State { all: USER_HUNK_STATES, what: "a keep or a drop" },
            _ => return None,
        }),
    }
}

/// What `append` adds to a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListSpec {
    /// A plan's `steps`: steps.
    Steps,
    /// A diff's `hunks`: hunks.
    Hunks,
    /// A hunk's `hunks.<id>.lines`: diff lines.
    Lines,
    /// A metric's `series`: numbers.
    Series,
}

/// What `append` may add at `path` on a block of `kind`; `None` if nothing.
/// The user appends only steps, with `u-` ids (§3.4).
pub fn append_spec(kind: Kind, author: Author, path: &Path) -> Option<ListSpec> {
    match (kind, author, path.head(), path.item().is_some(), path.field()) {
        (Kind::Plan, _, "steps", false, None) => Some(ListSpec::Steps),
        (Kind::Diff, Author::Agent, "hunks", false, None) => Some(ListSpec::Hunks),
        (Kind::Diff, Author::Agent, "hunks", true, Some("lines")) => Some(ListSpec::Lines),
        (Kind::Metric, Author::Agent, "series", false, None) => Some(ListSpec::Series),
        _ => None,
    }
}

/// Whether `remove` may name `path` on a block of `kind`: an item of a list
/// with ids, and only by the agent (§1.4).
pub fn remove_spec(kind: Kind, author: Author, path: &Path) -> bool {
    author == Author::Agent && path.field().is_none() && path.item().is_some() && kind.main_list() == Some(path.head())
}

/// Whether `order` may name the list `path` on a block of `kind`: a plan's
/// steps, or (the agent's only) a diff's hunks.
pub fn order_spec(kind: Kind, author: Author, path: &Path) -> bool {
    path.item().is_none()
        && match (kind, author) {
            (Kind::Plan, _) => path.head() == "steps",
            (Kind::Diff, Author::Agent) => path.head() == "hunks",
            _ => false,
        }
}

/// A value `set` gives, normalized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SetValue {
    /// A string, or a state's word.
    Text(String),
    Flag(bool),
    Whole(u32),
    Progress(Progress),
}

impl Spec {
    /// `v` as this spec takes it at `at`, normalized as a patch's value is.
    pub fn check(self, v: &Value, path: &str, at: &Path) -> Result<SetValue, Error> {
        Ok(match self {
            Spec::Text { lines, min, max } => {
                let lines = if at.as_str() == "text" { Lines::Multi } else { lines };
                SetValue::Text(string(v, path, lines, min, max, "")?)
            }
            Spec::NoteText => SetValue::Text(note_text(v, path)?),
            Spec::State { all, what } => SetValue::Text(state_word(v, path, all, what)?.to_string()),
            Spec::Flag => match v {
                Value::Bool(b) => SetValue::Flag(*b),
                _ => return Err(Error::new(path, "must be true or false")),
            },
            Spec::Whole { min, max } => SetValue::Whole(whole(v, path, min, max)?),
            Spec::Progress => SetValue::Progress(progress(v, path)?),
        })
    }
}

/// Items `append` adds, normalized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Appended {
    Steps(Vec<Step>),
    Hunks(Vec<Hunk>),
    Lines(Vec<String>),
    Series(Vec<Num>),
}

impl ListSpec {
    /// `v`, a non-empty list of what this list takes, normalized: items
    /// unique by id; each list within its own cap.
    pub fn check(self, v: &Value, path: &str, author: Author) -> Result<Appended, Error> {
        Ok(match self {
            ListSpec::Steps => {
                let items = list(v, path, 1, PLAN_STEPS_MAX, "steps", "")?;
                let steps =
                    items.iter().enumerate().map(|(i, s)| step(s, &format!("{path}[{i}]"), author)).collect::<Result<Vec<_>, _>>()?;
                Appended::Steps(steps)
            }
            ListSpec::Hunks => {
                let items = list(v, path, 1, HUNKS_MAX, "hunks", "")?;
                let hunks = items.iter().enumerate().map(|(i, h)| hunk(h, &format!("{path}[{i}]"))).collect::<Result<Vec<_>, _>>()?;
                let lines: usize = hunks.iter().map(|h| h.lines.len()).sum();
                if lines > DIFF_LINES_MAX {
                    return Err(Error::new(path, format!("at most {DIFF_LINES_MAX} lines in all (got {lines})")));
                }
                Appended::Hunks(hunks)
            }
            ListSpec::Lines => Appended::Lines(diff_lines(v, path, 1)?),
            ListSpec::Series => Appended::Series(series(v, path, 1)?),
        })
    }
}

/// A patch's parts, normalized (§1.4): an agent's `patch` op, or the user's
/// edit (§3.3, with no `remove`). They apply in four phases: `remove`;
/// `append` (lists in byte order of their paths, items in array order);
/// `set` (in byte order of the paths); `order`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parts {
    pub append: BTreeMap<Path, Appended>,
    pub order: BTreeMap<Path, Vec<ItemId>>,
    pub remove: Vec<Path>,
    pub set: BTreeMap<Path, SetValue>,
}

impl Parts {
    pub fn is_empty(&self) -> bool {
        self.append.is_empty() && self.order.is_empty() && self.remove.is_empty() && self.set.is_empty()
    }

    /// Whether every path of these parts fits a block of `kind`, written by
    /// `author`, values and all: an edit's admission (§3.3), and the check
    /// the fold runs per path before applying one (B2). The first that
    /// doesn't is the error, named by its path.
    pub fn fits(&self, kind: Kind, author: Author) -> Result<(), Error> {
        let not = |p: &Path| Error::new(p.as_str(), format!("not a path a {kind} takes here"));
        for (p, a) in &self.append {
            let spec = append_spec(kind, author, p).ok_or_else(|| not(p))?;
            spec.check(&serde_json::to_value(a).map_err(|e| Error::new(p.as_str(), e.to_string()))?, p.as_str(), author)?;
        }
        if let Some(p) = self.order.keys().find(|p| !order_spec(kind, author, p)) {
            return Err(not(p));
        }
        if let Some(p) = self.remove.iter().find(|p| !remove_spec(kind, author, p)) {
            return Err(not(p));
        }
        for (p, v) in &self.set {
            let spec = set_spec(kind, author, p).ok_or_else(|| not(p))?;
            spec.check(&serde_json::to_value(v).map_err(|e| Error::new(p.as_str(), e.to_string()))?, p.as_str(), p)?;
        }
        Ok(())
    }

    /// The parts as entries of a map, in byte order: `append`, `order`,
    /// `remove`, `set`, each only when it has any.
    pub(crate) fn write<M: SerializeMap>(&self, m: &mut M) -> Result<(), M::Error> {
        if !self.append.is_empty() {
            m.serialize_entry("append", &self.append)?;
        }
        if !self.order.is_empty() {
            m.serialize_entry("order", &self.order)?;
        }
        if !self.remove.is_empty() {
            m.serialize_entry("remove", &self.remove)?;
        }
        if !self.set.is_empty() {
            m.serialize_entry("set", &self.set)?;
        }
        Ok(())
    }

    fn entries(&self) -> usize {
        usize::from(!self.append.is_empty())
            + usize::from(!self.order.is_empty())
            + usize::from(!self.remove.is_empty())
            + usize::from(!self.set.is_empty())
    }
}

/// The parts alone, as a JSON object (`{"append":…,"set":…}`).
impl Serialize for Parts {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.entries()))?;
        self.write(&mut m)?;
        m.end()
    }
}

/// One path of a patch, as written, before any kind is known.
enum Raw<'a> {
    Append(Path, &'a Value),
    Order(Path, Vec<ItemId>),
    Remove(Path),
    Set(Path, &'a Value),
}

/// A part's entry and where a refusal names it.
struct Entry<'a> {
    raw: Raw<'a>,
    at: String,
}

impl Entry<'_> {
    fn path(&self) -> &Path {
        match &self.raw {
            Raw::Append(p, _) | Raw::Order(p, _) | Raw::Remove(p) | Raw::Set(p, _) => p,
        }
    }

    /// Under `kind`: `Ok(None)` if the kind has no such path, the value's
    /// refusal if it has, else the normalized value.
    fn fit(&self, kind: Kind, author: Author) -> Result<Option<Fitted>, Error> {
        Ok(match &self.raw {
            Raw::Append(p, v) => match append_spec(kind, author, p) {
                Some(spec) => Some(Fitted::Append(spec.check(v, &self.at, author)?)),
                None => None,
            },
            Raw::Order(p, ids) => order_spec(kind, author, p).then(|| Fitted::Order(ids.clone())),
            Raw::Remove(p) => remove_spec(kind, author, p).then_some(Fitted::Remove),
            Raw::Set(p, v) => match set_spec(kind, author, p) {
                Some(spec) => Some(Fitted::Set(spec.check(v, &self.at, p)?)),
                None => None,
            },
        })
    }

    /// The refusal for a path no kind has.
    fn nowhere(&self) -> Error {
        let p = self.path();
        let msg = match &self.raw {
            Raw::Set(..) if matches!(p.head(), "steps" | "hunks" | "series") && p.item().is_none() => {
                format!("{} is a list: append to it, remove its items or order it", quote(p.as_str()))
            }
            Raw::Set(..) => format!("{} is not a field of any kind", quote(p.as_str())),
            Raw::Append(..) => format!("{} is not a list of any kind (steps, hunks, hunks.<id>.lines, series)", quote(p.as_str())),
            Raw::Order(..) => format!("{} is not a list with ids (steps, hunks)", quote(p.as_str())),
            Raw::Remove(..) => format!("{} is not an item of a list with ids (steps.<id>, hunks.<id>)", quote(p.as_str())),
        };
        Error::new(&self.at, msg)
    }
}

enum Fitted {
    Append(Appended),
    Order(Vec<ItemId>),
    Remove,
    Set(SetValue),
}

/// Object keys in byte order, whatever the build's map order.
fn sorted(o: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut out: Vec<_> = o.iter().collect();
    out.sort_by(|a, b| a.0.cmp(b.0));
    out
}

/// A patch's parts in `o` (an op, or an edit's line 2), `author`'s parts
/// only: the other keys are the caller's to check. `path` names the patch
/// in a refusal (`ops[1]`).
pub(crate) fn parse_parts(o: &Map<String, Value>, path: &str, author: Author) -> Result<Parts, Error> {
    let agent = author == Author::Agent;
    let mut entries: Vec<Entry> = Vec::new();

    // `append`: list paths to non-empty lists.
    let mut appended: BTreeSet<String> = BTreeSet::new();
    if let Some(v) = o.get("append") {
        let p = join(path, "append");
        for (k, v) in sorted(object(v, &p, "{list: [items]}")?) {
            let at = join(&p, &key_path(k));
            let Some(lp) = Path::parse(k) else {
                return Err(Error::new(at, "not a path: a list (steps) or an item's list (hunks.h1.lines)"));
            };
            if lp.item().is_some() && lp.field().is_none() {
                return Err(Error::new(at, "names an item; append to its list (steps) or to its lines (hunks.h1.lines)"));
            }
            if !v.is_array() {
                return Err(Error::new(at, "must be a list of what to add"));
            }
            // Items' ids, for the overlap rule; their shape is checked with the kind.
            if lp.item().is_none() {
                for item in v.as_array().into_iter().flatten() {
                    if let Some(id) = item.get("id").and_then(Value::as_str).and_then(ItemId::parse) {
                        let ip = format!("{}.{id}", lp.head());
                        if !appended.insert(ip.clone()) {
                            return Err(Error::new(path, format!("{} is named twice (append)", quote(&ip))));
                        }
                    }
                }
            }
            entries.push(Entry { raw: Raw::Append(lp, v), at });
        }
    }

    // `order`: a list to the ids in order.
    if let Some(v) = o.get("order") {
        let p = join(path, "order");
        for (k, v) in sorted(object(v, &p, "{list: [item ids]}")?) {
            let at = join(&p, &key_path(k));
            let lp = Path::parse(k).filter(|lp| lp.item().is_none());
            let Some(lp) = lp else {
                return Err(Error::new(at, "not a list: order takes a list's name (steps) and its ids"));
            };
            let Some(ids) = v.as_array().filter(|a| !a.is_empty()) else {
                return Err(Error::new(at, "must be a list of item ids, in order"));
            };
            let mut seen: Vec<ItemId> = Vec::with_capacity(ids.len());
            for (i, id) in ids.iter().enumerate() {
                let ip = format!("{at}[{i}]");
                let Some(item) = id.as_str().and_then(ItemId::parse) else {
                    return Err(Error::new(ip, format!("{} is not an item id", json_quote(id))));
                };
                if seen.contains(&item) {
                    return Err(Error::new(ip, format!("\"{item}\" is listed twice")));
                }
                seen.push(item);
            }
            entries.push(Entry { raw: Raw::Order(lp, seen), at });
        }
    }

    // `remove`: items, the agent's only.
    let mut removed: BTreeSet<String> = BTreeSet::new();
    if agent && let Some(v) = o.get("remove") {
        let p = join(path, "remove");
        let Some(items) = v.as_array() else {
            return Err(Error::new(p, "must be a list of items (steps.fix)"));
        };
        for (i, item) in items.iter().enumerate() {
            let at = format!("{p}[{i}]");
            let ip = item.as_str().and_then(Path::parse).filter(|ip| ip.item().is_some() && ip.field().is_none());
            let Some(ip) = ip else {
                return Err(Error::new(at, format!("{} is not an item path (steps.fix)", json_quote(item))));
            };
            if !removed.insert(ip.as_str().to_string()) {
                return Err(Error::new(path, format!("{} is named twice (remove)", quote(ip.as_str()))));
            }
            if appended.contains(ip.as_str()) {
                return Err(Error::new(path, format!("{} is named twice (remove and append)", quote(ip.as_str()))));
            }
            entries.push(Entry { raw: Raw::Remove(ip), at });
        }
    }

    // `set`, then `state` sugar into it.
    let mut set: Vec<Entry> = Vec::new();
    if let Some(v) = o.get("set") {
        let p = join(path, "set");
        for (k, v) in sorted(object(v, &p, "{path: value}")?) {
            let at = join(&p, &key_path(k));
            let Some(sp) = Path::parse(k) else {
                return Err(Error::new(at, "not a path: a field (title), or an item's field (steps.fix.label)"));
            };
            if sp.item().is_some() && sp.field().is_none() {
                return Err(Error::new(at, "names an item; set one of its fields, like steps.fix.label"));
            }
            set.push(Entry { raw: Raw::Set(sp, v), at });
        }
    }
    if agent && let Some(v) = o.get("state") {
        let p = join(path, "state");
        let mut lists: BTreeSet<&str> = BTreeSet::new();
        for (k, w) in sorted(object(v, &p, "{item id: state}")?) {
            let at = join(&p, &key_path(k));
            let Some(item) = ItemId::parse(k) else {
                return Err(Error::new(at, "not an item id"));
            };
            let list = match w.as_str() {
                Some(s) if StepState::parse(s).is_some() => "steps",
                Some(s) if HunkState::parse(s).is_some() => "hunks",
                _ => {
                    return Err(Error::new(
                        at,
                        format!(
                            "{} is not {} ({}) or {} ({})",
                            json_quote(w),
                            StepState::WHAT,
                            StepState::ALL.join(", "),
                            HunkState::WHAT,
                            HunkState::ALL.join(", ")
                        ),
                    ));
                }
            };
            lists.insert(list);
            let sp = Path::of(list, item.as_str(), "state");
            if set.iter().any(|e| e.path() == &sp) {
                return Err(Error::new(path, format!("{} is named twice (state and set)", quote(sp.as_str()))));
            }
            set.push(Entry { raw: Raw::Set(sp, w), at });
        }
        if lists.len() > 1 {
            return Err(Error::new(p, "mixes plan states and hunk states; tick one list per patch"));
        }
    }
    set.sort_by(|a, b| a.path().cmp(b.path()));
    for e in &set {
        let p = e.path();
        if agent && p.head() == "steps" && p.field() == Some("stop") {
            return Err(Error::new(&e.at, "stop is the user's: an agent op can't name it"));
        }
        if let Some(ip) = p.item_path() {
            if removed.contains(&ip) {
                return Err(Error::new(path, format!("{} sets a field of an item the patch removes", quote(p.as_str()))));
            }
            if appended.contains(&ip) {
                return Err(Error::new(path, format!("{} sets a field of an item the patch appends", quote(p.as_str()))));
            }
        }
    }
    for e in &entries {
        if let (Raw::Append(p, _), Some(ip)) = (&e.raw, e.path().item_path()) {
            if removed.contains(&ip) {
                return Err(Error::new(path, format!("{} appends to an item the patch removes", quote(p.as_str()))));
            }
            if appended.contains(&ip) {
                return Err(Error::new(path, format!("{} appends to an item the patch appends", quote(p.as_str()))));
            }
        }
    }
    entries.extend(set);
    if entries.is_empty() {
        let parts = if agent { "set, append, remove, order or state" } else { "set, append or order" };
        return Err(Error::new(path, format!("a patch needs {parts}")));
    }
    fit_kinds(&entries, path, author)
}

/// The parts, if one kind takes every entry (see the module docs); else
/// the refusal that says best why none does.
fn fit_kinds(entries: &[Entry], path: &str, author: Author) -> Result<Parts, Error> {
    let mut live: Vec<Kind> = Kind::ALL.to_vec();
    let mut shapes: Vec<Vec<Kind>> = Vec::with_capacity(entries.len());
    let mut parts = Parts::default();
    for (i, e) in entries.iter().enumerate() {
        let fits: Vec<(Kind, Result<Option<Fitted>, Error>)> = Kind::ALL.iter().map(|&k| (k, e.fit(k, author))).collect();
        let has: Vec<Kind> = fits.iter().filter(|(_, r)| !matches!(r, Ok(None))).map(|(k, _)| *k).collect();
        if has.is_empty() {
            return Err(e.nowhere());
        }
        let takes: Vec<Kind> = fits.iter().filter(|(_, r)| matches!(r, Ok(Some(_)))).map(|(k, _)| *k).collect();
        if takes.is_empty() {
            return Err(refusal(e, &has, author));
        }
        shapes.push(has.clone());
        let next: Vec<Kind> = live.iter().copied().filter(|k| takes.contains(k)).collect();
        if next.is_empty() {
            // Every kind that has all the paths so far, by shape.
            let common: Vec<Kind> = Kind::ALL.iter().copied().filter(|k| shapes.iter().all(|s| s.contains(k))).collect();
            let Some(&k) = common.last() else {
                let q = shapes[..i].iter().position(|s| !s.iter().any(|k| has.contains(k))).unwrap_or(0);
                return Err(Error::new(
                    path,
                    format!("{} and {} are paths of different kinds", quote(entries[q].path().as_str()), quote(e.path().as_str())),
                ));
            };
            // One of them refuses its value under `k`, or `k` would be live.
            for e in &entries[..=i] {
                e.fit(k, author)?;
            }
            unreachable!("a kind that takes every entry is live");
        }
        live = next;
        // Normalizing never depends on the kind (module docs): any taker's value.
        let Some((_, Ok(Some(v)))) = fits.into_iter().find(|(_, r)| matches!(r, Ok(Some(_)))) else {
            unreachable!("takes is not empty");
        };
        match v {
            Fitted::Append(a) => {
                parts.append.insert(e.path().clone(), a);
            }
            Fitted::Order(ids) => {
                parts.order.insert(e.path().clone(), ids);
            }
            Fitted::Remove => parts.remove.push(e.path().clone()),
            Fitted::Set(v) => {
                parts.set.insert(e.path().clone(), v);
            }
        }
    }
    Ok(parts)
}

/// Why no kind takes `e`'s value, when `has` (the kinds with its path) all
/// refuse it: a `state` word against every such kind's words; anything
/// else as the last of them says (for `text`, a note's bound, the widest).
fn refusal(e: &Entry, has: &[Kind], author: Author) -> Error {
    if let (Raw::Set(p, Value::String(s)), true) = (&e.raw, has.len() > 1)
        && p.as_str() == "state"
    {
        let lists: Vec<String> = has
            .iter()
            .filter_map(|&k| match set_spec(k, author, p) {
                Some(Spec::State { all, .. }) => Some(format!("a {k} ({})", all.join(", "))),
                _ => None,
            })
            .collect();
        let (last, rest) = lists.split_last().expect("has is not empty");
        return Error::new(&e.at, format!("{} is not a state of {} or {last}", quote(s), rest.join(", ")));
    }
    let k = *has.last().expect("has is not empty");
    match e.fit(k, author) {
        Err(err) => err,
        Ok(_) => unreachable!("every kind in has refuses"),
    }
}
