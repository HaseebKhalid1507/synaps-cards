//! The board (B2, `docs/blocks.md` v1.3 §1.7–§1.8, §3.4–§3.5): every
//! block's state as a pure function of the session's history, so every desk
//! holding the same history draws the same blocks.
//!
//! [`Board`] reads the card fold's [`Event`]s ([`crate::fold::events`] keeps
//! a block call's input and its result) from the history the desk keeps
//! ([`Board::read`]), and wraps the card fold ([`Board::cards`]): `card:<id>`
//! ops resolve against it.
//!
//! - **Commit at the result** (§1.7): a block call's ops take effect at the
//!   call's position, once its result binds ([`block::Receipt::binds`]).
//!   The board holds an assistant message's block calls open until it has
//!   read the next message's results, then commits them in call order,
//!   before anything later. A call with no binding result by then is
//!   unbound for good: a plain row, none of its ops applied. "By then" is
//!   [`Event::ResultsEnd`], which [`crate::fold::events`] puts where the
//!   next message's results end, whatever else that message holds. A
//!   stream without it still commits at a genuine user message, or at a
//!   call after a result (that is the next assistant message); but where
//!   the next message gave no event (another tool's results only, a result
//!   with no text), only `ResultsEnd` closes the batch.
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
//!   counts them, and [`Board::owned_bytes`] everything the board keeps but
//!   the card fold, which [`Board::card_fold_bytes`] reports apart).
//! - **Re-folding** ([`Refold`]): the board at a revision record's
//!   reference, op by op ([`Target::Revision`]: its batch's binding read
//!   from its results, the card fold as the commit had it), or the whole
//!   history keeping one hand-back's outcome ([`Target::Outcome`]); in
//!   slices of at most [`REFOLD_SLICE`] events, refused past [`REFOLD_MAX`].
//!
//! **References** ([`Pos`]) are positions in the board's input: an event's
//! index, and an op's index in its call. The board never copies the history:
//! a revision record or a log line holds a reference, and [`op_at`] reads
//! the op back. So the live overlay and the history's fold agree only if
//! they read the same events.
//!
//! **What the desk's live adapter must guarantee (D5).** The board is exact
//! for the events it reads; the adapter makes the live events the history's:
//! 1. one canonical sequence for the stream, a settled replay and a
//!    reattach: an assistant message's calls in model order (every tool's,
//!    as each takes its tool id), then the next message's final results,
//!    then [`Event::ResultsEnd`], then genuine user text, exactly as
//!    [`crate::fold::events`] extracts them (the same result filter, the
//!    host's saved-block unwrapping, the same user-text admission). An event
//!    the history won't hold moves every later reference;
//! 2. only a call's whole input and a result's final text: no partial JSON,
//!    no output chunks, no second event for a call whose input changed
//!    (rebuild from the stable prefix instead);
//! 3. `ResultsEnd` when the round's results are final (cancelled ones
//!    included), before a steer lands at its `SteeringDelivered`: nothing
//!    the user sends falls inside a batch;
//! 4. a take from [`Board::bound`], never from [`Board::view`]; pending
//!    blocks drawn without revision numbers; the overlay recomputed when a
//!    live event changes it, not every frame;
//! 5. a rollback, a settled history that replaces the live one, or a
//!    compaction: fold again from where they diverge (a reference never
//!    survives another history under the same positions), the generation
//!    from the real first message;
//! 6. re-folds ([`Refold`]) in small steps, the 30 ms clock checked between
//!    them (a step can't be preempted), one scratch at a time.
//!
//! **Decisions.** Where the design left a contract or something the user
//! sees open, B2 took a default, made in one function marked `Decision Dn`.
//! D2, D4 and D6 read the design as written. D1, D3, D5, D7 and D8 amend
//! `docs/blocks.md` (each its own commit, so another answer is a local
//! change); Haseeb's card on them is pending:
//! - D1 (§1.5): a resolved card has no block until a `replace` converts it,
//!   so `patch`, `collapse`, `move` and `remove` on it skip (`no block
//!   yet`);
//! - D2 (§1.3): a `replace` without a title keeps the block's;
//! - D3 (§1.4, §1.8): a list's and a body's caps are checked per path: the
//!   path that would pass one is skipped, and the patch's other paths apply;
//! - D4 (§3.4): a user's skip of a skipped step, or unskip of a todo one, is
//!   `unchanged`, not a conflict;
//! - D5 (§1.2): a `move` with only `at` joins the `after` block's list,
//!   whatever its target (a card's into the rail is skipped);
//! - D6 (§3.4): what overwrites the user's edit (`changed your edit`): the
//!   agent's `set` of the path, `order` of the list, or `remove` of the
//!   item, each only where it changes the value;
//! - D7 (§1.5): a stored plan may lose its last step (1–20 is a `put`'s and
//!   a `replace`'s);
//! - D8 (§1.8): the caps are owned bytes, container overhead included, and
//!   §1.8's table gives each owner's ceiling ([`State::owned_bytes`]).
//!
//! **Readings** (mechanical, `Reading:` where they're made):
//! - an assistant message after an assistant message (no results between)
//!   is the next message: its `ResultsEnd` comes before its calls, so the
//!   first one's calls are unbound;
//! - a card closed in the same batch is resolved at the commit (the card
//!   fold has read the batch's results); one asked in it is still open;
//! - a call that doesn't bind logs one line, `unbound`, for all its ops;
//! - an agent append's live ids are no-ops (idempotent), and a spent id or
//!   the 65th id skips the list's whole path;
//! - a new group sits at its first member that qualifies;
//! - a group's membership is its body, so a join or a leave is a revision
//!   of it; at 9,999 revisions a member's move or remove that would revise
//!   it is skipped (`its group is at 9,999 revisions`), and one that ends a
//!   group of two still applies;
//! - a series append changes the block only if the series it leaves
//!   differs; a status's `text` is folded to one line before it is
//!   compared ([`check_stored`] holds every stored body to B1's rules);
//! - a `remove` of an absent id logs `no change`; a `collapse` to the state
//!   it has logs `no change` but still marks the agent's latest fold;
//! - an edit of an id with no live block is `the block is gone`; of another
//!   incarnation, or from beyond the latest revision, `rewritten`;
//! - the highest version seen counts every notice read, stale ones and ids
//!   named twice included;
//! - only the user's last writes are kept: a path the agent wrote last has
//!   no entry;
//! - no past body is materialized: a past revision is a [`Refold`] to its
//!   reference (§1.8's re-fold scratch, the desk's); a record of a call that
//!   never bound, or is still pending, has no revision to show.

use std::collections::{BTreeMap, VecDeque};

use serde_json::Value;

use crate::block::message::{self, Edit, Trailer};
use crate::block::{self, Author, BlockId, Body, Generation, ItemId, Kind, Op, Path, Ref, Slot, Tag};
use crate::fold::{self, Fold};

pub use crate::fold::Event;

mod apply;
mod edit;
mod size;
mod state;

pub use apply::check_stored;
pub use edit::{Key, keys};
pub use size::Largest;
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
/// A live block's owned bytes, at most (§1.8, D8; 64-bit targets): its
/// entry in the board, its id, title and body, its item ids and last
/// writers, and its list's entry. A maximal plan is the largest: 20 steps
/// in 4 KiB, 64 item ids, [`WRITERS_MAX`] writers.
pub const BLOCK_MAX_OWNED: usize = 18 * 1024;
/// The last writers a block keeps (§1.8): a plan's 20 steps, each with its
/// label, state, stop and the user's added item, and the steps' order.
pub const WRITERS_MAX: usize = 4 * block::PLAN_STEPS_MAX + 1;
/// A tombstone's owned bytes, at most (§1.8).
pub const TOMBSTONE_MAX_OWNED: usize = 96;
/// A visibility record's owned bytes, at most (§1.8): its map entry and id.
pub const VISIBILITY_MAX_OWNED: usize = 104;
/// A run title's owned bytes, at most (§1.8): a summary of 80 characters,
/// up to 320 bytes, and its position (no tool id).
pub const RUN_MAX_OWNED: usize = 352;
/// An edit outcome's owned bytes, at most (§1.8): one code per key of an
/// edit within 8 KiB.
pub const OUTCOME_MAX_OWNED: usize = 2 * 1024;
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

/// The board of one conversation generation: the history's fold, plus the
/// open batch (module docs).
///
/// It keeps no copy of the history (§1.8): the open batch is a range of it,
/// `[open, position)`, read again at its commit and by [`Board::view`] and
/// [`Board::bound`]. So the board reads from the history the desk keeps
/// ([`Board::read`]), and every call that reads takes it: the same events,
/// extended, never another history under the same positions. When the
/// history changes under the board (a rollback, a settled history without
/// the live turn, a compaction), the desk folds again from where the two
/// diverge (§1.8).
#[derive(Clone, Debug)]
pub struct Board {
    state: State,
    cards: Fold,
    /// The open batch's first event.
    open: usize,
    /// A block call is in the open batch.
    calls: bool,
    /// A result has been read since the open batch began.
    results: bool,
    at: usize,
}

/// What a block call of the open batch has, read from the history: its
/// result's text, none yet, or a later call took its tool id over (so it
/// never gets one).
enum Answer<'e> {
    Result(&'e str),
    None,
    Taken,
}

/// The answer of the call at `i`, from `events[i + 1..end]`: the first
/// result for its tool id, unless a later call took the id over. Reading:
/// the board scans the batch rather than keep a record per call, so it
/// retains nothing per pending call; a batch's commit costs its length
/// squared in comparisons of tool ids, as a view does.
fn answer(events: &[Event], i: usize, end: usize) -> Answer<'_> {
    let Event::Call { tool_id, .. } = &events[i] else { return Answer::None };
    let mut first = None;
    for e in &events[i + 1..end] {
        match e {
            Event::Call { tool_id: t, .. } if t == tool_id => return Answer::Taken,
            Event::Result { tool_id: t, text } if t == tool_id && first.is_none() => first = Some(text.as_str()),
            _ => {}
        }
    }
    first.map_or(Answer::None, Answer::Result)
}

impl Board {
    /// An empty board of `generation` (§1.2).
    pub fn new(generation: Generation) -> Self {
        Board { state: State::new(generation), cards: Fold::default(), open: 0, calls: false, results: false, at: 0 }
    }

    /// The board of a history (`api_messages`, Full), and its events: its
    /// generation is the first message's, its events [`fold::events`]'.
    pub fn of_history(messages: &[Value]) -> (Self, Vec<Event>) {
        let generation = Generation::of(messages.first().unwrap_or(&Value::Null));
        let events = fold::events(messages);
        (Self::fold(generation, &events), events)
    }

    /// The board of `events`.
    pub fn fold(generation: Generation, events: &[Event]) -> Self {
        let mut b = Board::new(generation);
        b.read(events);
        b
    }

    /// Reads every event of `events` past [`Board::position`]. `events` is
    /// the history the board has read so far, extended.
    ///
    /// # Panics
    ///
    /// If `events` is shorter than what the board has read: it isn't that
    /// history (fold again from where they diverge).
    pub fn read(&mut self, events: &[Event]) {
        assert!(events.len() >= self.at, "the board read {} events of a history of {}: fold it again", self.at, events.len());
        while self.at < events.len() {
            self.read_one(events);
        }
    }

    /// The committed board: the history's fold before the open batch.
    pub fn committed(&self) -> &State {
        &self.state
    }

    fn read_one(&mut self, events: &[Event]) {
        let at = self.at;
        let event = &events[at];
        match event {
            Event::Call { name, .. } => {
                // A call after a result starts the next assistant message.
                if self.results {
                    self.commit(events, at);
                }
                if block::NAMES.contains(&name.as_str()) {
                    self.calls = true;
                }
                self.cards.apply(event);
            }
            Event::Result { .. } => {
                self.results = true;
                self.cards.apply(event);
            }
            Event::ResultsEnd => {
                self.commit(events, at);
                self.cards.apply(event);
            }
            Event::User { text } => {
                self.commit(events, at);
                self.cards.apply(event);
                let read = message::read(text);
                if let Some(e) = &read.edit {
                    self.state.edit(Pos::event(at), e);
                }
                self.state.notices(Pos::event(at), &read.trailer);
            }
        }
        self.at += 1;
    }

    /// The open batch's block calls, in call order, onto `state`: every
    /// call, as if each without its result were unbound (`Commit`), or the
    /// bound prefix only (`Bound`), or the overlay (`View`). The batch is
    /// `events[self.open..end]`.
    ///
    /// With `stop`, an op's reference: no call after its call, and its call
    /// through that op only; `true` if that op was reached (its call bound).
    ///
    /// `card:` ops resolve against `cards`, the card fold as it is at the
    /// commit: the batch's results read (§1.7).
    #[allow(clippy::too_many_arguments)]
    fn replay(&self, cards: &Fold, state: &mut State, events: &[Event], end: usize, mode: Replay, stop: Option<Pos>) -> bool {
        let mut pending = false;
        for i in self.open..end {
            let Event::Call { tool_id, name, input } = &events[i] else { continue };
            if !block::NAMES.contains(&name.as_str()) {
                continue;
            }
            if stop.is_some_and(|s| i > s.event) {
                break;
            }
            let batch = block::parse_call(input).ok();
            let binds = match (&batch, answer(events, i, end)) {
                (None, _) | (_, Answer::Taken) => false,
                (Some(b), Answer::Result(text)) => block::Receipt::parse(text).is_some_and(|r| r.binds(b)),
                (Some(_), Answer::None) => match mode {
                    Replay::Commit => false,
                    Replay::Bound => return false,
                    Replay::View => {
                        pending = true;
                        true
                    }
                },
            };
            let ops = match (&batch, stop) {
                (Some(b), Some(Pos { event, op: Some(k) })) if event == i => b.ops.get(..=usize::from(k)),
                (Some(b), _) => Some(&b.ops[..]),
                (None, _) => None,
            };
            match ops {
                Some(ops) if binds => {
                    let cx = apply::Cx { call: i, tool_id, cards, pending };
                    state.call(&cx, ops);
                    if stop.is_some_and(|s| s.event == i) {
                        return true;
                    }
                }
                _ => state.unbound(Pos::event(i), pending),
            }
        }
        false
    }

    /// Commits the open batch, `events[self.open..end]`: the next one
    /// begins at `end`.
    fn commit(&mut self, events: &[Event], end: usize) {
        if self.calls {
            let generation = self.state.generation;
            let mut state = std::mem::replace(&mut self.state, State::new(generation));
            self.replay(&self.cards, &mut state, events, end, Replay::Commit, None);
            self.state = state;
        }
        self.open = end;
        self.calls = false;
        self.results = false;
    }

    fn history<'e>(&self, events: &'e [Event]) -> &'e [Event] {
        assert!(events.len() >= self.at, "the board read {} events of a history of {}: fold it again", self.at, events.len());
        &events[..self.at]
    }

    /// The board as the desk draws it (§1.7): committed, then the open
    /// calls in call order. A call whose result binds applies; one whose
    /// result doesn't is left out; one without its result yet is pending,
    /// and it and every op after it apply provisionally ([`Block::pending`],
    /// [`Logged::pending`]). With every result in, it is the board the
    /// history will hold. `events` is the history the board read.
    pub fn view(&self, events: &[Event]) -> State {
        let mut s = self.state.clone();
        if self.calls {
            self.replay(&self.cards, &mut s, self.history(events), self.at, Replay::View, None);
        }
        s
    }

    /// The bound board (§3.2): committed, then the open calls up to the
    /// first without its result. A take starts here, never from pending
    /// state.
    pub fn bound(&self, events: &[Event]) -> State {
        let mut s = self.state.clone();
        if self.calls {
            self.replay(&self.cards, &mut s, self.history(events), self.at, Replay::Bound, None);
        }
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

/// What a re-fold materializes (§1.8's scratch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A revision (§3.6): the board right after the op or the user message
    /// at this reference took effect, a [`Record`]'s `at`. An op's call is
    /// read as its batch commits it: every call before it in the batch, its
    /// binding read from its result wherever that lies in the next message,
    /// then its own ops through this one, and nothing after. A user
    /// message's is the board after its edit and notices.
    Revision(Pos),
    /// A hand-back's outcome (§3.7), `at` the user message's: the whole
    /// history, keeping that outcome however many later hand-backs come, so
    /// the agent's later writes mark it as they would have.
    Outcome(Pos),
}

/// A board folded to a [`Target`], cooperatively (§1.8): a revision view,
/// or an evicted outcome's row. [`Refold::step`] reads at most its budget
/// and at most [`REFOLD_SLICE`] events, so the desk can yield between
/// slices; it adds the 30 ms clock (D5). The step that reaches the target
/// also reads the target's batch.
#[derive(Clone, Debug)]
pub struct Refold<'a> {
    events: &'a [Event],
    target: Target,
    /// The board reads this far, then the target is materialized.
    until: usize,
    board: Board,
    /// Once done: the board at the target, if the target took effect.
    done: Option<Option<State>>,
}

/// A re-fold past [`REFOLD_MAX`] events: `too far back to show`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooFar;

impl<'a> Refold<'a> {
    /// A re-fold of `events`, the history the reference is into, to
    /// `target`.
    pub fn to(generation: Generation, events: &'a [Event], target: Target) -> Result<Self, TooFar> {
        let until = match target {
            Target::Revision(Pos { event, op: Some(_) }) => event,
            Target::Revision(Pos { event, op: None }) => event.saturating_add(1),
            Target::Outcome(_) => events.len(),
        }
        .min(events.len());
        if until > REFOLD_MAX {
            return Err(TooFar);
        }
        let mut board = Board::new(generation);
        if let Target::Outcome(at) = target {
            board.state.pinned = Some(at);
        }
        Ok(Refold { events, target, until, board, done: None })
    }

    /// Read up to `budget` more events, and never more than
    /// [`REFOLD_SLICE`]; `true` once done. A step can't be preempted, so the
    /// desk gives small budgets and checks its 30 ms clock between steps
    /// (§1.8: a yield every 50,000 events or 30 ms, whichever comes first).
    pub fn step(&mut self, budget: usize) -> bool {
        if self.done.is_some() {
            return true;
        }
        let to = self.until.min(self.board.position().saturating_add(budget.min(REFOLD_SLICE)));
        self.board.read(&self.events[..to]);
        if self.board.position() < self.until {
            return false;
        }
        self.done = Some(self.finish());
        // One working set (§1.8): the board's was moved into the result,
        // and the card fold is let go.
        self.board.cards = Fold::default();
        true
    }

    /// The board at the target, materialized in the board's own working
    /// set: moved out of it, never cloned.
    fn finish(&mut self) -> Option<State> {
        let events = self.events;
        let generation = self.board.state.generation;
        let take = |b: &mut Board| std::mem::replace(&mut b.state, State::new(generation));
        match self.target {
            Target::Revision(at @ Pos { event, op: Some(_) }) => {
                let Some(Event::Call { name, .. }) = events.get(event) else { return None };
                if !block::NAMES.contains(&name.as_str()) {
                    return None;
                }
                // The call starts the next batch if a result came before it.
                if self.board.results {
                    self.board.commit(events, event);
                }
                // Its batch ends where the board would commit it; with no
                // end yet, only its bound prefix is final.
                let mut results = false;
                let mut end = None;
                for (j, e) in events.iter().enumerate().skip(event + 1) {
                    match e {
                        Event::ResultsEnd | Event::User { .. } => end = Some(j),
                        Event::Call { .. } if results => end = Some(j),
                        Event::Result { .. } => results = true,
                        Event::Call { .. } => {}
                    }
                    if end.is_some() {
                        break;
                    }
                }
                let (end, mode) = end.map_or((events.len(), Replay::Bound), |e| (e, Replay::Commit));
                // The card fold as the commit has it: the batch read.
                for e in &events[event..end] {
                    self.board.cards.apply(e);
                }
                let mut s = take(&mut self.board);
                self.board.replay(&self.board.cards, &mut s, events, end, mode, Some(at)).then_some(s)
            }
            Target::Revision(Pos { op: None, .. }) => {
                let mut s = take(&mut self.board);
                if self.board.calls {
                    self.board.replay(&self.board.cards, &mut s, &events[..self.until], self.until, Replay::Bound, None);
                }
                Some(s)
            }
            Target::Outcome(_) => {
                let mut s = take(&mut self.board);
                if self.board.calls {
                    self.board.replay(&self.board.cards, &mut s, events, events.len(), Replay::View, None);
                }
                Some(s)
            }
        }
    }

    /// Once done ([`Refold::step`]): the board at the target. `None` if the
    /// target never took effect: its call didn't bind, or it is pending.
    pub fn state(&self) -> Option<&State> {
        self.done.as_ref()?.as_ref()
    }

    /// The board read so far.
    pub fn board(&self) -> &Board {
        &self.board
    }
}

#[cfg(test)]
mod tests;
