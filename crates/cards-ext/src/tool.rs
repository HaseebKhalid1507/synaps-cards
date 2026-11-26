//! The `ui` tool (design §1), which the model sees as `cards_ui`: check who
//! calls, validate with cards-spec, mint an id, and return at once with the
//! result text (§1.5). Nothing waits for the answer, which comes later as a
//! user message and never through here. The only state is the rate counter
//! (§5.5), bounded across chats ([`crate::chats`]), and how many ids have
//! been minted (§1.5).

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cards_spec::sanitize::cut_chars;
use cards_spec::{CardDoc, CardId, DefaultChoice, Envelope, FIELD_LABEL_MAX, Input, OPTION_LABEL_MAX, parse_input};
use serde_json::{Value, json};

use crate::chats::{CHATS_MAX, Chats};

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
/// Past the last of the 36^6 ids ([`Ids`]): never in practice, but never an
/// id twice.
pub const SPENT: &str = "cards_ui: every card id is taken until the extension restarts; ask in plain text";

/// §5.5: cards shown per chat.
pub const PER_MINUTE: usize = 3;
pub const PER_TEN_MINUTES: usize = 6;
const MINUTE: Duration = Duration::from_secs(60);
#[cfg(test)]
const TEN_MINUTES: Duration = crate::chats::KEPT;

/// A tool call refused: the JSON-RPC error the host hands the model as the
/// call's output (`Extension error: <message>`), with no envelope, so the
/// desk shows a plain row.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: i64,
    pub message: String,
}

impl Refusal {
    fn invalid(message: String) -> Self {
        Refusal { code: INVALID_PARAMS, message }
    }

    fn refused(message: String) -> Self {
        Refusal { code: REFUSED, message }
    }
}

/// `initialize`'s result: protocol 1 and the one tool, its input schema
/// cards-spec's `schema.json` (§1.4).
pub fn initialize() -> Value {
    let schema: Value = serde_json::from_str(cards_spec::SCHEMA).expect("cards-spec's schema.json is JSON");
    json!({
        "protocol_version": PROTOCOL_VERSION,
        "capabilities": { "tools": [{ "name": NAME, "description": DESCRIPTION.trim_end(), "input_schema": schema }] }
    })
}

#[derive(Default)]
pub struct Cards {
    rate: Rate,
    ids: Ids,
}

impl Cards {
    /// One `tool.call`: the result text, or why not.
    pub fn call(&mut self, params: &Value, now: Instant) -> Result<String, Refusal> {
        let name = params.get("name").and_then(Value::as_str);
        if name != Some(NAME) {
            let name = name.map_or_else(|| "(none)".to_string(), |n| format!("{n:?}"));
            return Err(Refusal::invalid(format!("cards: no tool {name}; this extension has one, `{NAME}`")));
        }
        let chat = chat_of(params).ok_or_else(|| Refusal::refused(WORKER.to_string()))?;
        match parse_input(params.get("input").unwrap_or(&Value::Null)).map_err(|e| Refusal::invalid(e.to_string()))? {
            Input::Close(card) => Ok(Envelope::closed(card).to_string()),
            Input::Create { doc, wait } => {
                if self.ids.spent() {
                    return Err(Refusal::refused(SPENT.to_string()));
                }
                self.rate.admit(&chat, now).map_err(Refusal::refused)?;
                let id = self.ids.mint().expect("not spent: checked before the rate");
                Ok(result_text(&id, &doc, wait))
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

/// §5.5's counter: cards shown per chat in the last minute and the last ten,
/// in a [`Chats`] table, bounded across chats as well as in each: a new chat
/// that finds all [`CHATS_MAX`] places held by chats with a card in their
/// windows is refused, and no chat is evicted early, so none escapes its
/// limit. In memory: a restarted process starts it again, which only errs
/// lenient.
#[derive(Default)]
pub struct Rate {
    shown: Chats<()>,
}

impl Rate {
    /// Count one more card for `chat` at `now`, or say why not. A refused
    /// card isn't counted.
    pub fn admit(&mut self, chat: &str, now: Instant) -> Result<(), String> {
        let place = self.shown.at(chat, now);
        let times = place.entries();
        if times.len() >= PER_TEN_MINUTES {
            return Err(format!("cards_ui: {PER_TEN_MINUTES} cards in the last 10 minutes; ask in plain text"));
        }
        if times.iter().filter(|&&(t, ())| now.saturating_duration_since(t) < MINUTE).count() >= PER_MINUTE {
            return Err(format!("cards_ui: {PER_MINUTE} cards in the last minute; ask in plain text"));
        }
        place.push(()).map_err(|_| format!("cards_ui: {CHATS_MAX} other chats showed cards in the last 10 minutes; ask in plain text"))
    }
}

/// How many card ids there are: `c` and 6 of `[a-z0-9]`.
pub const IDS: u64 = 36u64.pow(6);

/// Card ids (§1.5), never the same twice while the extension runs, with
/// nothing remembered to make it so: the n-th id is [`permute`]'s n-th
/// value over the 36^6 ids, under keys drawn when the process starts. A
/// permutation never repeats a value, so the first 36^6 ids are all
/// different, and past them [`Ids::mint`] has none ([`SPENT`]): at §5.5's
/// pace in all [`CHATS_MAX`] chats, 1,536 cards in 10 minutes, a process
/// would need 27 years to get there. A restart draws new keys, so across one
/// the ids are unique in practice (36^6 ids), as drawn ids were. Not secret
/// (§5.2): the keys only make the ids look drawn.
pub struct Ids {
    keys: [u64; 4],
    next: u64,
}

impl Default for Ids {
    fn default() -> Self {
        let keys = RandomState::new();
        let clock = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        Ids::keyed(std::array::from_fn(|round| {
            let mut h = keys.build_hasher();
            h.write_usize(round);
            h.write_u128(clock);
            h.finish()
        }))
    }
}

impl Ids {
    /// The ids under `keys`, from the first.
    pub fn keyed(keys: [u64; 4]) -> Self {
        Ids { keys, next: 0 }
    }

    /// Whether all [`IDS`] have been minted.
    pub fn spent(&self) -> bool {
        self.next >= IDS
    }

    /// The next id, or none once [`spent`](Ids::spent).
    pub fn mint(&mut self) -> Option<CardId> {
        let n = self.next;
        (n < IDS).then(|| {
            self.next += 1;
            Self::id_of(permute(&self.keys, 16, IDS, n))
        })
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

/// A keyed permutation of `0..n`, for `n` up to `2^(2·half)`: a Feistel
/// network on `2·half` bits, one round a key, which is a permutation of
/// `0..2^(2·half)` whatever its round function; then walked until it lands
/// in `0..n` (cycle walking), which keeps it one of `0..n`: `x`'s cycle comes
/// back to `x`, so the walk ends, and the value it ends on is reached from
/// `x` alone. Ids use `half` 16: 2^32 values for the 36^6 ids, so a walk
/// is about two passes of the network on average (at most 2^32/36^6 ≈ 1.97
/// over all ids), though no small bound holds for any one walk.
fn permute(keys: &[u64], half: u32, n: u64, x: u64) -> u64 {
    debug_assert!(x < n && n <= 1 << (2 * half));
    let mask = (1u64 << half) - 1;
    let mut y = x;
    loop {
        let (mut l, mut r) = (y >> half, y & mask);
        for &k in keys {
            (l, r) = (r, l ^ (mix(k ^ r) & mask));
        }
        y = (l << half) | r;
        if y < n {
            return y;
        }
    }
}

/// The round function, splitmix64's finalizer: any function keeps the
/// network a permutation; this one only makes the ids look drawn.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
impl Rate {
    fn window(&self, chat: &str) -> Option<&std::collections::VecDeque<(Instant, ())>> {
        self.shown.window(chat)
    }

    fn len(&self) -> usize {
        self.shown.len()
    }

    fn capacity(&self) -> usize {
        self.shown.capacity()
    }
}

#[cfg(test)]
mod tests;
