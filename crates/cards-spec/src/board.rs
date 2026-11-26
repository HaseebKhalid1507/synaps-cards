//! The board (B2, `docs/blocks.md` v1.3 §1.7–§1.8, §3.4–§3.5): every
//! block's state as a pure function of the session's history, so every desk
//! holding the same history draws the same blocks.
//!
//! [`Board`] reads the card fold's [`Event`]s ([`crate::fold::events`] keeps
//! a block call's input and its result), one at a time, and wraps the card
//! fold ([`Board::cards`]): `card:<id>` ops resolve against it.
//!
//! - **Commit at the result** (§1.7): a block call's ops take effect at the
//!   call's position, once its result binds ([`block::Receipt::binds`]).
//!   The board holds an assistant message's block calls open until it has
//!   read the next message's results, then commits them in call order,
//!   before anything later. A call with no binding result by then is
//!   unbound for good: a plain row, none of its ops applied.
//! - **The live overlay** ([`Board::view`]): the committed board, cloned,
//!   with the open calls read on top in call order. A call with no result
//!   yet is *pending*: its ops apply provisionally, marked, and so does
//!   every op after it. [`Board::bound`] is the bound prefix only, what a
//!   take starts from (§3.2).
//! - **A genuine user message** ([`Event::User`]) is read as an edit
//!   (§3.3, [`message::read`]) and then for its notices (§3.5).
//! - **What it keeps** ([`State`]): live blocks, tombstones, revision
//!   records, visibility records, run titles, edit outcomes and the op log,
//!   each under §1.8's cap with its stated eviction ([`State::owned_bytes`]
//!   counts them).
//! - **Re-folding** ([`Refold`]): a board up to a past position, in slices
//!   of [`REFOLD_SLICE`] events, refused past [`REFOLD_MAX`].
//!
//! **References** ([`Pos`]) are positions in the board's input: an event's
//! index, and an op's index in its call. The board never copies the history:
//! a revision record or a log line holds a reference, and [`op_at`] reads
//! the op back. So the live overlay and the history's fold agree only if
//! they read the same events: the desk feeds the live turn as the history
//! will hold it (an assistant message's calls, then the next message's
//! results, then its text), as [`crate::fold::events`] gives them.
//!
//! **Readings.** Where the design leaves a mechanical choice, this module
//! takes one and says so (`Reading:` on the item). The choices that are the
//! design's to make are marked `Decision (pending Haseeb)`, each in one
//! function, so a different answer is a local change.

use std::collections::{BTreeMap, VecDeque};

use serde_json::Value;

use crate::block::message::{self, Edit, Trailer};
use crate::block::{self, Author, Batch, BlockId, Body, Generation, ItemId, Kind, Op, Path, Ref, Slot, Tag};
use crate::fold::{self, Fold};

pub use crate::fold::Event;

mod apply;
mod edit;
mod size;
mod state;

pub use edit::{Key, keys};
pub use state::*;

/// Tombstones kept: the oldest ending is dropped (§1.8).
pub const TOMBSTONES_MAX: usize = 64;
/// Revision records per live incarnation (§1.8).
pub const RECORDS_PER_BLOCK: usize = 64;
/// Revision records across the board (§1.8).
pub const RECORDS_MAX: usize = 1024;
/// The changed paths a revision record names (§1.8).
pub const RECORD_PATHS_MAX: usize = 4;
/// Visibility records (§3.5, §1.8).
pub const VISIBILITY_MAX: usize = 64;
/// Run titles (§1.8).
pub const RUNS_MAX: usize = 64;
/// Hand-backs whose outcome is kept (§1.8).
pub const OUTCOMES_MAX: usize = 32;
/// Op log entries (§3.8, §1.8).
pub const LOG_MAX: usize = 256;
/// The board's working set, owned bytes with container overhead (§1.8).
pub const WORKING_SET_MAX_BYTES: usize = 1 << 20;
/// A revision record's and a log entry's owned bytes (§1.8).
pub const RECORD_MAX_BYTES: usize = 256;
/// A re-fold yields after this many events (§1.8).
pub const REFOLD_SLICE: usize = 50_000;
/// Past this many events, a re-fold is refused: `too far back to show`.
pub const REFOLD_MAX: usize = 200_000;

/// A reference into the board's input (§1.8): an event's index, and for an
/// agent op, its index in the call. A user message's has no op.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pos {
    pub event: usize,
    pub op: Option<u8>,
}

impl Pos {
    pub fn event(event: usize) -> Self {
        Pos { event, op: None }
    }
}

/// Where a block draws (§1.2): at a call's anchor in the transcript (the
/// position of the op that last placed it; a converted card's is its call),
/// or in a rail slot. Each place holds a list, in the order blocks were
/// placed there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Place {
    Transcript(usize),
    Rail(Slot),
}

impl Place {
    pub fn target(self) -> block::Target {
        match self {
            Place::Transcript(_) => block::Target::Transcript,
            Place::Rail(_) => block::Target::Rail,
        }
    }
}

/// A block call of the open batch: the current assistant message's,
/// waiting for the next message's results.
#[derive(Clone, Debug, PartialEq)]
struct Open {
    tool_id: String,
    at: usize,
    /// `None` if [`block::parse_call`] refuses the input: never bound.
    batch: Option<Batch>,
    /// `Some(binds)` once its result is read.
    result: Option<bool>,
    /// A later call took its tool id over: it never gets a result.
    taken: bool,
}

/// The board of one conversation generation: the history's fold, plus the
/// open batch (module docs).
#[derive(Clone, Debug)]
pub struct Board {
    state: State,
    cards: Fold,
    open: Vec<Open>,
    /// A result has been read since the open batch's first call.
    results: bool,
    at: usize,
}

impl Board {
    /// An empty board of `generation` (§1.2).
    pub fn new(generation: Generation) -> Self {
        Board { state: State::new(generation), cards: Fold::default(), open: Vec::new(), results: false, at: 0 }
    }

    /// The board of a history (`api_messages`, Full): its generation is the
    /// first message's, its events [`fold::events`]'.
    pub fn of_history(messages: &[Value]) -> Self {
        let generation = Generation::of(messages.first().unwrap_or(&Value::Null));
        Self::fold(generation, &fold::events(messages))
    }

    /// The board of `events`, in order.
    pub fn fold<'a>(generation: Generation, events: impl IntoIterator<Item = &'a Event>) -> Self {
        let mut b = Board::new(generation);
        for e in events {
            b.apply(e);
        }
        b
    }

    /// Read one more event.
    pub fn apply(&mut self, event: &Event) {
        let at = self.at;
        self.at += 1;
        match event {
            Event::Call { tool_id, name, input } => {
                // A call after a result starts the next assistant message.
                if self.results {
                    self.commit();
                }
                for o in self.open.iter_mut().filter(|o| &o.tool_id == tool_id) {
                    o.taken = true;
                }
                if block::NAMES.contains(&name.as_str()) {
                    let batch = block::parse_call(input).ok();
                    self.open.push(Open { tool_id: tool_id.clone(), at, batch, result: None, taken: false });
                }
                self.cards.apply(event);
            }
            Event::Result { tool_id, text } => {
                // The tool id's latest call; one taken over never binds.
                if let Some(o) = self.open.iter_mut().rev().find(|o| &o.tool_id == tool_id)
                    && o.result.is_none()
                {
                    let binds = block::Receipt::parse(text).zip(o.batch.as_ref()).is_some_and(|(r, b)| r.binds(b));
                    o.result = Some(binds);
                }
                self.results = true;
                self.cards.apply(event);
            }
            Event::User { text } => {
                self.commit();
                self.cards.apply(event);
                let read = message::read(text);
                if let Some(e) = &read.edit {
                    self.state.edit(Pos::event(at), e);
                }
                self.state.notices(Pos::event(at), &read.trailer);
            }
        }
    }

    /// The open batch's calls, in call order, onto the committed board:
    /// every call, as if each result were in (`last`), or the bound prefix
    /// only (`bound`), or the overlay (neither).
    fn replay(&self, state: &mut State, mode: Replay) {
        let mut pending = false;
        for o in &self.open {
            let binds = match o.result {
                _ if o.taken || o.batch.is_none() => false,
                Some(b) => b,
                None if mode == Replay::Commit => false,
                None if mode == Replay::Bound => return,
                None => {
                    pending = true;
                    true
                }
            };
            match (&o.batch, binds) {
                (Some(batch), true) => {
                    let cx = apply::Cx { call: o.at, tool_id: &o.tool_id, cards: &self.cards, pending };
                    state.call(&cx, batch);
                }
                _ => state.unbound(Pos::event(o.at), pending),
            }
        }
    }

    fn commit(&mut self) {
        let generation = self.state.generation;
        let mut state = std::mem::replace(&mut self.state, State::new(generation));
        self.replay(&mut state, Replay::Commit);
        self.state = state;
        self.open.clear();
        self.results = false;
    }

    /// The board as the desk draws it (§1.7): committed, then the open
    /// calls in call order. A call whose result binds applies; one whose
    /// result doesn't is left out; one without its result yet is pending,
    /// and it and every op after it apply provisionally ([`Block::pending`],
    /// [`Logged::pending`]). With every result in, it is the board the
    /// history will hold.
    pub fn view(&self) -> State {
        let mut s = self.state.clone();
        self.replay(&mut s, Replay::View);
        s
    }

    /// The bound board (§3.2): committed, then the open calls up to the
    /// first without its result. A take starts here, never from pending
    /// state.
    pub fn bound(&self) -> State {
        let mut s = self.state.clone();
        self.replay(&mut s, Replay::Bound);
        s
    }

    /// The card fold it wraps.
    pub fn cards(&self) -> &Fold {
        &self.cards
    }

    /// Events read so far: the next event's position.
    pub fn position(&self) -> usize {
        self.at
    }

    pub fn generation(&self) -> Generation {
        self.state.generation
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Replay {
    Commit,
    View,
    Bound,
}

/// The op a reference names, read back from the board's input (§1.8: a
/// record never holds its op): the call at `at.event`, parsed again, and
/// its op `at.op`. `None` if that isn't a block call's op.
pub fn op_at(events: &[Event], at: Pos) -> Option<Op> {
    let Event::Call { name, input, .. } = events.get(at.event)? else {
        return None;
    };
    if !block::NAMES.contains(&name.as_str()) {
        return None;
    }
    block::parse_call(input).ok()?.ops.into_iter().nth(usize::from(at.op?))
}

/// The edit a hand-back's reference names, read back (§1.8: an outcome
/// keeps one code per [`Key`], not the keys).
pub fn edit_at(events: &[Event], at: Pos) -> Option<Edit> {
    let Event::User { text } = events.get(at.event)? else {
        return None;
    };
    message::read(text).edit
}

/// A board folded up to a past position, cooperatively (§1.8): a revision
/// view, or an evicted outcome's row. [`Refold::step`] reads at most a
/// budget of events, so the desk can yield between slices (it adds its own
/// 30 ms clock).
#[derive(Clone, Debug)]
pub struct Refold<'a> {
    events: &'a [Event],
    until: usize,
    board: Board,
}

/// A re-fold past [`REFOLD_MAX`] events: `too far back to show`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooFar;

impl<'a> Refold<'a> {
    /// A re-fold of `events` up to (not including) `until`.
    pub fn new(generation: Generation, events: &'a [Event], until: usize) -> Result<Self, TooFar> {
        let until = until.min(events.len());
        if until > REFOLD_MAX {
            return Err(TooFar);
        }
        Ok(Refold { events, until, board: Board::new(generation) })
    }

    /// Read up to `budget` more events; `true` once it has reached `until`.
    pub fn step(&mut self, budget: usize) -> bool {
        let from = self.board.position();
        let to = self.until.min(from.saturating_add(budget));
        for e in &self.events[from..to] {
            self.board.apply(e);
        }
        self.board.position() == self.until
    }

    /// The board so far.
    pub fn board(&self) -> &Board {
        &self.board
    }
}

#[cfg(test)]
mod tests;
