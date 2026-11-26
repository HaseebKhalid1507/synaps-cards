//! The `ui` tool (design §1), which the model sees as `cards_ui`: check who
//! calls, validate with cards-spec, mint an id, and return at once with the
//! result text (§1.5). Nothing waits for the answer, which comes later as a
//! user message and never through here. The only state is the rate counter
//! (§5.5) and the ids minted so far (§1.5).
//!
//! [`Cards`] also routes the second tool, `block` ([`crate::block`]), when
//! `initialize` turned it on.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cards_spec::sanitize::cut_chars;
use cards_spec::{CardDoc, CardId, DefaultChoice, Envelope, FIELD_LABEL_MAX, Input, OPTION_LABEL_MAX, parse_input};
use serde_json::{Value, json};

use crate::block::{self, Blocks};

/// The tool's name as the extension declares it. The host namespaces it
/// `cards:ui`, and the model sees `cards_ui` (`protocol.md`, "Registered tool
/// runtime names").
pub const NAME: &str = "ui";
pub const PROTOCOL_VERSION: u32 = 1;
pub const DESCRIPTION: &str = include_str!("description.md");

/// A success result, in UTF-8 bytes (§1.5): under the result bounds and the
/// host's default spill threshold.
pub const RESULT_MAX_BYTES: usize = 1024;
const LIVE: &str = "The user may also just reply in text. Keep working meanwhile.";
const WAIT: &str = "You can't continue without it: end your turn now; the answer starts your next turn.";

pub const INVALID_PARAMS: i64 = -32602;
pub const REFUSED: i64 = -32000;
pub const WORKER: &str = "cards_ui: only the main chat can show cards";

/// §5.5: cards shown per chat.
pub const PER_MINUTE: usize = 3;
pub const PER_TEN_MINUTES: usize = 6;
const MINUTE: Duration = Duration::from_secs(60);
const TEN_MINUTES: Duration = Duration::from_secs(600);

/// A tool call refused: the JSON-RPC error the host hands the model as the
/// call's output (`Extension error: <message>`), with no envelope, so the
/// desk shows a plain row.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: i64,
    pub message: String,
}

impl Refusal {
    pub(crate) fn invalid(message: String) -> Self {
        Refusal { code: INVALID_PARAMS, message }
    }

    pub(crate) fn refused(message: String) -> Self {
        Refusal { code: REFUSED, message }
    }
}

/// `initialize`'s result: protocol 1 and the tools, `ui` with cards-spec's
/// `schema.json` as its input schema (§1.4), then, when `blocks` is on,
/// `block` with `schema-blocks.json` (`docs/blocks.md` §1.1). Off, `block`
/// isn't declared at all, so its description costs nothing.
pub fn initialize(blocks: bool) -> Value {
    let schema: Value = serde_json::from_str(cards_spec::SCHEMA).expect("cards-spec's schema.json is JSON");
    let mut tools = vec![json!({ "name": NAME, "description": DESCRIPTION.trim_end(), "input_schema": schema })];
    if blocks {
        let schema: Value = serde_json::from_str(cards_spec::block::SCHEMA).expect("cards-spec's schema-blocks.json is JSON");
        tools.push(json!({ "name": block::NAME, "description": block::DESCRIPTION.trim_end(), "input_schema": schema }));
    }
    json!({ "protocol_version": PROTOCOL_VERSION, "capabilities": { "tools": tools } })
}

#[derive(Default)]
pub struct Cards {
    rate: Rate,
    ids: Ids,
    /// `block`, once `initialize` turned it on; `None` until then and off.
    blocks: Option<Blocks>,
}

impl Cards {
    /// `initialize`: read the gate from `params.config` (`docs/blocks.md`
    /// §1.1), then declare the tools. The host resolves the config when it
    /// starts the process, so the gate holds for the process's life; a
    /// change takes `synaps daemon reload-extension cards`.
    pub fn initialize(&mut self, params: &Value) -> Value {
        let on = block::gate(params.get("config"));
        if on != self.blocks.is_some() {
            self.blocks = on.then(Blocks::default);
        }
        initialize(on)
    }

    /// One `tool.call`: the result text, or why not.
    pub fn call(&mut self, params: &Value, now: Instant) -> Result<String, Refusal> {
        let name = params.get("name").and_then(Value::as_str);
        if name == Some(NAME) {
            return self.ui(params, now);
        }
        match (name, self.blocks.as_mut()) {
            (Some(block::NAME), Some(blocks)) => blocks.call(params, now),
            (Some(block::NAME), None) => {
                Err(Refusal::invalid(format!("cards: no tool \"{}\"; blocks are off (extension.cards.{})", block::NAME, block::GATE)))
            }
            (name, blocks) => {
                let name = name.map_or_else(|| "(none)".to_string(), |n| format!("{n:?}"));
                let has = match blocks {
                    Some(_) => format!("two, `{NAME}` and `{}`", block::NAME),
                    None => format!("one, `{NAME}`"),
                };
                Err(Refusal::invalid(format!("cards: no tool {name}; this extension has {has}")))
            }
        }
    }

    /// One `tool.call` of `ui`.
    fn ui(&mut self, params: &Value, now: Instant) -> Result<String, Refusal> {
        let chat = chat_of(params).ok_or_else(|| Refusal::refused(WORKER.to_string()))?;
        match parse_input(params.get("input").unwrap_or(&Value::Null)).map_err(|e| Refusal::invalid(e.to_string()))? {
            Input::Close(card) => Ok(Envelope::closed(card).to_string()),
            Input::Create { doc, wait } => {
                self.rate.admit(&chat, now).map_err(Refusal::refused)?;
                Ok(result_text(&self.ids.mint(), &doc, wait))
            }
        }
    }
}

/// The chat a call comes from, which keys the rate counter; `None` for a
/// worker, whose card no chat would show (§1.5, in its order).
///
/// A null `session_id` alone isn't a worker: on `dev` it is also every chat
/// under the `SYNAPS_HOOK_SESSION_ID=0` kill switch, and a host before
/// 0.10.3 sends no `session_id` at all (`protocol.md`, `tool.call`;
/// `session-id.md`). The host-written `caller` says more: a worker's names
/// its `parent` (`Author::child`), a chat's never does, and the kill switch
/// leaves it alone. So a `parent` refuses, a `session_id` is a chat, a
/// `caller` without one is a chat by its `group`, and a call with neither
/// is refused.
pub fn chat_of(params: &Value) -> Option<String> {
    let caller = params.get("caller").and_then(Value::as_object);
    if caller.is_some_and(|c| c.contains_key("parent")) {
        return None;
    }
    if let Some(session) = params.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        return Some(format!("session {session}"));
    }
    let group = caller?.get("group")?.as_str().filter(|g| !g.is_empty())?;
    Some(format!("group {group}"))
}

/// A success result (§1.5), at most [`RESULT_MAX_BYTES`]: the envelope
/// (at most 256 bytes, only its question cut), the options numbered with
/// the default marked, the fields, then the fixed trailer. Options and
/// fields fill what the envelope and the trailer leave; when the next one
/// wouldn't fit, the list ends `  …and N more`.
pub fn result_text(card: &CardId, doc: &CardDoc, wait: bool) -> String {
    let head = Envelope::showing(card.clone(), doc, wait).to_string();
    let tail = [
        format!("Its answer will come as a user message starting \"[card {card}]\", values as JSON on the next line."),
        String::from(if wait { WAIT } else { LIVE }),
    ];
    let suggested = |label: &str| match &doc.default {
        Some(DefaultChoice::One(d)) => d == label,
        Some(DefaultChoice::Many(ds)) => ds.iter().any(|d| d == label),
        None => false,
    };
    let options = doc.options.iter().enumerate().map(|(i, o)| {
        let mark = if suggested(&o.label) { " (suggested)" } else { "" };
        format!("  {}. {}{mark}", i + 1, cut_chars(&o.label, OPTION_LABEL_MAX))
    });
    let fields = doc.fields.iter().map(|f| {
        let mark = if f.optional { " (optional)" } else { "" };
        format!("  {}: {}{mark}", f.id, cut_chars(&f.label, FIELD_LABEL_MAX))
    });
    let items: Vec<String> = options.chain(fields).collect();
    let more = |n: usize| format!("  …and {n} more");
    let mut room = RESULT_MAX_BYTES - head.len() - tail.iter().map(|l| 1 + l.len()).sum::<usize>();
    let mut lines = vec![head];
    for (i, item) in items.iter().enumerate() {
        let after = items.len() - i - 1;
        let need = 1 + item.len() + if after > 0 { 1 + more(after).len() } else { 0 };
        if need > room {
            lines.push(more(items.len() - i));
            break;
        }
        room -= 1 + item.len();
        lines.push(item.clone());
    }
    lines.extend(tail);
    lines.join("\n")
}

/// §5.5's counter: cards shown per chat in the last minute and the last ten.
/// In memory: a restarted process starts it again, which only errs lenient.
#[derive(Default)]
pub struct Rate {
    shown: HashMap<String, VecDeque<Instant>>,
}

impl Rate {
    /// Count one more card for `chat` at `now`, or say why not. A refused
    /// card isn't counted.
    pub fn admit(&mut self, chat: &str, now: Instant) -> Result<(), String> {
        self.shown.retain(|_, times| {
            while times.front().is_some_and(|&t| now.saturating_duration_since(t) >= TEN_MINUTES) {
                times.pop_front();
            }
            !times.is_empty()
        });
        let times = self.shown.entry(chat.to_string()).or_default();
        if times.len() >= PER_TEN_MINUTES {
            return Err(format!("cards_ui: {PER_TEN_MINUTES} cards in the last 10 minutes; ask in plain text"));
        }
        if times.iter().filter(|&&t| now.saturating_duration_since(t) < MINUTE).count() >= PER_MINUTE {
            return Err(format!("cards_ui: {PER_MINUTE} cards in the last minute; ask in plain text"));
        }
        times.push_back(now);
        Ok(())
    }
}

/// Card ids (§1.5): `c` and 6 of `[a-z0-9]`, never the same twice while
/// the extension runs: a draw that names an id already minted draws again.
/// One process serves every chat, so a session's ids are unique for the
/// process's life; a restart starts a fresh set, and across one they are
/// unique in practice (36^6 ids). Not secret (§5.2), so the default draw,
/// std's randomly keyed hasher over a counter and the clock, is enough.
/// The set grows by one id per card, which §5.5 caps at 36 an hour a chat.
pub struct Ids {
    draw: Box<dyn FnMut() -> u64>,
    minted: HashSet<CardId>,
}

impl Default for Ids {
    fn default() -> Self {
        let keys = RandomState::new();
        let mut n = 0u64;
        Ids::drawing(move || {
            let mut h = keys.build_hasher();
            h.write_u64(n);
            h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
            n += 1;
            h.finish()
        })
    }
}

impl Ids {
    /// Ids from `draw`'s values, each read as [`Ids::id_of`] reads it.
    pub fn drawing(draw: impl FnMut() -> u64 + 'static) -> Self {
        Ids { draw: Box::new(draw), minted: HashSet::new() }
    }

    pub fn mint(&mut self) -> CardId {
        loop {
            let id = Self::id_of((self.draw)());
            if self.minted.insert(id.clone()) {
                return id;
            }
        }
    }

    /// `c`, then `v`'s 6 lowest base-36 digits, lowest first, `a`–`z` for
    /// 0–25 and `0`–`9` for 26–35.
    pub fn id_of(mut v: u64) -> CardId {
        const DIGITS: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut id = String::from("c");
        for _ in 0..6 {
            id.push(char::from(DIGITS[(v % 36) as usize]));
            v /= 36;
        }
        CardId::parse(&id).expect("c and 6 of [a-z0-9]")
    }
}

#[cfg(test)]
mod tests;
