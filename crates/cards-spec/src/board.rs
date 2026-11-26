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
//! - **Replaying a batch** (its commit, [`Board::view`], [`Board::bound`]):
//!   in passes of up to [`REPLAY_IDS`] block calls, each reading the rest of
//!   the batch once for its calls' results, through an index that borrows
//!   their tool ids from the history (≤ [`REPLAY_MAX_OWNED`] bytes). A batch
//!   one pass takes costs two visits an event, and each further pass one
//!   more read of the rest of it (its exact work: `board::replay`).
//! - **Re-folding** ([`Refold`]): the board at a revision record's
//!   reference, op by op ([`Target::Revision`]: its batch's binding read
//!   from its results, the card fold as the commit had it), or the whole
//!   history keeping one hand-back's outcome ([`Target::Outcome`]). In
//!   steps of at most their budget of work and [`REFOLD_SLICE`], every
//!   event visited counted (a commit pauses with its step); in one working
//!   set; reading at most [`REFOLD_MAX`] events, the target's batch
//!   included ([`TooFar`]).
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
//!    them (a step can't be preempted, and does at most its budget of
//!    work), one scratch at a time; `too far back to show` on a [`TooFar`]
//!    from a step as from [`Refold::to`].
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
//!   §1.8's table gives each owner's ceiling ([`State::owned_bytes`]); no
//!   past body is kept (the 192 KiB cache is gone): a past revision is the
//!   re-fold scratch's board ([`Refold::owned_bytes`], under its own MiB).
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
mod replay;
mod size;
mod state;

pub use apply::check_stored;
pub use edit::{Key, keys};
use replay::{Mode, Replay};
pub use replay::{REPLAY_IDS, REPLAY_MAX_OWNED};
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
        if self.commits(&events[self.at]) {
            self.commit(events, self.at);
        }
        self.read_event(events);
    }

    /// Whether reading `e` commits the open batch first: `ResultsEnd`, a
    /// user message, or a call after a result (the next assistant
    /// message's).
    fn commits(&self, e: &Event) -> bool {
        match e {
            Event::Call { .. } => self.results,
            Event::Result { .. } => false,
            Event::ResultsEnd | Event::User { .. } => true,
        }
    }

    /// Reads the next event, its batch committed if it commits one.
    fn read_event(&mut self, events: &[Event]) {
        let at = self.at;
        let event = &events[at];
        match event {
            Event::Call { name, .. } => {
                if block::NAMES.contains(&name.as_str()) {
                    self.calls = true;
                }
                self.cards.apply(event);
            }
            Event::Result { .. } => {
                self.results = true;
                self.cards.apply(event);
            }
            Event::ResultsEnd => self.cards.apply(event),
            Event::User { text } => {
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

    /// Commits the open batch, `events[self.open..end]`, replayed to its
    /// end: the next one begins at `end`.
    fn commit(&mut self, events: &[Event], end: usize) {
        if self.calls {
            let mut all = usize::MAX;
            Replay::new(self.open, end, Mode::Commit, None).run(&self.cards, &mut self.state, events, &mut all);
        }
        self.close(end);
    }

    /// The open batch is committed: the next one begins at `end`.
    fn close(&mut self, end: usize) {
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
        self.replayed(events, Mode::View)
    }

    /// The bound board (§3.2): committed, then the open calls up to the
    /// first without its result. A take starts here, never from pending
    /// state.
    pub fn bound(&self, events: &[Event]) -> State {
        self.replayed(events, Mode::Bound)
    }

    fn replayed(&self, events: &[Event], mode: Mode) -> State {
        let mut s = self.state.clone();
        if self.calls {
            let mut all = usize::MAX;
            Replay::new(self.open, self.at, mode, None).run(&self.cards, &mut s, self.history(events), &mut all);
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
/// or an evicted outcome's row, in the re-fold scratch.
///
/// **Work.** [`Refold::step`] does at most its budget of work, and never
/// more than [`REFOLD_SLICE`]: one unit for every event it visits, however
/// it visits it (folding it, looking for the target's batch's end, reading
/// it into the card fold, a replay's read of it for results, applying its
/// call). A batch's commit is a replay that pauses with the step and
/// resumes with the next. So the desk can yield between steps, and adds
/// the 30 ms clock (D5).
///
/// **Extent.** It reads at most the history's first [`REFOLD_MAX`] events,
/// the target's batch included: an op whose batch ends (or whose result
/// comes) past them is [`TooFar`], found by the step that gets there.
///
/// **One working set.** The board it reads becomes the board at the
/// target: the target is materialized in place, never cloned, and the card
/// fold is let go once done. [`Refold::owned_bytes`] counts it all, the
/// replay's index included, the card fold apart.
#[derive(Clone, Debug)]
pub struct Refold<'a> {
    events: &'a [Event],
    target: Target,
    /// The board reads this far, then the target is materialized.
    until: usize,
    /// The board read so far, then the board at the target.
    board: Board,
    work: Work<'a>,
    /// Events visited so far, every kind of work counted.
    visited: usize,
}

/// Where a [`Refold`] is.
#[derive(Clone, Debug)]
enum Work<'a> {
    /// Reading the history up to the target, and the commit of a batch
    /// under way, if any.
    Fold(Option<Replay<'a>>),
    /// An op's revision: its batch's end looked for from `at` (where the
    /// board would commit it: `ResultsEnd`, a user message, a call after a
    /// result), `results` once one has been read.
    End { at: usize, results: bool },
    /// An op's revision: the card fold reads the target's batch,
    /// `[at, end)`, as its commit had it.
    Cards { at: usize, end: usize, mode: Mode },
    /// The open batch replayed to the target.
    Target(Replay<'a>),
    /// Done: whether the target took effect, or too far back.
    Done(Result<bool, TooFar>),
}

/// A re-fold past [`REFOLD_MAX`] events: `too far back to show`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooFar;

impl<'a> Refold<'a> {
    /// A re-fold of `events`, the history the reference is into, to
    /// `target`: [`TooFar`] at once if the target itself lies past
    /// [`REFOLD_MAX`] events.
    pub fn to(generation: Generation, events: &'a [Event], target: Target) -> Result<Self, TooFar> {
        let (until, extent) = match target {
            Target::Revision(Pos { event, op: Some(_) }) => (event, event.saturating_add(1)),
            Target::Revision(Pos { event, op: None }) => (event.saturating_add(1), event.saturating_add(1)),
            Target::Outcome(_) => (events.len(), events.len()),
        };
        if extent.min(events.len()) > REFOLD_MAX {
            return Err(TooFar);
        }
        let mut board = Board::new(generation);
        if let Target::Outcome(at) = target {
            board.state.pinned = Some(at);
        }
        Ok(Refold { events, target, until: until.min(events.len()), board, work: Work::Fold(None), visited: 0 })
    }

    /// Does up to `budget` more work, and never more than [`REFOLD_SLICE`]
    /// (one an event visited): `Ok(true)` once done, [`TooFar`] if the
    /// target's batch runs past [`REFOLD_MAX`] events. A step can't be
    /// preempted, so the desk gives small budgets and checks its 30 ms
    /// clock between steps (§1.8: a yield every 50,000 events or 30 ms,
    /// whichever comes first).
    pub fn step(&mut self, budget: usize) -> Result<bool, TooFar> {
        let mut left = budget.min(REFOLD_SLICE);
        let before = left;
        self.work(&mut left);
        self.visited += before - left;
        match self.work {
            Work::Done(done) => done.map(|_| true),
            _ => Ok(false),
        }
    }

    fn work(&mut self, left: &mut usize) {
        let events = self.events;
        let target = self.target;
        loop {
            let b = &mut self.board;
            let next = match &mut self.work {
                Work::Done(_) => return,
                Work::Fold(commit) => {
                    if let Some(job) = commit {
                        if job.run(&b.cards, &mut b.state, events, left).is_none() {
                            return;
                        }
                        b.close(job.end);
                        *commit = None;
                    }
                    let at = b.at;
                    // An op's call starts the next batch if a result came
                    // before it: that batch commits first.
                    let op = matches!(target, Target::Revision(Pos { op: Some(_), .. }));
                    if (at < self.until || op && at == self.until && at < events.len()) && b.commits(&events[at]) {
                        if b.calls {
                            *commit = Some(Replay::new(b.open, at, Mode::Commit, None));
                            continue;
                        }
                        b.close(at);
                    }
                    if at < self.until {
                        let Some(l) = left.checked_sub(1) else { return };
                        *left = l;
                        b.read_event(events);
                        continue;
                    }
                    match target {
                        Target::Revision(Pos { event, op: Some(_) }) => match events.get(event) {
                            Some(Event::Call { name, .. }) if block::NAMES.contains(&name.as_str()) => {
                                Work::End { at: event + 1, results: false }
                            }
                            _ => Work::Done(Ok(false)),
                        },
                        Target::Revision(Pos { op: None, .. }) if b.calls => {
                            Work::Target(Replay::new(b.open, self.until, Mode::Bound, None))
                        }
                        Target::Outcome(_) if b.calls => Work::Target(Replay::new(b.open, events.len(), Mode::View, None)),
                        _ => Work::Done(Ok(true)),
                    }
                }
                Work::End { at, results } => loop {
                    // With no end yet, only its bound prefix is final.
                    if *at >= events.len() {
                        break Work::Cards { at: target_event(target), end: events.len(), mode: Mode::Bound };
                    }
                    if *at >= REFOLD_MAX {
                        break Work::Done(Err(TooFar));
                    }
                    let Some(l) = left.checked_sub(1) else { return };
                    *left = l;
                    let end = match &events[*at] {
                        Event::ResultsEnd | Event::User { .. } => true,
                        Event::Call { .. } => *results,
                        Event::Result { .. } => {
                            *results = true;
                            false
                        }
                    };
                    if end {
                        break Work::Cards { at: target_event(target), end: *at, mode: Mode::Commit };
                    }
                    *at += 1;
                },
                Work::Cards { at, end, mode } => {
                    while *at < *end {
                        let Some(l) = left.checked_sub(1) else { return };
                        *left = l;
                        b.cards.apply(&events[*at]);
                        *at += 1;
                    }
                    Work::Target(Replay::new(b.open, *end, *mode, Some(target_at(target))))
                }
                Work::Target(job) => {
                    let Some(reached) = job.run(&b.cards, &mut b.state, events, left) else { return };
                    Work::Done(Ok(reached || !matches!(target, Target::Revision(Pos { op: Some(_), .. }))))
                }
            };
            if let Work::Done(done) = &next {
                // One working set (§1.8): the card fold goes, and so does
                // the board if the target never took effect.
                b.cards = Fold::default();
                if *done != Ok(true) {
                    b.state = State::new(b.state.generation);
                }
            }
            self.work = next;
        }
    }

    /// Once done ([`Refold::step`]): the board at the target. `None` if the
    /// target never took effect (its call didn't bind, or it is pending),
    /// or it was too far back.
    pub fn state(&self) -> Option<&State> {
        matches!(self.work, Work::Done(Ok(true))).then_some(&self.board.state)
    }

    /// Events folded so far: the next one's position.
    pub fn position(&self) -> usize {
        self.board.position()
    }

    /// Work done so far: events visited, every kind of visit counted
    /// ([`Refold`]).
    pub fn visited(&self) -> usize {
        self.visited
    }

    /// The replay under way, if any.
    fn replay(&self) -> Option<&Replay<'a>> {
        match &self.work {
            Work::Fold(Some(r)) | Work::Target(r) => Some(r),
            _ => None,
        }
    }
}

fn target_at(target: Target) -> Pos {
    match target {
        Target::Revision(at) | Target::Outcome(at) => at,
    }
}

fn target_event(target: Target) -> usize {
    target_at(target).event
}

#[cfg(test)]
mod tests;
