//! The answer message (§3.3): what the desk sends as the user's message,
//! and what a fold reads back as a card's answer.
//!
//! ```text
//! [card c4f9x2a] Which approach? → Refactor the parser
//! {"choice":"Refactor the parser"}
//! ```
//!
//! **Line 1, for people:** `[card <id>]`, ` changed]` or ` skipped]`, a
//! space, the question, ` → `, and a summary: the chosen labels joined with
//! `, ` (`(none)` for an empty multi choice), then the note as
//! `<prompt>: <note>`, then `Label: value` for each field (`yes`/`no` for a
//! check, `—` for an empty optional one), joined with ` · `; a skip reads
//! `(skipped)`. Each piece is sanitized (single line) and cut at
//! [`SUMMARY_PIECE_MAX`] characters with `…`, and the line at
//! [`MESSAGE_LINE_MAX`].
//!
//! **Line 2, for machines:** the values as compact JSON ([`Answer::to_json`],
//! keys in a fixed order, fields in doc order, so the bytes are the same
//! with and without `preserve_order`). A skip is `{}`.
//!
//! **Reading** ([`parse`]): a text is a message only if it is exactly two
//! lines; line 1 has the prefix above, a non-empty rest, no control
//! character and at most [`MESSAGE_LINE_MAX`] characters; line 2 is one
//! JSON object from `{` to `}` (decoded strictly: no repeated keys) of at
//! most [`ANSWER_MAX_BYTES`]; and `skipped` comes with `{}` and only with
//! it. The question and summary are display only: the doc is the truth,
//! and [`Parsed::validate`] checks the values against it. Anything else is
//! ordinary text.

use serde_json::Value;

use crate::sanitize::{Lines, cut_chars, ui_sanitize};
use crate::*;

/// What an answer message says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `[card <id>]`
    Answer,
    /// `[card <id> changed]`: a later answer to the same card.
    Change,
    /// `[card <id> skipped]`, with `{}`.
    Skip,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Answer => "",
            Kind::Change => " changed",
            Kind::Skip => " skipped",
        }
    }
}

/// A validated reply to a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Answer(Answer),
    Change(Answer),
    Skip,
}

impl Reply {
    pub fn kind(&self) -> Kind {
        match self {
            Reply::Answer(_) => Kind::Answer,
            Reply::Change(_) => Kind::Change,
            Reply::Skip => Kind::Skip,
        }
    }
}

/// The message for `reply` to the card `card` asked with `doc`. The
/// answer must be [`parse_answer`]'s for this doc: this writes, it doesn't
/// check.
pub fn build(card: &CardId, doc: &CardDoc, reply: &Reply) -> String {
    let skip = Answer::skip();
    let (answer, summary) = match reply {
        Reply::Answer(a) | Reply::Change(a) => (a, summary(doc, a)),
        Reply::Skip => (&skip, "(skipped)".to_string()),
    };
    let question = ui_sanitize(&doc.question, Lines::Single);
    let line = format!("[card {card}{}] {question} → {summary}", reply.kind().tag());
    format!("{}\n{}", cut_chars(&line, MESSAGE_LINE_MAX), answer.to_json())
}

fn summary(doc: &CardDoc, a: &Answer) -> String {
    let piece = |s: &str| cut_chars(&ui_sanitize(s, Lines::Single), SUMMARY_PIECE_MAX);
    let mut pieces: Vec<String> = Vec::new();
    match &a.choice {
        Some(Choice::One(l)) => pieces.push(piece(l)),
        Some(Choice::Many(ls)) if ls.is_empty() => pieces.push("(none)".into()),
        Some(Choice::Many(ls)) => pieces.push(ls.iter().map(|l| piece(l)).collect::<Vec<_>>().join(", ")),
        None => {}
    }
    if let (Some(note), Some(Choice::One(l))) = (&a.note, &a.choice) {
        let prompt = doc.option(l).and_then(|o| o.note.as_deref()).unwrap_or("Note");
        pieces.push(piece(&format!("{prompt}: {note}")));
    }
    for (id, v) in a.fields.iter().flatten() {
        let label = doc.field(id).map_or(id.as_str(), |f| f.label.as_str());
        let value = match v {
            FieldValue::Check(true) => "yes",
            FieldValue::Check(false) => "no",
            FieldValue::Text(t) if t.is_empty() => "—",
            FieldValue::Text(t) => t.as_str(),
        };
        pieces.push(piece(&format!("{label}: {value}")));
    }
    pieces.join(" · ")
}

/// A text with the grammar of an answer message: what it says, not yet
/// checked against the card's doc.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub card: CardId,
    pub kind: Kind,
    /// Line 2, decoded: an object (`{}` exactly when `kind` is `Skip`).
    pub values: Value,
}

/// Read `text` as an answer message; `None` if it isn't one (see the
/// module docs).
pub fn parse(text: &str) -> Option<Parsed> {
    let (line1, line2) = text.split_once('\n')?;
    if line2.contains('\n') || line2.len() > ANSWER_MAX_BYTES || !line2.starts_with('{') || !line2.ends_with('}') {
        return None;
    }
    if line1.chars().count() > MESSAGE_LINE_MAX || line1.chars().any(char::is_control) {
        return None;
    }
    let (id, rest) = line1.strip_prefix("[card ")?.split_at_checked(7)?;
    let card = CardId::parse(id)?;
    let (kind, rest) = [Kind::Answer, Kind::Change, Kind::Skip]
        .into_iter()
        .find_map(|k| rest.strip_prefix(k.tag()).and_then(|r| r.strip_prefix("] ")).map(|r| (k, r)))?;
    if rest.is_empty() {
        return None;
    }
    let values = decode_json(line2).ok()?;
    let empty = values.as_object()?.is_empty();
    ((kind == Kind::Skip) == empty).then_some(Parsed { card, kind, values })
}

impl Parsed {
    /// The reply, once its values pass [`parse_answer`] for the card's doc.
    pub fn validate(&self, doc: &CardDoc) -> Result<Reply, Error> {
        match self.kind {
            Kind::Skip => Ok(Reply::Skip),
            Kind::Answer => parse_answer(doc, &self.values).map(Reply::Answer),
            Kind::Change => parse_answer(doc, &self.values).map(Reply::Change),
        }
    }
}

#[cfg(test)]
mod tests;
