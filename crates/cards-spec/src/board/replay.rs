//! The open batch's replay (§1.7, §1.8): its block calls in call order onto
//! a state, as work that can pause, one unit of its budget for every event
//! it visits.
//!
//! The batch is read in passes. A pass takes the block calls from where
//! the last one stopped until its index holds [`REPLAY_IDS`] tool ids; it
//! reads the batch on to its end once, for each of those ids its first
//! result and whether a later call took it over; then it applies its calls,
//! or, if it read no block call it couldn't take, the rest of the batch
//! (the last pass). So the index borrows its ids from the history, copies
//! none, and never holds more than [`REPLAY_IDS`] of them
//! ([`REPLAY_MAX_OWNED`] bytes).
//!
//! **Its work, exactly:** every pass reads from its first event to the
//! batch's end, and the passes apply the batch once between them. A batch
//! one pass takes (no block call after the one that fills it) costs two
//! visits an event, results and other tools' events included; each further
//! pass one more read of the rest of the batch. So `p` passes over `E`
//! events cost at most `(1 + p) × E`; a pass before the last takes at
//! least [`REPLAY_IDS`] block calls (more if it sees an id again), so for
//! `C ≥ 1` block calls `p ≤ ⌈C / 256⌉`. Bounded, and every visit budgeted,
//! but not linear when block calls dominate a long batch. The board's commit, [`Board::view`] and [`Board::bound`]
//! run a replay to its end; a [`Refold`] runs its own under the step's
//! budget.

use std::collections::HashMap;

use super::*;

/// The tool ids a replay's index holds, at most: a batch of more block
/// calls is replayed in passes of this many.
pub const REPLAY_IDS: usize = 256;

/// A replay's index at its largest, in owned bytes (D8's model of a hash
/// table: its buckets, each an entry and a control byte, plus a group):
/// 16,912 bytes on 64-bit targets.
pub const REPLAY_MAX_OWNED: usize = 2 * REPLAY_IDS * (size_of::<(&'static str, Seen)>() + 1) + 16;

/// What a replay knows of a tool id of its pass: the last block call of
/// the pass with it, and that call's answer so far: the position of its
/// first result, or [`NO_RESULT`] or [`TAKEN`].
#[derive(Clone, Copy, Debug)]
pub(super) struct Seen {
    call: usize,
    answer: usize,
}

/// No result for the call yet.
const NO_RESULT: usize = usize::MAX;
/// A later call took the tool id over, so the call never gets a result.
const TAKEN: usize = usize::MAX - 1;

/// What the replay makes of a call without its result (§1.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// The commit: unbound, as if its result never came.
    Commit,
    /// The live overlay: pending, and so is every call after it.
    View,
    /// The bound prefix: the replay ends before it.
    Bound,
}

/// A replay of `events[open..end]` (module docs).
#[derive(Clone, Debug)]
pub(super) struct Replay<'e> {
    pub(super) end: usize,
    mode: Mode,
    /// An op's reference: no call after its call, and its call through
    /// that op only.
    stop: Option<Pos>,
    /// The next event to apply.
    at: usize,
    /// The pass's calls are `events[at..to]`.
    to: usize,
    /// The pass has read the batch up to here.
    scan: usize,
    /// The pass still takes the block calls it reads.
    taking: bool,
    /// The pass read a block call it couldn't take: another pass follows.
    more: bool,
    /// A call without its result was drawn (`View`): every call from it on
    /// is pending.
    pending: bool,
    ids: HashMap<&'e str, Seen>,
}

impl<'e> Replay<'e> {
    pub(super) fn new(open: usize, end: usize, mode: Mode, stop: Option<Pos>) -> Self {
        Replay { end, mode, stop, at: open, to: open, scan: open, taking: true, more: false, pending: false, ids: HashMap::new() }
    }

    /// Replays onto `state`, the card fold `cards` as the commit has it,
    /// until done or `budget` is spent (one an event visited): `None` if
    /// it paused. Once done: whether `stop`'s op took effect (its call
    /// bound).
    pub(super) fn run(&mut self, cards: &Fold, state: &mut State, events: &'e [Event], budget: &mut usize) -> Option<bool> {
        while self.at < self.end {
            while self.scan < self.end {
                *budget = budget.checked_sub(1)?;
                let j = self.scan;
                self.scan += 1;
                let taking = self.taking;
                self.read(&events[j], j);
                if taking {
                    self.to = j + 1;
                }
            }
            if !self.more {
                // The last pass: no block call after its calls, so it
                // applies the rest of the batch, never read again.
                self.to = self.end;
            }
            while self.at < self.to {
                *budget = budget.checked_sub(1)?;
                let i = self.at;
                self.at += 1;
                if let Some(reached) = self.apply(cards, state, events, i) {
                    self.at = self.end;
                    return Some(reached);
                }
            }
            self.ids.clear();
            self.scan = self.at;
            self.taking = true;
            self.more = false;
        }
        Some(false)
    }

    /// The pass reads event `j` for its answers.
    fn read(&mut self, event: &'e Event, j: usize) {
        match event {
            Event::Call { tool_id, name, .. } if self.taking && block::NAMES.contains(&name.as_str()) => {
                // An earlier call of the pass with this id: taken over.
                self.ids.insert(tool_id.as_str(), Seen { call: j, answer: NO_RESULT });
                self.taking = self.ids.len() < REPLAY_IDS;
            }
            Event::Call { tool_id, name, .. } => {
                if let Some(s) = self.ids.get_mut(tool_id.as_str()) {
                    s.answer = TAKEN;
                }
                // A block call the full pass can't take: the next one will.
                self.more |= block::NAMES.contains(&name.as_str());
            }
            Event::Result { tool_id, .. } => {
                if let Some(s) = self.ids.get_mut(tool_id.as_str())
                    && s.answer == NO_RESULT
                {
                    s.answer = j;
                }
            }
            Event::ResultsEnd | Event::User { .. } => {}
        }
    }

    /// Applies event `i` if it's a block call: `Some` if the replay ends
    /// there (its stop, or the bound prefix's end), with whether the stop's
    /// op took effect.
    fn apply(&mut self, cards: &Fold, state: &mut State, events: &[Event], i: usize) -> Option<bool> {
        let Event::Call { tool_id, name, input } = &events[i] else { return None };
        if !block::NAMES.contains(&name.as_str()) {
            return None;
        }
        if self.stop.is_some_and(|s| i > s.event) {
            return Some(false);
        }
        let answer = match self.ids.get(tool_id.as_str()) {
            Some(s) if s.call == i => s.answer,
            _ => TAKEN,
        };
        let batch = block::parse_call(input).ok();
        let binds = match (&batch, answer) {
            (None, _) | (_, TAKEN) => false,
            (Some(_), NO_RESULT) => match self.mode {
                Mode::Commit => false,
                Mode::Bound => return Some(false),
                Mode::View => {
                    self.pending = true;
                    true
                }
            },
            (Some(b), at) => match &events[at] {
                Event::Result { text, .. } => block::Receipt::parse(text).is_some_and(|r| r.binds(b)),
                _ => false,
            },
        };
        let ops = match (&batch, self.stop) {
            (Some(b), Some(Pos { event, op: Some(k) })) if event == i => b.ops.get(..=usize::from(k)),
            (Some(b), _) => Some(&b.ops[..]),
            (None, _) => None,
        };
        match ops {
            Some(ops) if binds => {
                let cx = apply::Cx { call: i, tool_id, cards, pending: self.pending };
                state.call(&cx, ops);
                if self.stop.is_some_and(|s| s.event == i) {
                    return Some(true);
                }
            }
            _ => state.unbound(Pos::event(i), self.pending),
        }
        None
    }

    /// Its index's owned bytes (D8's model of a hash table:
    /// [`REPLAY_MAX_OWNED`]).
    pub(super) fn owned_bytes(&self) -> usize {
        match self.ids.capacity() {
            0 => 0,
            cap => (cap * 8 / 7).next_power_of_two() * (size_of::<(&str, Seen)>() + 1) + 16,
        }
    }
}
