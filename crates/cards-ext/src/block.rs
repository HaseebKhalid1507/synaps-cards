//! The `block` tool (`docs/blocks.md` §1.1), which the model sees as
//! `cards_block`: check who calls, validate the batch with
//! `cards_spec::block`, count it, and return at once with its receipt,
//! `ok 3 ops (blocks/1 #…)` (§1.6). "ok" means valid, not applied: nothing
//! here knows a board. The desk folds the call from the transcript and binds
//! it by the same digest. The only state is the rate counter (§7).
//!
//! Declared only when the manifest's `blocks` config is on
//! ([`gate`]); off by default until the desk renders blocks (§1.1, §9.5).
//!
//! **Readings** (agent-visible, where `docs/blocks.md` leaves a choice):
//! - the counts in the rate refusals include the refused call, as §1.6's
//!   example does (`31 calls in the last minute` at a limit of 30); the ops
//!   refusal names its limit, since one call's ops take it past by any
//!   amount ([`Rate::admit`]);
//! - a refused call isn't counted, as a refused card isn't;
//! - a new chat that finds all [`CHATS_MAX`] places held by chats with calls
//!   in their windows is refused, not let in by evicting one:
//!   `cards_block: 256 other chats used blocks in the last 10 minutes; try
//!   again in 37 s`, the wait until the chat idle longest leaves ([`Rate`]);
//! - the gate is on for JSON `true`, or a string the host reads as on
//!   (`1`, `true`, `yes`, `on`, any case, spaces trimmed); anything else is
//!   off, and a value that is neither on nor off is logged ([`gate`]).

use std::time::{Duration, Instant};

use cards_spec::block::{RESULT_MAX_BYTES, parse_call};
use serde_json::Value;

#[cfg(test)]
use crate::chats::KEPT as TEN_MINUTES;
use crate::chats::{CHATS_MAX, Chats};
use crate::tool::{Refusal, chat_of};

/// The tool's name as the extension declares it; the host namespaces it
/// `cards:block`, and the model sees `cards_block` (§1.1).
pub const NAME: &str = "block";
/// §2.6's text, verbatim: B3 ships it, B5 tunes it.
pub const DESCRIPTION: &str = include_str!("description-block.md");
/// The manifest's config key (`extension.cards.blocks`), default `false`.
pub const GATE: &str = "blocks";

/// What every refusal starts with: the model reads it as the call's output.
pub const PREFIX: &str = "cards_block: ";
/// §1.1: a worker's transcript is not the user's document.
pub const WORKER: &str = "cards_block: only the main chat has blocks";

/// §7, per chat.
pub const CALLS_PER_MINUTE: usize = 30;
pub const OPS_PER_MINUTE: usize = 120;
pub const CALLS_PER_TEN_MINUTES: usize = 200;
const MINUTE: Duration = Duration::from_secs(60);

/// Whether `initialize`'s `params.config` turns blocks on. The host passes
/// the manifest's default as JSON (`false`), and a value from the
/// environment (`SYNAPS_EXTENSION_CARDS_BLOCKS`) or a config file as a
/// string; a string is read as the host reads its own switches.
pub fn gate(config: Option<&Value>) -> bool {
    match config.and_then(|c| c.get(GATE)) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(on)) => *on,
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" | "" => false,
            _ => {
                eprintln!("cards: extension.cards.{GATE} is {s:?}, neither on nor off; blocks stay off");
                false
            }
        },
        Some(other) => {
            eprintln!("cards: extension.cards.{GATE} is {other}, neither on nor off; blocks stay off");
            false
        }
    }
}

#[derive(Default)]
pub struct Blocks {
    rate: Rate,
}

impl Blocks {
    /// One `tool.call` of `block`: the receipt, or why not. Who calls is
    /// checked first, so a worker can't validate its way in; then the batch
    /// (the first failure is the refusal, §1.3); then the rate.
    pub fn call(&mut self, params: &Value, now: Instant) -> Result<String, Refusal> {
        let chat = chat_of(params).ok_or_else(|| Refusal::refused(WORKER.to_string()))?;
        let batch = parse_call(params.get("input").unwrap_or(&Value::Null)).map_err(|e| Refusal::invalid(format!("{PREFIX}{e}")))?;
        self.rate.admit(&chat, batch.ops.len(), now).map_err(Refusal::refused)?;
        let receipt = batch.receipt().to_string();
        debug_assert!(receipt.len() <= RESULT_MAX_BYTES, "{receipt}");
        Ok(receipt)
    }
}

/// §7's brake: calls and ops per chat in the last minute, calls in the last
/// ten. In memory, so a restart starts it again, which only errs lenient
/// (design §5.5).
///
/// **Bounded across chats, not only in each**, in its own [`Chats`] table
/// (`cards_ui`'s rate keeps another, so the two tools don't share places).
/// A chat keeps at most
/// [`CALLS_PER_TEN_MINUTES`] calls, and at most [`CHATS_MAX`] chats are kept,
/// each under a 64-bit keyed hash of its name, so a long host id costs what
/// a short one does (two names that collide share one limit, which errs
/// strict). A chat leaves when its last call is ten minutes old, so the
/// chats idle longest leave first, and never sooner: by then it has nothing
/// left to limit, and leaving resets nothing. No chat with a call in its
/// window is evicted, so no crowd of other chats can buy one a fresh
/// budget; while all [`CHATS_MAX`] have one, a new chat is refused until the
/// one idle longest leaves. Every call prunes, so a chat's place is free the
/// moment it leaves, and the memory a crowd or a burst took is given back
/// once the table, or a chat's calls, are a quarter full.
#[derive(Default)]
pub struct Rate {
    calls: Chats<usize>,
}

impl Rate {
    /// Count a call of `ops` ops for `chat` at `now`, or say why not.
    pub fn admit(&mut self, chat: &str, ops: usize, now: Instant) -> Result<(), String> {
        let place = self.calls.at(chat, now);
        let calls = place.entries();
        if calls.len() >= CALLS_PER_TEN_MINUTES {
            let n = CALLS_PER_TEN_MINUTES + 1;
            return Err(format!("{PREFIX}{n} calls in the last 10 minutes; batch your ops or patch less"));
        }
        let (n, used) = calls
            .iter()
            .filter(|&&(t, _)| now.saturating_duration_since(t) < MINUTE)
            .fold((0, 0), |(n, used), &(_, ops)| (n + 1, used + ops));
        if n >= CALLS_PER_MINUTE {
            let n = CALLS_PER_MINUTE + 1;
            return Err(format!("{PREFIX}{n} calls in the last minute; batch your ops or patch less"));
        }
        if used + ops > OPS_PER_MINUTE {
            return Err(format!("{PREFIX}{} ops in the last minute, over {OPS_PER_MINUTE}; patch less", used + ops));
        }
        place.push(ops).map_err(|wait| {
            let s = (wait.as_secs() + u64::from(wait.subsec_nanos() > 0)).max(1);
            format!("{PREFIX}{CHATS_MAX} other chats used blocks in the last 10 minutes; try again in {s} s")
        })
    }
}

#[cfg(test)]
impl Rate {
    fn window(&self, chat: &str) -> Option<&std::collections::VecDeque<(Instant, usize)>> {
        self.calls.window(chat)
    }

    fn len(&self) -> usize {
        self.calls.len()
    }

    fn capacity(&self) -> usize {
        self.calls.capacity()
    }
}

#[cfg(test)]
mod tests;
