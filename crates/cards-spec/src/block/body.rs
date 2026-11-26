//! The kinds' bodies (§1.5): their types, defaults and bounds.
//!
//! Every string is validated as cards' are (`doc::string`: line endings,
//! then a control or bidi character refused, then the raw bound) and
//! normalized by `ui_sanitize`. An optional string left empty is absent.
//! A body's cap is its normalized compact JSON, so a body accepted at a
//! `put` is measured as the fold measures it after a patch.
//!
//! **Readings** (§1.5 sketches the bodies; these complete them):
//! - defaults: a step's `state` is `todo`, a hunk's `open`, a status's
//!   `working`, a metric's `neutral`. A diff and a file give their `state`:
//!   neither has a neutral one;
//! - an agent's hunk (in a `put`, a `replace` or an `append`) is `open`:
//!   `kept` and `dropped` are the user's decisions (§3.4), so an agent
//!   that writes one is refused;
//! - a hunk's `at` is one line of at most [`HUNK_AT_MAX`] characters, and a
//!   hunk has 1 to [`DIFF_LINES_MAX`] lines; a diff line is one line, `""`
//!   allowed (a blank context line);
//! - a diff gives `hunks` or `from:"edits"`, not both, or neither (a header);
//! - a status's `progress` is whole numbers, `of` at least 1;
//! - a file's `line` is a whole number of at least 1, as a JSON integer:
//!   any a `u64` holds, no product ceiling (§1.5, amended at B1);
//! - series numbers are [`Num`]s: any finite number, no ceiling.

use std::collections::BTreeSet;

use super::*;

// ── Bounds (§1.5) ─────────────────────────────────────────────────────────

pub const PLAN_STEPS_MIN: usize = 1;
pub const PLAN_STEPS_MAX: usize = 20;
pub const STEP_LABEL_MAX: usize = 100;
pub const STEP_NOTE_MAX: usize = 80;
pub const PLAN_MAX_BYTES: usize = 4 * 1024;
pub const STATUS_TEXT_MAX: usize = 80;
pub const STATUS_DETAIL_MAX: usize = 120;
/// `n ≤ of ≤ 10⁶`.
pub const PROGRESS_MAX: u32 = 1_000_000;
pub const STATUS_MAX_BYTES: usize = 512;
pub const DIFF_FILE_MAX: usize = 200;
pub const HUNKS_MIN: usize = 1;
pub const HUNKS_MAX: usize = 6;
/// Lines in all a diff's hunks.
pub const DIFF_LINES_MAX: usize = 120;
pub const DIFF_LINE_MAX: usize = 160;
pub const HUNK_AT_MAX: usize = 80;
pub const DIFF_MAX_BYTES: usize = 6 * 1024;
pub const NOTE_TEXT_MAX: usize = 1500;
pub const NOTE_LINES_MAX: usize = 30;
pub const NOTE_MAX_BYTES: usize = 2 * 1024;
pub const METRIC_LABEL_MAX: usize = 60;
/// `value`, `was` and `unit`: the agent formats them.
pub const METRIC_VALUE_MAX: usize = 16;
pub const SERIES_MAX: usize = 32;
pub const METRIC_MAX_BYTES: usize = 1024;
pub const FILE_PATH_MAX: usize = 200;
pub const FILE_NOTE_MAX: usize = 80;
pub const FILE_MAX_BYTES: usize = 512;
/// A group's body is the desk's (8 member ids), never the agent's.
pub const GROUP_MAX_BYTES: usize = 512;
/// Below this, a whole series number is written as an integer ([`Num`]).
const NUM_PLAIN_BELOW: f64 = 1e16;

macro_rules! states {
    ($(#[$m:meta])* $name:ident, $what:literal, [$($v:ident = $s:literal),+ $(,)?]) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($v),+ }

        impl $name {
            /// Its words, in the design's order.
            pub const ALL: &'static [&'static str] = &[$($s),+];
            /// What a refusal calls one: `a plan state`.
            pub const WHAT: &'static str = $what;

            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($s => Some(Self::$v),)+
                    _ => None,
                }
            }

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$v => $s),+
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    };
}

states!(
    /// A plan step's state.
    StepState,
    "a plan state",
    [Todo = "todo", Doing = "doing", Done = "done", Failed = "failed", Skipped = "skipped"]
);
states!(StatusState, "a status state", [Working = "working", Waiting = "waiting", Blocked = "blocked", Done = "done"]);
states!(DiffState, "a diff state", [Editing = "editing", Proposed = "proposed", Written = "written", Reverted = "reverted"]);
states!(HunkState, "a hunk state", [Open = "open", Kept = "kept", Dropped = "dropped"]);
states!(MetricState, "a metric state", [Ok = "ok", Warn = "warn", Bad = "bad", Neutral = "neutral"]);
states!(FileState, "a file state", [Reading = "reading", Editing = "editing", Written = "written", Done = "done"]);

/// One of a state enum's words at `path`, or the refusal that lists them.
pub(crate) fn state_word(v: &Value, path: &str, all: &'static [&'static str], what: &str) -> Result<&'static str, Error> {
    let words = all.join(", ");
    match v.as_str() {
        Some(s) => all.iter().copied().find(|w| *w == s).ok_or_else(|| Error::new(path, format!("{} is not {what} ({words})", quote(s)))),
        None => Err(Error::new(path, format!("must be {what} ({words})"))),
    }
}

fn parse_state<T>(
    v: Option<&Value>,
    path: &str,
    all: &'static [&'static str],
    what: &str,
    parse: fn(&str) -> Option<T>,
    default: Option<T>,
) -> Result<T, Error> {
    match (v, default) {
        (None, Some(d)) => Ok(d),
        (None, None) => Err(Error::new(path, format!("required: {what} ({})", all.join(", ")))),
        (Some(v), _) => Ok(parse(state_word(v, path, all, what)?).expect("a listed word parses")),
    }
}

/// A series number (§1.5): any finite number, `-0` as `0`. JSON has no
/// other kind, so every number is one; the series' count (32) and the
/// metric's 1 KiB bound how many.
///
/// **Canonical form:** as serde_json writes an `f64`, the shortest decimal
/// that reads back, without a trailing `.0`. So a whole number below 10¹⁶
/// is an integer (`1.0` is `1`, `1e15` is `1000000000000000`), from 10¹⁶ it
/// takes an exponent (`1e+16`, `1.5e+300`), and any other number is as it
/// reads (`1.5`, `0.00012`, `1e-7`). `scripts/oracle.py` writes the same
/// bytes, and numbers read correctly rounded (`float_roundtrip`), so the
/// form reads back to the same number.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Num(f64);

// Never NaN (`Num::parse` refuses it), so equality is total.
impl Eq for Num {}

impl Num {
    pub fn parse(v: &Value) -> Option<Self> {
        let x = v.as_f64()?;
        x.is_finite().then_some(Num(if x == 0.0 { 0.0 } else { x }))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Below 10¹⁶ a whole f64 is exact in an i64; serde_json would add `.0`.
        if self.0.fract() == 0.0 && self.0.abs() < NUM_PLAIN_BELOW { s.serialize_i64(self.0 as i64) } else { s.serialize_f64(self.0) }
    }
}

pub(crate) fn num(v: &Value, path: &str) -> Result<Num, Error> {
    Num::parse(v).ok_or_else(|| Error::new(path, format!("must be a number (got {})", crate::doc::json_quote(v))))
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A plan's step. `stop` is the user's (set by an edit, §3.4): it is never
/// in an agent's op, and written only when true.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Step {
    pub id: ItemId,
    pub label: String,
    pub state: StepState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub stop: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlanBody {
    pub steps: Vec<Step>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub n: u32,
    pub of: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StatusBody {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    pub state: StatusState,
}

/// A diff's hunk: a proposal the user may keep or drop (§3.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hunk {
    pub id: ItemId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    pub lines: Vec<String>,
    pub state: HunkState,
}

/// `from:"edits"`: the desk draws the session's edits to the file (§1.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FromEdits;

impl Serialize for FromEdits {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("edits")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiffBody {
    pub file: String,
    pub state: DiffState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<FromEdits>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hunks: Vec<Hunk>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NoteBody {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MetricBody {
    pub label: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<Num>,
    pub state: MetricState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileBody {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub state: FileState,
}

/// A group's body: its members, in order (§1.3). Made by the fold's
/// `merge`, never written by the agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GroupBody {
    pub members: Vec<BlockId>,
}

/// A block's body, normalized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Body {
    Plan(PlanBody),
    Status(StatusBody),
    Diff(DiffBody),
    Note(NoteBody),
    Metric(MetricBody),
    File(FileBody),
    Group(GroupBody),
}

impl Body {
    pub fn kind(&self) -> Kind {
        match self {
            Body::Plan(_) => Kind::Plan,
            Body::Status(_) => Kind::Status,
            Body::Diff(_) => Kind::Diff,
            Body::Note(_) => Kind::Note,
            Body::Metric(_) => Kind::Metric,
            Body::File(_) => Kind::File,
            Body::Group(_) => Kind::Group,
        }
    }

    /// Its normalized compact JSON's length: what the kind's cap bounds.
    pub fn bytes(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |v| v.len())
    }

    /// An agent's body for `kind`, validated and normalized: a `put`'s or a
    /// `replace`'s (§1.5). `path` names it in a refusal.
    pub fn parse(kind: Kind, v: &Value, path: &str) -> Result<Body, Error> {
        let shape = match kind {
            Kind::Plan => "{steps}",
            Kind::Status => "{text, detail?, progress?, state?}",
            Kind::Diff => "{file, state, from? or hunks?}",
            Kind::Note => "{text}",
            Kind::Metric => "{label, value, was?, unit?, series?, state?}",
            Kind::File => "{path, line?, note?, state}",
            Kind::Group => return Err(Error::new(path, "a group is made by merge, never written")),
        };
        let o = object(v, path, shape)?;
        let body = match kind {
            Kind::Plan => Body::Plan(plan(o, path)?),
            Kind::Status => Body::Status(status(o, path)?),
            Kind::Diff => Body::Diff(diff(o, path)?),
            Kind::Note => Body::Note(note(o, path)?),
            Kind::Metric => Body::Metric(metric(o, path)?),
            Kind::File => Body::File(file(o, path)?),
            Kind::Group => unreachable!("refused above"),
        };
        let n = body.bytes();
        let max = kind.max_bytes();
        if n > max {
            return Err(Error::new(
                path,
                format!("over {max} bytes for a {kind} (got {n}, normalized); shorten it or split it over blocks"),
            ));
        }
        Ok(body)
    }
}

/// A list of `min..=max` items at `path`.
pub(crate) fn list<'a>(v: &'a Value, path: &str, min: usize, max: usize, what: &str, advice: &str) -> Result<&'a [Value], Error> {
    let Some(items) = v.as_array() else {
        return Err(Error::new(path, format!("must be a list of {min} to {max} {what}")));
    };
    if items.len() < min {
        return Err(Error::new(path, format!("at least {min} {what}")));
    }
    if items.len() > max {
        return Err(Error::new(path, format!("at most {max} {what} (got {}){advice}", items.len())));
    }
    Ok(items)
}

/// An item's id, by `author`: the agent's never starts `u-`, the user's is
/// always one [`ItemId::mint`] makes.
pub(crate) fn item_id(v: Option<&Value>, path: &str, author: Author) -> Result<ItemId, Error> {
    let Some(v) = v else {
        return Err(Error::new(path, "required"));
    };
    let Some(s) = v.as_str() else {
        return Err(Error::new(path, "must be an item id"));
    };
    let Some(id) = ItemId::parse(s) else {
        return Err(Error::new(
            path,
            format!("{} must match ^[a-z0-9][a-z0-9_-]{{0,23}}$: a lowercase letter or digit, then up to 23 of them, _ or -", quote(s)),
        ));
    };
    match author {
        Author::Agent if id.is_user() => {
            Err(Error::new(path, format!("{} starts with u-, which is the user's; pick another id", quote(s))))
        }
        Author::User if !id.is_minted() => Err(Error::new(path, format!("{} is not a user's id (u- and 6 of a-z2-7)", quote(s)))),
        _ => Ok(id),
    }
}

/// Item ids, none twice in their list.
fn unique<'a>(ids: impl Iterator<Item = &'a ItemId>, path: &str) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    for (i, id) in ids.enumerate() {
        if !seen.insert(id) {
            return Err(Error::new(format!("{path}[{i}].id"), format!("\"{id}\" is used twice; item ids are unique in their list")));
        }
    }
    Ok(())
}

fn plan(o: &Map<String, Value>, path: &str) -> Result<PlanBody, Error> {
    known(o, &["steps"], path, "a plan takes steps")?;
    let p = join(path, "steps");
    let Some(v) = o.get("steps") else {
        return Err(Error::new(p, format!("required: {PLAN_STEPS_MIN} to {PLAN_STEPS_MAX} steps")));
    };
    let items = list(v, &p, PLAN_STEPS_MIN, PLAN_STEPS_MAX, "steps", "; split the plan or fold done steps into a note")?;
    let steps = items.iter().enumerate().map(|(i, s)| step(s, &format!("{p}[{i}]"), Author::Agent)).collect::<Result<Vec<_>, _>>()?;
    unique(steps.iter().map(|s| &s.id), &p)?;
    Ok(PlanBody { steps })
}

/// A step, the agent's (a put's, or appended) or the user's (appended by an
/// edit: a `u-` id, a label and the state `todo`, §3.4).
pub(crate) fn step(v: &Value, path: &str, author: Author) -> Result<Step, Error> {
    let o = object(v, path, "{id, label, state?, note?}")?;
    if o.contains_key("stop") && author == Author::Agent {
        return Err(Error::new(join(path, "stop"), "stop is the user's: an agent op can't name it"));
    }
    match author {
        Author::Agent => known(o, &["id", "label", "state", "note"], path, "a step takes id, label, state and note")?,
        Author::User => known(o, &["id", "label", "state"], path, "a user's step takes id, label and state")?,
    }
    let id = item_id(o.get("id"), &join(path, "id"), author)?;
    let Some(label) = o.get("label") else {
        return Err(Error::new(join(path, "label"), "required"));
    };
    let label = string(label, &join(path, "label"), Lines::Single, 1, STEP_LABEL_MAX, "; put the rest in its note")?;
    let state =
        parse_state(o.get("state"), &join(path, "state"), StepState::ALL, StepState::WHAT, StepState::parse, Some(StepState::Todo))?;
    if author == Author::User && state != StepState::Todo {
        return Err(Error::new(join(path, "state"), "a step the user adds is todo"));
    }
    let note = optional(o, "note", path, STEP_NOTE_MAX)?;
    Ok(Step { id, label, state, note, stop: false })
}

fn status(o: &Map<String, Value>, path: &str) -> Result<StatusBody, Error> {
    known(o, &["text", "detail", "progress", "state"], path, "a status takes text, detail, progress and state")?;
    let Some(text) = o.get("text") else {
        return Err(Error::new(join(path, "text"), "required"));
    };
    let text = string(text, &join(path, "text"), Lines::Single, 1, STATUS_TEXT_MAX, "; put the rest in detail")?;
    let detail = optional(o, "detail", path, STATUS_DETAIL_MAX)?;
    let progress = o.get("progress").map(|v| progress(v, &join(path, "progress"))).transpose()?;
    let state = parse_state(
        o.get("state"),
        &join(path, "state"),
        StatusState::ALL,
        StatusState::WHAT,
        StatusState::parse,
        Some(StatusState::Working),
    )?;
    Ok(StatusBody { text, detail, progress, state })
}

/// `{"n":3,"of":7}`: `0 ≤ n ≤ of`, `1 ≤ of ≤ 10⁶`.
pub(crate) fn progress(v: &Value, path: &str) -> Result<Progress, Error> {
    let o = object(v, path, "{n, of}")?;
    known(o, &["n", "of"], path, "progress takes n and of")?;
    let (Some(n), Some(of)) = (o.get("n"), o.get("of")) else {
        return Err(Error::new(path, "needs n and of: {\"n\":3,\"of\":7}"));
    };
    let n = whole(n, &join(path, "n"), 0, PROGRESS_MAX.into())? as u32;
    let of = whole(of, &join(path, "of"), 1, PROGRESS_MAX.into())? as u32;
    if n > of {
        return Err(Error::new(join(path, "n"), format!("{n} is more than of ({of})")));
    }
    Ok(Progress { n, of })
}

fn diff(o: &Map<String, Value>, path: &str) -> Result<DiffBody, Error> {
    known(o, &["file", "state", "from", "hunks"], path, "a diff takes file, state, from and hunks")?;
    let Some(file) = o.get("file") else {
        return Err(Error::new(join(path, "file"), "required"));
    };
    let file = string(file, &join(path, "file"), Lines::Single, 1, DIFF_FILE_MAX, "")?;
    let state = parse_state(o.get("state"), &join(path, "state"), DiffState::ALL, DiffState::WHAT, DiffState::parse, None)?;
    let from = match o.get("from") {
        None => None,
        Some(v) if v == "edits" => Some(FromEdits),
        Some(v) => {
            return Err(Error::new(join(path, "from"), format!("{} is not a source; from takes \"edits\"", crate::doc::json_quote(v))));
        }
    };
    let hunks = match o.get("hunks") {
        None => Vec::new(),
        Some(_) if from.is_some() => {
            return Err(Error::new(join(path, "hunks"), "from:\"edits\" draws the session's edits; give hunks or from, not both"));
        }
        Some(v) => {
            let p = join(path, "hunks");
            let items = list(v, &p, HUNKS_MIN, HUNKS_MAX, "hunks", "; propose fewer at once")?;
            let hunks = items.iter().enumerate().map(|(i, h)| hunk(h, &format!("{p}[{i}]"))).collect::<Result<Vec<_>, _>>()?;
            unique(hunks.iter().map(|h| &h.id), &p)?;
            let lines: usize = hunks.iter().map(|h| h.lines.len()).sum();
            if lines > DIFF_LINES_MAX {
                return Err(Error::new(p, format!("at most {DIFF_LINES_MAX} lines in all (got {lines})")));
            }
            hunks
        }
    };
    Ok(DiffBody { file, state, from, hunks })
}

pub(crate) fn hunk(v: &Value, path: &str) -> Result<Hunk, Error> {
    let o = object(v, path, "{id, at?, lines, state?}")?;
    known(o, &["id", "at", "lines", "state"], path, "a hunk takes id, at, lines and state")?;
    let id = item_id(o.get("id"), &join(path, "id"), Author::Agent)?;
    let at = optional(o, "at", path, HUNK_AT_MAX)?;
    let p = join(path, "lines");
    let Some(v) = o.get("lines") else {
        return Err(Error::new(p, "required: the hunk's lines"));
    };
    let lines = diff_lines(v, &p, 1)?;
    let state = agent_hunk_state(o.get("state"), &join(path, "state"))?;
    Ok(Hunk { id, at, lines, state })
}

/// An agent's hunk is `open`, given or by default: a keep or a drop is
/// the user's to give (§3.4).
fn agent_hunk_state(v: Option<&Value>, path: &str) -> Result<HunkState, Error> {
    match v.map(|v| (v, v.as_str().and_then(HunkState::parse))) {
        None | Some((_, Some(HunkState::Open))) => Ok(HunkState::Open),
        Some((v, Some(_))) => Err(Error::new(
            path,
            format!("{} is the user's keep or drop; an agent's hunk is open, so leave state out", crate::doc::json_quote(v)),
        )),
        Some((v, None)) => {
            Err(Error::new(path, format!("{} is not a hunk state; an agent's hunk is open, so leave state out", crate::doc::json_quote(v))))
        }
    }
}

/// `min..=120` diff lines, each one line of at most 160 characters.
pub(crate) fn diff_lines(v: &Value, path: &str, min: usize) -> Result<Vec<String>, Error> {
    let items = list(v, path, min, DIFF_LINES_MAX, "lines", "")?;
    items.iter().enumerate().map(|(i, l)| string(l, &format!("{path}[{i}]"), Lines::Single, 0, DIFF_LINE_MAX, "")).collect()
}

/// A note's text: at most 1500 characters on at most 30 lines.
pub(crate) fn note_text(v: &Value, path: &str) -> Result<String, Error> {
    let text = string(v, path, Lines::Multi, 1, NOTE_TEXT_MAX, "; split it over notes")?;
    let lines = text.split('\n').count();
    if lines > NOTE_LINES_MAX {
        return Err(Error::new(path, format!("at most {NOTE_LINES_MAX} lines (got {lines})")));
    }
    Ok(text)
}

fn note(o: &Map<String, Value>, path: &str) -> Result<NoteBody, Error> {
    known(o, &["text"], path, "a note takes text")?;
    let Some(text) = o.get("text") else {
        return Err(Error::new(join(path, "text"), "required"));
    };
    Ok(NoteBody { text: note_text(text, &join(path, "text"))? })
}

fn metric(o: &Map<String, Value>, path: &str) -> Result<MetricBody, Error> {
    known(o, &["label", "value", "was", "unit", "series", "state"], path, "a metric takes label, value, was, unit, series and state")?;
    let (Some(label), Some(value)) = (o.get("label"), o.get("value")) else {
        let missing = if o.contains_key("label") { "value" } else { "label" };
        return Err(Error::new(join(path, missing), "required"));
    };
    let label = string(label, &join(path, "label"), Lines::Single, 1, METRIC_LABEL_MAX, "")?;
    let value = string(value, &join(path, "value"), Lines::Single, 1, METRIC_VALUE_MAX, "; format it shorter")?;
    let was = optional(o, "was", path, METRIC_VALUE_MAX)?;
    let unit = optional(o, "unit", path, METRIC_VALUE_MAX)?;
    let series = match o.get("series") {
        None => Vec::new(),
        Some(v) => series(v, &join(path, "series"), 0)?,
    };
    let state = parse_state(
        o.get("state"),
        &join(path, "state"),
        MetricState::ALL,
        MetricState::WHAT,
        MetricState::parse,
        Some(MetricState::Neutral),
    )?;
    Ok(MetricBody { label, value, was, unit, series, state })
}

/// `min..=32` numbers.
pub(crate) fn series(v: &Value, path: &str, min: usize) -> Result<Vec<Num>, Error> {
    let items = list(v, path, min, SERIES_MAX, "numbers", "; an append rolls off the oldest")?;
    items.iter().enumerate().map(|(i, x)| num(x, &format!("{path}[{i}]"))).collect()
}

fn file(o: &Map<String, Value>, path: &str) -> Result<FileBody, Error> {
    known(o, &["path", "line", "note", "state"], path, "a file takes path, line, note and state")?;
    let Some(p) = o.get("path") else {
        return Err(Error::new(join(path, "path"), "required"));
    };
    let file_path = string(p, &join(path, "path"), Lines::Single, 1, FILE_PATH_MAX, "")?;
    let line = o.get("line").map(|v| whole(v, &join(path, "line"), 1, u64::MAX)).transpose()?;
    let note = optional(o, "note", path, FILE_NOTE_MAX)?;
    let state = parse_state(o.get("state"), &join(path, "state"), FileState::ALL, FileState::WHAT, FileState::parse, None)?;
    Ok(FileBody { path: file_path, line, note, state })
}
