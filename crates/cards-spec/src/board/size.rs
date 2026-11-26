//! Owned bytes (§1.8): "B2 and D6 count owned bytes, container overhead
//! included, and the caps apply to those counts". A model, not an
//! allocator's measurement: a value's inline size plus what it owns on the
//! heap, a `String`, a `Vec` and a `VecDeque` by capacity, a `BTreeMap`
//! entry at twice its key and value plus a node's share ([`BTREE_ENTRY`]).
//! A block gives back what its strings and vectors hold past their length
//! after every change ([`Block::compact`]), and a deque keeps its capacity at
//! its cap ([`push_capped`]), so a count is its content's and its ceiling is
//! fixed. The card fold isn't counted: [`Board::card_fold_bytes`] reports it
//! apart.
//!
//! Decision D8 (§1.8): the caps are these owned bytes, and §1.8's table
//! gives each owner's ceiling, for 64-bit targets: [`BLOCK_MAX_OWNED`] a
//! live block (its item ids and last writers included, so nothing is
//! charged twice), [`TOMBSTONE_MAX_OWNED`], [`RECORD_MAX_BYTES`] a record
//! and a log line, [`VISIBILITY_MAX_OWNED`], [`RUN_MAX_OWNED`] and
//! [`OUTCOME_MAX_OWNED`]. At their counts they sum under
//! [`WORKING_SET_MAX_BYTES`]; the open batch is a range of the history (no
//! bytes per call), a run is keyed by its call's position (no tool id), and
//! no past body is kept: a revision view is the re-fold scratch's one
//! working set ([`Refold::owned_bytes`]), which with its replay's index
//! ([`REPLAY_MAX_OWNED`]) sums under the same MiB.

use std::mem::size_of;

use super::*;
use crate::block::{Hunk, Num, Step};

/// What a `BTreeMap` entry costs besides its key and value: a node holds
/// 6 to 11 entries, so up to about as much again, plus its pointers.
const BTREE_ENTRY: usize = 16;

/// A listed block's share of its place's list, at most: the place's entry
/// (as if it had the place to itself), its slot in the list, and its id.
const LIST_ENTRY_MAX_OWNED: usize =
    2 * (size_of::<Place>() + size_of::<Vec<Ref>>()) + BTREE_ENTRY + size_of::<Ref>() + crate::block::BLOCK_ID_MAX;

/// Each owner's largest entry, in owned bytes ([`State::largest_entries`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Largest {
    /// A live block, its item ids, last writers and list entry included:
    /// ≤ [`BLOCK_MAX_OWNED`].
    pub block: usize,
    /// ≤ [`TOMBSTONE_MAX_OWNED`].
    pub tombstone: usize,
    /// ≤ [`RECORD_MAX_BYTES`].
    pub record: usize,
    /// ≤ [`VISIBILITY_MAX_OWNED`].
    pub visibility: usize,
    /// ≤ [`RUN_MAX_OWNED`].
    pub run: usize,
    /// ≤ [`OUTCOME_MAX_OWNED`].
    pub outcome: usize,
    /// A log line: ≤ [`RECORD_MAX_BYTES`].
    pub log: usize,
}

/// A value's heap bytes: what it owns besides its inline size.
pub(crate) trait Owned {
    fn heap(&self) -> usize;

    fn owned(&self) -> usize
    where
        Self: Sized,
    {
        size_of::<Self>() + self.heap()
    }
}

macro_rules! inline {
    ($($t:ty),*) => { $(impl Owned for $t { fn heap(&self) -> usize { 0 } })* };
}
inline!(bool, u8, u16, u32, u64, usize, Pos, Place, Tag, Generation, Kind, Author, Did, Code, Why, PathWhy, EditResult, Vis, Writer, Num);

impl Owned for String {
    fn heap(&self) -> usize {
        self.capacity()
    }
}

impl Owned for Box<str> {
    fn heap(&self) -> usize {
        self.len()
    }
}

impl<T: Owned> Owned for Option<T> {
    fn heap(&self) -> usize {
        self.as_ref().map_or(0, Owned::heap)
    }
}

impl<T: Owned> Owned for Vec<T> {
    fn heap(&self) -> usize {
        self.capacity() * size_of::<T>() + self.iter().map(Owned::heap).sum::<usize>()
    }
}

impl<T: Owned> Owned for VecDeque<T> {
    fn heap(&self) -> usize {
        self.capacity() * size_of::<T>() + self.iter().map(Owned::heap).sum::<usize>()
    }
}

impl<K: Owned, V: Owned> Owned for BTreeMap<K, V> {
    fn heap(&self) -> usize {
        self.iter().map(|(k, v)| entry::<K, V>() + k.heap() + v.heap()).sum()
    }
}

impl Owned for BlockId {
    fn heap(&self) -> usize {
        self.capacity()
    }
}

impl Owned for ItemId {
    fn heap(&self) -> usize {
        self.capacity()
    }
}

impl Owned for Path {
    fn heap(&self) -> usize {
        self.capacity()
    }
}

impl Owned for Ref {
    fn heap(&self) -> usize {
        match self {
            Ref::Block(b) => b.heap(),
            Ref::Card(c) => c.capacity(),
        }
    }
}

impl Owned for Step {
    fn heap(&self) -> usize {
        self.id.heap() + self.label.heap() + self.note.heap()
    }
}

impl Owned for Hunk {
    fn heap(&self) -> usize {
        self.id.heap() + self.at.heap() + self.lines.heap()
    }
}

impl Owned for Body {
    fn heap(&self) -> usize {
        match self {
            Body::Plan(p) => p.steps.heap(),
            Body::Status(s) => s.text.heap() + s.detail.heap(),
            Body::Diff(d) => d.file.heap() + d.hunks.heap(),
            Body::Note(n) => n.text.heap(),
            Body::Metric(m) => m.label.heap() + m.value.heap() + m.was.heap() + m.unit.heap() + m.series.heap(),
            Body::File(f) => f.path.heap() + f.note.heap(),
            Body::Group(g) => g.members.heap(),
        }
    }
}

impl Owned for Items {
    fn heap(&self) -> usize {
        self.0.capacity() * size_of::<(ItemId, Option<u32>)>() + self.0.iter().map(|(i, _)| i.heap()).sum::<usize>()
    }
}

impl Owned for Writers {
    fn heap(&self) -> usize {
        self.0.capacity() * size_of::<(Path, Writer)>() + self.0.iter().map(|(p, _)| p.heap()).sum::<usize>()
    }
}

impl Block {
    /// Gives back what its strings and vectors hold past their length, so
    /// its owned bytes are its content's (an append or a sanitized string
    /// can leave more). Run after every change to it.
    pub(crate) fn compact(&mut self) {
        fn opt(s: &mut Option<String>) {
            if let Some(s) = s {
                s.shrink_to_fit();
            }
        }
        opt(&mut self.title);
        match &mut self.body {
            Body::Plan(p) => {
                p.steps.shrink_to_fit();
                for s in &mut p.steps {
                    s.label.shrink_to_fit();
                    opt(&mut s.note);
                }
            }
            Body::Status(s) => {
                s.text.shrink_to_fit();
                opt(&mut s.detail);
            }
            Body::Diff(d) => {
                d.file.shrink_to_fit();
                d.hunks.shrink_to_fit();
                for h in &mut d.hunks {
                    opt(&mut h.at);
                    h.lines.shrink_to_fit();
                    for l in &mut h.lines {
                        l.shrink_to_fit();
                    }
                }
            }
            Body::Note(n) => n.text.shrink_to_fit(),
            Body::Metric(m) => {
                m.label.shrink_to_fit();
                m.value.shrink_to_fit();
                opt(&mut m.was);
                opt(&mut m.unit);
                m.series.shrink_to_fit();
            }
            Body::File(f) => {
                f.path.shrink_to_fit();
                opt(&mut f.note);
            }
            Body::Group(g) => g.members.shrink_to_fit(),
        }
        self.items.0.shrink_to_fit();
        self.writers.0.shrink_to_fit();
    }

    /// Its last writers (≤ [`WRITERS_MAX`]).
    pub fn writer_count(&self) -> usize {
        self.writers.0.len()
    }
}

/// A `BTreeMap` entry's owned bytes besides what its key and value own.
fn entry<K, V>() -> usize {
    2 * (size_of::<K>() + size_of::<V>()) + BTREE_ENTRY
}

impl Owned for Block {
    fn heap(&self) -> usize {
        self.id.heap()
            + self.group.heap()
            + self.title.heap()
            + self.body.heap()
            + self.items.heap()
            + self.writers.heap()
            + self.changed_your_edit.heap()
    }
}

impl Owned for Tombstone {
    fn heap(&self) -> usize {
        self.id.heap()
    }
}

impl Owned for Record {
    fn heap(&self) -> usize {
        self.paths.heap()
    }
}

impl Owned for Run {
    fn heap(&self) -> usize {
        self.summary.heap()
    }
}

impl Owned for EditOutcome {
    fn heap(&self) -> usize {
        self.id.heap() + self.codes.heap()
    }
}

impl Owned for Outcome {
    fn heap(&self) -> usize {
        match self {
            Outcome::SkippedPath(p, _) => p.heap(),
            _ => 0,
        }
    }
}

impl Owned for Logged {
    fn heap(&self) -> usize {
        self.id.heap() + self.outcome.heap()
    }
}

impl Board {
    /// What the board retains besides the card fold, in owned bytes: the
    /// committed working set, and the open batch, which is a range of the
    /// history (two positions and two flags), so nothing per call.
    pub fn owned_bytes(&self) -> usize {
        size_of::<Board>() - size_of::<State>() - size_of::<Fold>() + self.state.owned_bytes()
    }

    /// The card fold it wraps, apart: an estimate
    /// ([`Fold::retained_estimate`]). It grows with the history's cards, as
    /// it did before blocks, and no cap of §1.8 applies to it.
    pub fn card_fold_bytes(&self) -> usize {
        self.cards.retained_estimate()
    }
}

impl Refold<'_> {
    /// What the re-fold scratch owns besides the card fold it reads, in
    /// owned bytes (§1.8: one at a time, ≤ 1 MiB): one working set (the
    /// board it reads, which becomes the board at its target), and its
    /// replay's index while a batch replays (≤ [`REPLAY_MAX_OWNED`]).
    pub fn owned_bytes(&self) -> usize {
        size_of::<Self>() - size_of::<Board>() + self.board.owned_bytes() + self.replay().map_or(0, |r| r.owned_bytes())
    }
}

impl State {
    /// The working set's owned bytes (§1.8): at most
    /// [`WORKING_SET_MAX_BYTES`], an invariant the property tests check.
    pub fn owned_bytes(&self) -> usize {
        size_of::<State>()
            + self.blocks.heap()
            + self.lists.heap()
            + self.tombstones.heap()
            + self.records.heap()
            + self.visibility.records.heap()
            + self.runs.heap()
            + self.outcomes.heap()
            + self.log.heap()
    }

    /// Each owner's owned bytes, for the tests: blocks, tombstones,
    /// records, visibility, runs, outcomes, log.
    pub fn owned_by_owner(&self) -> [usize; 7] {
        [
            self.blocks.heap() + self.lists.heap(),
            self.tombstones.heap(),
            self.records.heap(),
            self.visibility.records.heap(),
            self.runs.heap(),
            self.outcomes.heap(),
            self.log.heap(),
        ]
    }

    /// Each owner's largest entry, in owned bytes, against its ceiling
    /// (§1.8, D8).
    pub fn largest_entries(&self) -> Largest {
        fn max<T>(it: impl Iterator<Item = T>, f: impl Fn(T) -> usize) -> usize {
            it.map(f).max().unwrap_or(0)
        }
        Largest {
            block: max(self.blocks.iter(), |(k, b)| entry::<Ref, Block>() + k.heap() + b.heap() + LIST_ENTRY_MAX_OWNED),
            tombstone: max(self.tombstones.iter(), Owned::owned),
            record: max(self.records.iter(), Owned::owned),
            visibility: max(self.visibility.records.keys(), |id| entry::<BlockId, Vis>() + id.heap()),
            run: max(self.runs.iter(), Owned::owned),
            outcome: max(self.outcomes.iter(), Owned::owned),
            log: max(self.log.iter(), Owned::owned),
        }
    }
}
