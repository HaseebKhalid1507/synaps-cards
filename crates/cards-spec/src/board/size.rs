//! Owned bytes (§1.8): "B2 and D6 count owned bytes, container overhead
//! included, and the caps apply to those counts". An estimate, not an
//! allocator's measurement: a value's inline size plus what it owns on the
//! heap, a `Vec` and a `VecDeque` by capacity, a `BTreeMap` entry at twice
//! its key and value plus a node's share ([`BTREE_ENTRY`]). The card fold
//! keeps its own bounds and isn't counted (§1.8).

use std::mem::size_of;

use super::*;
use crate::block::{Hunk, Num, Step};

/// What a `BTreeMap` entry costs besides its key and value: a node holds
/// 6 to 11 entries, so up to about as much again, plus its pointers.
const BTREE_ENTRY: usize = 16;

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
        let entry = 2 * (size_of::<K>() + size_of::<V>()) + BTREE_ENTRY;
        self.iter().map(|(k, v)| entry + k.heap() + v.heap()).sum()
    }
}

impl Owned for BlockId {
    fn heap(&self) -> usize {
        self.as_str().len()
    }
}

impl Owned for ItemId {
    fn heap(&self) -> usize {
        self.as_str().len()
    }
}

impl Owned for Path {
    fn heap(&self) -> usize {
        self.as_str().len()
    }
}

impl Owned for Ref {
    fn heap(&self) -> usize {
        match self {
            Ref::Block(b) => b.heap(),
            Ref::Card(c) => c.as_str().len(),
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
        self.tool_id.heap() + self.summary.heap()
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

    /// One revision record's and one log line's owned bytes, the largest
    /// kept: each ≤ [`RECORD_MAX_BYTES`].
    pub fn largest_entries(&self) -> (usize, usize) {
        (self.records.iter().map(Owned::owned).max().unwrap_or(0), self.log.iter().map(Owned::owned).max().unwrap_or(0))
    }
}
