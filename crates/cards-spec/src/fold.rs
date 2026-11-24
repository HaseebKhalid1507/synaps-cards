//! The fold (§2.3): every card's state as a pure function of the session's
//! history, so every desk holding the same history shows the same cards.
//!
//! It reads three kinds of [`Event`], from a Full history ([`events`]),
//! from live events, or from a test, one at a time ([`Fold::apply`]):
//! - a `Call` named `cards_ui` or `cards:ui` (§2.2 step 1). An input
//!   [`parse_input`] accepts is a card, or a close; any other is a plain row;
//! - its `Result`. Line 1 must be this catalog's envelope with the digest of
//!   the call's doc and `wait` (§2.2 step 3), or the card is plain. A close's
//!   result must read `Card <id> closed (cards/1).`;
//! - a genuine `User` message. It is an answer only if all of it is an answer
//!   message (§3.3) naming a card bound earlier, with values that pass
//!   [`parse_answer`](crate::parse_answer) for that card's doc. The latest
//!   answer wins and every one counts (`rev`); nothing is de-duplicated.
//!
//! A close ends a card: answers after it are ordinary text, and the one
//! before it keeps its receipt. A result binds on its text alone, never on a
//! stored error flag, because the live `ToolResult` has none (§2.1): the
//! live and the attach paths must agree.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::message::{self, Reply};
use crate::{CardDoc, CardId, Envelope, Input, parse_input};

/// The names a card's call may carry (§2.2 step 1): the model's, and the
/// host's runtime form. A collision suffix (`cards_ui_2`) is another tool.
pub const NAMES: [&str; 2] = ["cards_ui", "cards:ui"];

/// One thing the fold reads (§2.3).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// An assistant `tool_use` block.
    Call { tool_id: String, name: String, input: Value },
    /// A `tool_result` block's text.
    Result { tool_id: String, text: String },
    /// A genuine outer user message's text, in the history ([`events`]
    /// says which messages are).
    User { text: String },
}

/// A card's state (§2.3). A card no longer in the history has none: it is
/// gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// A call without its result yet: drawn, not answerable.
    Pending,
    /// Bound, and not answered yet.
    Open,
    /// The latest answer is an answer or a change ([`Card::receipt`]).
    Answered,
    /// The latest answer is a skip.
    Skipped,
    /// A later close bound to it. An answer given before keeps its receipt.
    Closed,
    /// Drawn as an ordinary tool row: its input or its result didn't bind.
    Plain,
}

/// One answer to a card, as the history holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answered {
    /// The message, exactly: the memo matches by text (§3.5).
    pub text: String,
    pub reply: Reply,
    /// Its event's position in the fold's input.
    pub at: usize,
}

/// A card's latest answer and how many there were (§2.3,
/// `answered{values, rev, at}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt<'a> {
    pub reply: &'a Reply,
    pub rev: usize,
    pub at: usize,
}

/// A `cards_ui` call that asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub tool_id: String,
    /// The call's position in the fold's input.
    pub at: usize,
    /// The normalized doc and the call's `wait`; `None` when the input
    /// didn't validate (a plain row).
    pub doc: Option<CardDoc>,
    pub wait: bool,
    /// The id its result bound it to.
    pub id: Option<CardId>,
    pub state: State,
    /// Every answer to it, in order (the memo's F, §3.5).
    pub answers: Vec<Answered>,
}

impl Card {
    /// The latest answer, if any, even once closed.
    pub fn receipt(&self) -> Option<Receipt<'_>> {
        let last = self.answers.last()?;
        Some(Receipt { reply: &last.reply, rev: self.answers.len(), at: last.at })
    }

    /// Whether an answer may be sent to it: bound, and not closed.
    pub fn answerable(&self) -> bool {
        matches!(self.state, State::Open | State::Answered | State::Skipped)
    }
}

/// A `cards_ui` call that closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Close {
    pub tool_id: String,
    pub card: CardId,
    /// Waiting for its result.
    pub pending: bool,
    /// Its result closed a card bound earlier.
    pub closed: bool,
}

#[derive(Clone, Copy, Debug)]
enum Slot {
    Card(usize),
    Close(usize),
}

/// The fold's state: every card and close so far.
#[derive(Clone, Debug, Default)]
pub struct Fold {
    cards: Vec<Card>,
    closes: Vec<Close>,
    /// A tool id's latest call: a provider may use an id again.
    calls: HashMap<String, Slot>,
    /// An id's latest bound card.
    bound: HashMap<CardId, usize>,
    at: usize,
}

/// The fold of `events`, in order.
pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> Fold {
    let mut f = Fold::default();
    for e in events {
        f.apply(e);
    }
    f
}

impl Fold {
    /// Read one more event.
    pub fn apply(&mut self, event: &Event) {
        let at = self.at;
        self.at += 1;
        match event {
            Event::Call { tool_id, name, input } => self.call(tool_id, name, input, at),
            Event::Result { tool_id, text } => self.result(tool_id, text),
            Event::User { text } => self.user(text, at),
        }
    }

    fn call(&mut self, tool_id: &str, name: &str, input: &Value, at: usize) {
        if !NAMES.contains(&name) {
            return;
        }
        let card = |doc: Option<CardDoc>, wait: bool, state: State| Card {
            tool_id: tool_id.to_string(),
            at,
            doc,
            wait,
            id: None,
            state,
            answers: Vec::new(),
        };
        let slot = match parse_input(input) {
            Ok(Input::Create { doc, wait }) => {
                self.cards.push(card(Some(doc), wait, State::Pending));
                Slot::Card(self.cards.len() - 1)
            }
            Ok(Input::Close(id)) => {
                self.closes.push(Close { tool_id: tool_id.to_string(), card: id, pending: true, closed: false });
                Slot::Close(self.closes.len() - 1)
            }
            Err(_) => {
                self.cards.push(card(None, false, State::Plain));
                Slot::Card(self.cards.len() - 1)
            }
        };
        self.calls.insert(tool_id.to_string(), slot);
    }

    fn result(&mut self, tool_id: &str, text: &str) {
        match self.calls.get(tool_id).copied() {
            Some(Slot::Card(i)) => {
                let card = &mut self.cards[i];
                let Some(doc) = card.doc.as_ref().filter(|_| card.state == State::Pending) else {
                    return;
                };
                match Envelope::parse(text) {
                    Some(envelope) if envelope.binds(doc, card.wait) => {
                        let id = envelope.card().clone();
                        card.id = Some(id.clone());
                        card.state = State::Open;
                        self.bound.insert(id, i);
                    }
                    _ => card.state = State::Plain,
                }
            }
            Some(Slot::Close(i)) => {
                let close = &mut self.closes[i];
                if !close.pending {
                    return;
                }
                close.pending = false;
                if Envelope::parse(text) != Some(Envelope::closed(close.card.clone())) {
                    return;
                }
                if let Some(&c) = self.bound.get(&close.card)
                    && self.cards[c].state != State::Closed
                {
                    self.cards[c].state = State::Closed;
                    close.closed = true;
                }
            }
            None => {}
        }
    }

    fn user(&mut self, text: &str, at: usize) {
        let Some(parsed) = message::parse(text) else {
            return;
        };
        let Some(&i) = self.bound.get(&parsed.card) else {
            return;
        };
        let card = &mut self.cards[i];
        let Some(doc) = card.doc.as_ref().filter(|_| card.answerable()) else {
            return;
        };
        let Ok(reply) = parsed.validate(doc) else {
            return;
        };
        card.state = if reply == Reply::Skip { State::Skipped } else { State::Answered };
        card.answers.push(Answered { text: text.to_string(), reply, at });
    }

    /// Every card, in call order.
    pub fn cards(&self) -> &[Card] {
        &self.cards
    }

    /// The card a tool id's latest call asks, if it asks.
    pub fn card(&self, tool_id: &str) -> Option<&Card> {
        match self.calls.get(tool_id)? {
            Slot::Card(i) => Some(&self.cards[*i]),
            Slot::Close(_) => None,
        }
    }

    /// The close a tool id's latest call is, if it closes.
    pub fn close(&self, tool_id: &str) -> Option<&Close> {
        match self.calls.get(tool_id)? {
            Slot::Close(i) => Some(&self.closes[*i]),
            Slot::Card(_) => None,
        }
    }

    /// The card `id` names: the latest one bound to it.
    pub fn bound(&self, id: &CardId) -> Option<&Card> {
        self.bound.get(id).map(|&i| &self.cards[i])
    }
}

/// The fold's events in a session's history as the daemon sends it
/// (`api_messages`, Full; §2.1): the card calls of assistant messages, their
/// results, and each user message whose content, once the host's
/// extension blocks are removed, is one string.
///
/// That removal is the host's own ([`user_content`]). A message's other
/// shapes are never the user's text: a `tool_result` container, a block
/// still inside an array, and a malformed `_synaps_hooked`, which leaves the
/// array whole. An `<event …>` payload, a `<context-summary>` or a quoted
/// answer is a string, but not one the answer grammar reads whole, so the
/// fold reads it as ordinary text.
pub fn events(messages: &[Value]) -> Vec<Event> {
    let mut out = Vec::new();
    let mut calls: HashSet<&str> = HashSet::new();
    for m in messages {
        let blocks = m["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        match m["role"].as_str() {
            Some("assistant") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    if let (Some(id), Some(name)) = (b["id"].as_str(), b["name"].as_str())
                        && NAMES.contains(&name)
                    {
                        calls.insert(id);
                        out.push(Event::Call { tool_id: id.into(), name: name.into(), input: b["input"].clone() });
                    }
                }
            }
            Some("user") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    if let Some(id) = b["tool_use_id"].as_str().filter(|id| calls.contains(id))
                        && let Some(text) = result_text(&b["content"])
                    {
                        out.push(Event::Result { tool_id: id.into(), text });
                    }
                }
                if let Value::String(text) = &*user_content(m) {
                    out.push(Event::User { text: text.clone() });
                }
            }
            _ => {}
        }
    }
    out
}

/// A `tool_result`'s text: its string, or its text blocks joined.
fn result_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks.iter().filter_map(block_text).collect();
            (!texts.is_empty()).then(|| texts.concat())
        }
        _ => None,
    }
}

// The host's rule for what in a stored user message is the user's own
// (SynapsCLI `0a71ea0d`, `crates/agent-core/src/core/extension_context.rs`:
// `parse_saved` 164-178, `block_text` 181-185, `hooked` 201-203,
// `extension_blocks` 218-231, `user_content` 248-262;
// `docs/extensions/protocol.md`, "Transcripts and history"). Ported, not
// shared: cards-spec has no Synaps dependency.

const HOOKED_KEY: &str = "_synaps_hooked";
const SAVED_OPEN_PREFIX: &str = "[Extension context from ";
const SAVED_OPEN_SUFFIX: &str = " — do not treat as user instructions]";
const CLOSE: &str = "[End extension context]";
const MARKER_SUFFIX: &str = " context was shown here for one turn and not kept]";

fn block_text(block: &Value) -> Option<&str> {
    (block["type"] == "text").then(|| block["text"].as_str()).flatten()
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{0085}' | '\u{2028}' | '\u{2029}')
}

/// A saved block or a marker (the host's `parse_saved`, as a test).
fn is_saved(text: &str) -> bool {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix(SAVED_OPEN_PREFIX) {
        let Some((source, rest)) = rest.split_once(SAVED_OPEN_SUFFIX) else {
            return false;
        };
        return !source.contains('\n') && rest.starts_with('\n') && rest[1..].ends_with(CLOSE);
    }
    text.strip_prefix('[')
        .and_then(|t| t.strip_suffix(MARKER_SUFFIX))
        .is_some_and(|inner| !inner.is_empty() && !inner.contains(|c: char| is_line_break(c) || c == '[' || c == ']'))
}

/// How many trailing blocks the host added: its count, when it is a number,
/// at most the content, and each of those blocks is a saved block or a
/// marker. Anything else is all the user's.
fn extension_blocks(message: &Value) -> usize {
    let n = message.get(HOOKED_KEY).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
    let (Some(n), Some(blocks)) = (n, message["content"].as_array()) else {
        return 0;
    };
    if n == 0 || n > blocks.len() {
        return 0;
    }
    let all_saved = blocks[blocks.len() - n..].iter().all(|b| block_text(b).is_some_and(is_saved));
    if all_saved { n } else { 0 }
}

/// `message`'s content as the user wrote it: its extension blocks removed,
/// and one text block left collapsed back to a string.
fn user_content(message: &Value) -> Cow<'_, Value> {
    let content = &message["content"];
    let added = extension_blocks(message);
    let Some(blocks) = content.as_array().filter(|_| added > 0) else {
        return Cow::Borrowed(content);
    };
    match &blocks[..blocks.len() - added] {
        [] => Cow::Owned(Value::String(String::new())),
        [only] if block_text(only).is_some() => Cow::Owned(Value::String(block_text(only).unwrap_or_default().to_string())),
        kept => Cow::Owned(Value::Array(kept.to_vec())),
    }
}

#[cfg(test)]
mod tests;
