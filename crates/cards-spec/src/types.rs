//! The card's data and its bounds (§1.4, §5.3), from #75's `types.rs`. The
//! serde shapes are strict (`deny_unknown_fields`): a doc read back must be
//! exactly one this crate writes, and then go through
//! [`super::doc::revalidate`].

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The catalog this crate implements: `cards/1` in a result's envelope
/// (§1.5). It names the schema and the normalizer together, so a change to
/// either is a new catalog, and a major version of this crate.
pub const CATALOG: u32 = 1;

// ── Bounds (§1.4, §5.3). Each is tested at N and N+1. ────────────────────
// Character bounds count Unicode scalar values of the raw string (after
// `\r\n` / `\r` become `\n`), as JSON Schema does.

/// A card input, serialized compactly. Refused, never truncated.
pub const INPUT_MAX_BYTES: usize = 8 * 1024;
/// Nesting of an input, scalars counted: `{"options":[{"label":"x"}]}` is 4.
pub const INPUT_MAX_DEPTH: usize = 4;
/// What normalizing can add to an input of at most [`INPUT_MAX_BYTES`]:
/// - `{"label":…}` around each of 9 string options: 9 × 10;
/// - `,"multi":false`: 14;
/// - a text field with no `type`, `value` or `optional`:
///   `"type":"text","value":"","optional":false,` is 42, 5 fields;
/// - a string default becoming `[…]` with `multi`: 2;
/// - a default typed in another case and stored in the label's spelling.
///   Case folding shrinks a character by at most 2 bytes (U+212A KELVIN
///   SIGN, 3 bytes, folds to `k`), so ≤ 2 × 60 for each of 9 labels.
///
/// Sanitizing never grows a string (it drops characters; `\t` and a folded
/// `\n` are 2 bytes in JSON and become a 1-byte space), nor does an empty
/// optional string becoming absent.
pub const NORMALIZE_GROWTH_MAX: usize = 9 * 10 + 14 + 5 * 42 + 2 + 9 * 2 * 60;
/// A normalized doc, serialized compactly: what an accepted input can
/// become. Create and `revalidate` both measure the doc against it, so a
/// doc that was accepted always revalidates (Shady #439 H1).
pub const DOC_MAX_BYTES: usize = INPUT_MAX_BYTES + NORMALIZE_GROWTH_MAX;
pub const QUESTION_MAX: usize = 120;
pub const DETAIL_MAX: usize = 1500;
pub const DETAIL_MAX_LINES: usize = 30;
pub const OPTIONS_MIN: usize = 2;
pub const OPTIONS_MAX: usize = 9;
pub const OPTION_LABEL_MAX: usize = 60;
pub const HINT_MAX: usize = 120;
pub const NOTE_PROMPT_MAX: usize = 80;
pub const DEFAULT_MAX: usize = 9;
pub const FIELDS_MIN: usize = 1;
pub const FIELDS_MAX: usize = 5;
/// `^[a-z][a-z0-9_]{0,23}$`
pub const FIELD_ID_MAX: usize = 24;
pub const FIELD_LABEL_MAX: usize = 60;
pub const PLACEHOLDER_MAX: usize = 80;
/// A `text` value: the agent's prefill and the user's answer.
pub const TEXT_MAX: usize = 200;
/// A `long` value: the agent's prefill and the user's answer.
pub const LONG_MAX: usize = 2000;
/// A note typed for a `note` option.
pub const NOTE_MIN: usize = 1;
pub const NOTE_MAX: usize = 500;
/// Answer values, serialized compactly (§5.3).
pub const ANSWER_MAX_BYTES: usize = 8 * 1024;
/// Nesting of answer values, scalars counted: `{"fields":{"f":"x"}}` is 3.
pub const ANSWER_MAX_DEPTH: usize = 3;
/// A result's envelope line, in UTF-8 bytes (§1.5). Only its question is
/// cut to fit; the id, catalog and digest never are.
pub const ENVELOPE_MAX_BYTES: usize = 256;
/// An answer message's first line, in characters (§3.3).
pub const MESSAGE_LINE_MAX: usize = 200;
/// One piece of that line's summary (a label, `Label: value`), in
/// characters.
pub const SUMMARY_PIECE_MAX: usize = 40;

/// Whether a field is one line (question, labels, hints, placeholders, text
/// values) or many (`detail`, `long` values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lines {
    Single,
    Multi,
}

// ── The doc (§1.4) ───────────────────────────────────────────────────────

/// One normalized question. `wait` is the call's mode, not part of the doc
/// (the digest covers both, [`crate::digest`]). The fields are in schema
/// order, which is the order they serialize in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardDoc {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<CardOption>,
    #[serde(default)]
    pub multi: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<DefaultChoice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<CardField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The prompt of the free-text note this option asks for when picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The agent's recommendation: one label, or (with `multi`) a list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DefaultChoice {
    One(String),
    Many(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardField {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: FieldKind,
    /// The prefill: `""` / `false` when the agent gave none.
    pub value: FieldValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    pub optional: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Long,
    Check,
}

impl FieldKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::Long => "long",
            FieldKind::Check => "check",
        }
    }

    /// The bound on a `text` / `long` value; `None` for a check.
    pub fn max_chars(self) -> Option<usize> {
        match self {
            FieldKind::Text => Some(TEXT_MAX),
            FieldKind::Long => Some(LONG_MAX),
            FieldKind::Check => None,
        }
    }

    pub fn lines(self) -> Lines {
        if self == FieldKind::Long { Lines::Multi } else { Lines::Single }
    }
}

/// How a card is answered (§1.4), derived from the doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `options`, not `multi`, no `fields`: a click or digit answers at once.
    Quick,
    /// Anything else: a Send button.
    Form,
}

impl CardDoc {
    pub fn mode(&self) -> Mode {
        if !self.options.is_empty() && !self.multi && self.fields.is_empty() { Mode::Quick } else { Mode::Form }
    }

    /// The option with exactly this label.
    pub fn option(&self, label: &str) -> Option<&CardOption> {
        self.options.iter().find(|o| o.label == label)
    }

    /// The field with this id.
    pub fn field(&self, id: &str) -> Option<&CardField> {
        self.fields.iter().find(|f| f.id == id)
    }
}

// ── Answers (§1.4, §5.3) ─────────────────────────────────────────────────

/// A field's value: a prefill in the doc, or the user's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    Text(String),
    Check(bool),
}

/// The chosen label(s).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Choice {
    One(String),
    Many(Vec<String>),
}

/// Exactly what the model gets. A skip is the empty answer `{}`.
///
/// Serializes with its keys in a fixed order (`choice`, `note`, `fields`)
/// and its fields in the doc's order, whatever serde_json's map type, so
/// the model reads what the user read. Deserializing keeps the order it
/// reads and refuses unknown or repeated keys and repeated field ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub choice: Option<Choice>,
    pub note: Option<String>,
    /// Every field of the doc, in the doc's order.
    pub fields: Option<Vec<(String, FieldValue)>>,
}

impl Answer {
    /// The answer of a skip.
    pub fn skip() -> Self {
        Self::default()
    }

    pub fn is_skip(&self) -> bool {
        *self == Self::default()
    }

    /// The labels chosen, in order.
    pub fn chosen(&self) -> Vec<&str> {
        match &self.choice {
            None => Vec::new(),
            Some(Choice::One(l)) => vec![l.as_str()],
            Some(Choice::Many(ls)) => ls.iter().map(String::as_str).collect(),
        }
    }

    /// Compact JSON, keys in answer order: an answer message's line 2.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an answer always serializes")
    }
}

impl Serialize for Answer {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let len = self.choice.is_some() as usize + self.note.is_some() as usize + self.fields.is_some() as usize;
        let mut m = s.serialize_map(Some(len))?;
        if let Some(c) = &self.choice {
            m.serialize_entry("choice", c)?;
        }
        if let Some(n) = &self.note {
            m.serialize_entry("note", n)?;
        }
        if let Some(f) = &self.fields {
            m.serialize_entry("fields", &OrderedFields(f))?;
        }
        m.end()
    }
}

struct OrderedFields<'a>(&'a [(String, FieldValue)]);

impl Serialize for OrderedFields<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for Answer {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct AnswerVisitor;
        impl<'de> Visitor<'de> for AnswerVisitor {
            type Value = Answer;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an answer object {choice?, note?, fields?}")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Answer, A::Error> {
                let mut a = Answer::default();
                while let Some(k) = m.next_key::<String>()? {
                    let dup = match k.as_str() {
                        "choice" => a.choice.replace(m.next_value()?).is_some(),
                        "note" => a.note.replace(m.next_value()?).is_some(),
                        "fields" => a.fields.replace(m.next_value::<FieldList>()?.0).is_some(),
                        _ => return Err(de::Error::custom("unknown key; an answer has choice, note and fields")),
                    };
                    if dup {
                        return Err(de::Error::custom(format!("duplicate key {k}")));
                    }
                }
                Ok(a)
            }
        }
        d.deserialize_map(AnswerVisitor)
    }
}

struct FieldList(Vec<(String, FieldValue)>);

impl<'de> Deserialize<'de> for FieldList {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = FieldList;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of field values")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<FieldList, A::Error> {
                let mut out: Vec<(String, FieldValue)> = Vec::new();
                while let Some((k, v)) = m.next_entry::<String, FieldValue>()? {
                    if out.iter().any(|(id, _)| *id == k) {
                        return Err(de::Error::custom("duplicate field id"));
                    }
                    out.push((k, v));
                }
                Ok(FieldList(out))
            }
        }
        d.deserialize_map(FieldsVisitor)
    }
}
