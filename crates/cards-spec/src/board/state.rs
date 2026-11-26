//! The board's working set (§1.8): what it keeps, and the caps on it.

use super::*;

/// The board's working set: every owner of §1.8's table, each under its cap.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub(crate) generation: Generation,
    /// The creation number's counter (§1.2): every incarnation that started.
    pub(crate) creations: u64,
    pub(crate) blocks: BTreeMap<Ref, Block>,
    /// Each place's list: its blocks that aren't group members, in order.
    pub(crate) lists: BTreeMap<Place, Vec<Ref>>,
    pub(crate) tombstones: VecDeque<Tombstone>,
    /// Revision records, oldest first, of live and ended incarnations.
    pub(crate) records: VecDeque<Record>,
    pub(crate) visibility: Visibility,
    pub(crate) runs: VecDeque<Run>,
    pub(crate) outcomes: VecDeque<EditOutcome>,
    pub(crate) log: VecDeque<Logged>,
    /// Applying a pending call's ops (the overlay): everything they touch is
    /// marked.
    pub(crate) pending: bool,
    /// A re-fold's requested outcome ([`Target::Outcome`]): never evicted.
    pub(crate) pinned: Option<Pos>,
}

/// A live block: the id's slot (place, group, fold) and its current
/// incarnation (§1.2).
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: Ref,
    // The id's slot.
    pub place: Place,
    /// The group it is a member of: it draws inside it, not in a list.
    pub group: Option<BlockId>,
    pub collapsed: bool,
    /// The latest `collapse` op on it: the user's local fold wins until the
    /// agent's next one (§3.5).
    pub folded_at: Option<Pos>,
    // The incarnation.
    pub tag: Tag,
    pub creation: u64,
    /// The op that started the incarnation.
    pub created: Pos,
    pub kind: Kind,
    pub title: Option<String>,
    pub body: Body,
    /// The latest revision, `rN`.
    pub rev: u32,
    /// The `replace` that started it, if one did: `r1 · replaced`, its
    /// `why` read at the reference.
    pub replaced: Option<Pos>,
    /// Every item id it named, live or removed at a revision (§1.4).
    pub(crate) items: Items,
    /// The paths the user wrote last (§1.8's last writers).
    pub(crate) writers: Writers,
    /// The agent overwrote a path the user wrote last (§3.4 step 6): the
    /// header says `changed your edit` until the user focuses it.
    pub changed_your_edit: Option<Path>,
    /// The latest op on it that was skipped (the dim `skipped` chip, §1.3).
    pub skipped: Option<Pos>,
    /// A pending op touched it (the overlay): it draws provisionally, with
    /// no revision number (§1.7).
    pub pending: bool,
}

impl Block {
    /// Whether `id` names one of its items now.
    pub fn has_item(&self, id: &ItemId) -> bool {
        self.items.get(id) == Some(None)
    }

    /// The revision `id` was removed at, if it was.
    pub fn removed_at(&self, id: &ItemId) -> Option<u32> {
        self.items.get(id).flatten()
    }

    /// Item ids named over its life, live and removed (≤ [`block::ITEM_IDS_MAX`]).
    pub fn item_ids(&self) -> usize {
        self.items.len()
    }

    /// The user's write of `path`, if the user wrote it last: the hand-back
    /// and its key. A path is a field, `list.item.field`, an added item
    /// (`steps.u-k3x7qa`), or a list's order as the list's name (`steps`).
    /// Reading: the agent's writes aren't kept, as no rule needs them; a
    /// path the agent wrote last has no entry.
    pub fn user_wrote(&self, path: &Path) -> Option<Writer> {
        self.writers.get(path)
    }

    /// The group's members, in order; empty unless it is a group.
    pub fn members(&self) -> &[BlockId] {
        match &self.body {
            Body::Group(g) => &g.members,
            _ => &[],
        }
    }
}

/// A user's write: the hand-back, and which of its keys ([`keys`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Writer {
    pub at: Pos,
    pub key: u16,
}

/// An incarnation's item ids (§1.4), sorted: live (`None`), or removed at
/// a revision. At most [`block::ITEM_IDS_MAX`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Items(pub(crate) Vec<(ItemId, Option<u32>)>);

impl Items {
    pub(crate) fn get(&self, id: &ItemId) -> Option<Option<u32>> {
        self.0.binary_search_by(|(i, _)| i.cmp(id)).ok().map(|k| self.0[k].1)
    }

    pub(crate) fn contains(&self, id: &ItemId) -> bool {
        self.get(id).is_some()
    }

    pub(crate) fn insert(&mut self, id: ItemId, v: Option<u32>) {
        match self.0.binary_search_by(|(i, _)| i.cmp(&id)) {
            Ok(k) => self.0[k].1 = v,
            Err(k) => self.0.insert(k, (id, v)),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

/// The paths the user wrote last, sorted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Writers(pub(crate) Vec<(Path, Writer)>);

impl Writers {
    pub(crate) fn get(&self, p: &Path) -> Option<Writer> {
        self.0.binary_search_by(|(q, _)| q.cmp(p)).ok().map(|k| self.0[k].1)
    }

    pub(crate) fn insert(&mut self, p: Path, w: Writer) {
        match self.0.binary_search_by(|(q, _)| q.cmp(&p)) {
            Ok(k) => self.0[k].1 = w,
            Err(k) => self.0.insert(k, (p, w)),
        }
    }

    pub(crate) fn remove(&mut self, p: &Path) -> Option<Writer> {
        let k = self.0.binary_search_by(|(q, _)| q.cmp(p)).ok()?;
        Some(self.0.remove(k).1)
    }

    /// Takes out every write inside item `item` (`steps.fix`), the item's
    /// own included.
    pub(crate) fn remove_item(&mut self, item: &str) -> Vec<(Path, Writer)> {
        let (gone, kept) =
            std::mem::take(&mut self.0).into_iter().partition(|(q, _)| q.as_str() == item || q.item_path().as_deref() == Some(item));
        self.0 = kept;
        gone
    }
}

/// Pushes `v`, dropping the oldest past `cap`, and keeps the deque's
/// capacity (its owned bytes) at `cap`.
pub(crate) fn push_capped<T>(dq: &mut VecDeque<T>, v: T, cap: usize) {
    if dq.len() >= cap {
        dq.pop_front();
    }
    dq.push_back(v);
    if dq.capacity() > cap {
        dq.shrink_to(cap);
    }
}

/// An ended incarnation (§1.8): its id, tag, last revision, and the op that
/// ended it (its `why` is read there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub id: Ref,
    pub tag: Tag,
    pub rev: u32,
    pub ended: Pos,
}

/// One revision (§1.8): a reference, never the op itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub tag: Tag,
    pub rev: u32,
    pub at: Pos,
    pub author: Author,
    pub did: Did,
    /// How many paths it changed.
    pub changed: u16,
    /// The first ≤ [`RECORD_PATHS_MAX`] of them, space-separated.
    pub(crate) paths: Box<str>,
}

impl Record {
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.paths.split(' ').filter(|p| !p.is_empty())
    }
}

/// The visibility records (§3.5), the floor and the highest version seen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visibility {
    pub(crate) records: BTreeMap<BlockId, Vis>,
    pub(crate) floor: u32,
    pub(crate) highest: u32,
}

/// One id's record: dismissed or shown, and the version that set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vis {
    pub dismissed: bool,
    pub version: u32,
}

impl Visibility {
    /// The version a notice for `id` must be above to apply (§3.5): its
    /// record's, else the floor. The fold's admission and every desk rule
    /// use this one helper.
    pub fn effective_version(&self, id: &BlockId) -> u32 {
        self.records.get(id).map_or(self.floor, |r| r.version)
    }

    pub fn record(&self, id: &BlockId) -> Option<Vis> {
        self.records.get(id).copied()
    }

    pub fn dismissed(&self, id: &BlockId) -> bool {
        self.records.get(id).is_some_and(|r| r.dismissed)
    }

    /// The highest version of any evicted record.
    pub fn floor(&self) -> u32 {
        self.floor
    }

    /// The highest version in any notice of this generation's history: a
    /// desk mints one more (§3.5).
    pub fn highest(&self) -> u32 {
        self.highest
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// A run title (`collapse` with `run`, §1.3): its summary, and the latest
/// op that wrote it. The call at `at.event` is the one whose run it titles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub summary: Box<str>,
    pub at: Pos,
}

/// What a hand-back did (§3.7): one code per key, in [`keys`]' order; the
/// keys are read from the message at `at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditOutcome {
    pub at: Pos,
    pub id: Ref,
    pub result: EditResult,
    pub codes: Vec<Code>,
}

/// An edit's outcome as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditResult {
    /// Admitted: each key has its code.
    Applied,
    /// `of` names another incarnation, or the base is beyond the latest:
    /// `not applied: the block was rewritten` (§3.3).
    Rewritten,
    /// No live block has the id: `the block is gone` (§3.7). An open or
    /// unconverted card is one.
    Gone,
    /// The incarnation is at [`block::REVISIONS_MAX`]: `replace the block or use a new id`.
    Revisions,
}

/// One key's outcome (§3.4, §3.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    Applied,
    /// The value was already that: nothing to do.
    Unchanged,
    /// A step that is doing, done or failed: `deploy already ran`.
    AlreadyRan,
    /// The item isn't live.
    NotThere,
    /// An added step's id exists or existed (§1.4): `1 added step collided · add it again`.
    Collided,
    /// The list is at its cap.
    Full,
    /// A hunk's keep or drop on a diff that is no longer a proposal.
    NotProposal,
    /// It would take the body past its kind's byte cap.
    TooBig,
    /// Applied, then the agent overwrote it.
    Overwritten,
}

/// One line of the op log (§3.8): where, who, what, on which id, and how it
/// went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Logged {
    pub at: Pos,
    pub author: Author,
    pub did: Did,
    pub id: Option<Ref>,
    pub outcome: Outcome,
    pub pending: bool,
}

/// What a log line or a record is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Did {
    Put,
    Patch,
    Replace,
    Collapse,
    Run,
    Merge,
    Move,
    Remove,
    /// A block call whose result didn't bind: none of its ops applied.
    Call,
    /// A user's edit (§3.3).
    Edit,
    /// A notice (§3.5).
    Notice,
}

/// How an op, an edit or a notice went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    /// A `put` whose `at.after` didn't resolve: placed here, marked (§1.2).
    Here,
    /// It changed nothing (`no change`); a `remove` of an absent id too.
    NoChange,
    /// A `put` on a live id: `exists` (§1.3).
    Exists,
    Skipped(Why),
    /// One path of a patch skipped; the rest still apply (§1.4).
    SkippedPath(Path, PathWhy),
    /// A call that didn't bind (§1.6).
    Unbound,
    /// A notice at or below `effective_version`: `stale v3: v5 is later`.
    Stale {
        version: u32,
        later: u32,
    },
    /// An id named twice in one message's notices: neither applies.
    Twice,
}

/// Why an op (or a merge's member) was skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Why {
    /// No live block has the id.
    NotThere,
    /// An open card (§1.5): the agent closes it first.
    CardOpen,
    /// A resolved card no `replace` has converted: it has no block yet.
    NoBlock,
    /// 32 live blocks (§1.7).
    Full,
    /// A dismissed block can't be moved or join a group (§3.5).
    Dismissed,
    /// A group: `replace` skips it, and it can't be a member.
    Group,
    /// A merge's `into` is a live block that isn't a group.
    NotGroup,
    /// A card can't join a group (§1.5).
    Card,
    /// Already in a group.
    InGroup,
    /// Not in the group's target and slot.
    Elsewhere,
    /// The group has 8 members.
    GroupFull,
    /// A new group with fewer than 2 members.
    TooFew,
    /// A `move`'s `at.after` didn't resolve.
    After,
    /// A card can't go to the rail (§1.5).
    Rail,
    /// [`block::REVISIONS_MAX`] revisions: `replace the block or use a new id`.
    Revisions,
    /// Its group is at [`block::REVISIONS_MAX`] revisions, and leaving would
    /// revise it (two or more members would stay).
    GroupRevisions,
    /// Every path of the patch was skipped.
    Paths,
    /// An edit of another incarnation (§3.3).
    Rewritten,
    /// An edit of a block that is gone.
    Gone,
}

/// Why one path of a patch was skipped (§1.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathWhy {
    /// Not a path of this block's kind.
    NotKind,
    /// The item isn't there.
    NotThere,
    /// `steps.fix was removed in r3`: its id is spent.
    Removed(u32),
    /// The incarnation named 64 item ids: `replace the block`.
    Ids,
    /// The list would pass its cap.
    Full,
    /// It would take the body past its kind's byte cap.
    TooBig,
    /// A diff that draws the session's edits (`from:"edits"`) takes no
    /// hunks: "give hunks or from, not both" (§1.5).
    FromEdits,
}

impl State {
    pub(crate) fn new(generation: Generation) -> Self {
        State {
            generation,
            creations: 0,
            blocks: BTreeMap::new(),
            lists: BTreeMap::new(),
            tombstones: VecDeque::new(),
            records: VecDeque::new(),
            visibility: Visibility::default(),
            runs: VecDeque::new(),
            outcomes: VecDeque::new(),
            log: VecDeque::new(),
            pending: false,
            pinned: None,
        }
    }

    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Incarnations started in this generation so far.
    pub fn creations(&self) -> u64 {
        self.creations
    }

    /// The live block `id` names.
    pub fn block(&self, id: &Ref) -> Option<&Block> {
        self.blocks.get(id)
    }

    /// Every live block, groups and members included, by id.
    pub fn blocks(&self) -> impl Iterator<Item = &Block> {
        self.blocks.values()
    }

    pub fn live(&self) -> usize {
        self.blocks.len()
    }

    /// Each place's list, in place order: transcript anchors in history
    /// order, then the rail's slots.
    pub fn lists(&self) -> impl Iterator<Item = (Place, &[Ref])> {
        self.lists.iter().map(|(p, l)| (*p, l.as_slice()))
    }

    pub fn list(&self, place: Place) -> &[Ref] {
        self.lists.get(&place).map_or(&[], Vec::as_slice)
    }

    pub fn tombstones(&self) -> impl Iterator<Item = &Tombstone> {
        self.tombstones.iter()
    }

    /// The revision records the board still holds, oldest first.
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.records.iter()
    }

    /// `tag`'s records, oldest first.
    pub fn records_of(&self, tag: Tag) -> impl Iterator<Item = &Record> {
        self.records.iter().filter(move |r| r.tag == tag)
    }

    pub fn visibility(&self) -> &Visibility {
        &self.visibility
    }

    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.runs.iter()
    }

    /// The run title of the block call at position `call`: the run that
    /// call sits in.
    pub fn run(&self, call: usize) -> Option<&str> {
        self.runs.iter().find(|r| r.at.event == call).map(|r| &*r.summary)
    }

    pub fn outcomes(&self) -> impl Iterator<Item = &EditOutcome> {
        self.outcomes.iter()
    }

    /// The outcome of the hand-back at `at`, if still kept.
    pub fn outcome(&self, at: Pos) -> Option<&EditOutcome> {
        self.outcomes.iter().find(|o| o.at == at)
    }

    /// The op log, oldest first.
    pub fn log(&self) -> impl Iterator<Item = &Logged> {
        self.log.iter()
    }

    pub(crate) fn logged(&mut self, at: Pos, author: Author, did: Did, id: Option<Ref>, outcome: Outcome) {
        let pending = self.pending;
        if let (Outcome::Skipped(_) | Outcome::SkippedPath(..) | Outcome::Here, Some(id)) = (&outcome, &id)
            && let Some(b) = self.blocks.get_mut(id)
        {
            b.skipped = Some(at);
            b.pending |= pending;
        }
        push_capped(&mut self.log, Logged { at, author, did, id, outcome, pending }, LOG_MAX);
    }

    pub(crate) fn skip(&mut self, at: Pos, did: Did, id: &Ref, why: Why) {
        self.logged(at, Author::Agent, did, Some(id.clone()), Outcome::Skipped(why));
    }

    /// A call that didn't bind: one line, none of its ops.
    pub(crate) fn unbound(&mut self, at: Pos, pending: bool) {
        self.pending = pending;
        self.logged(at, Author::Agent, Did::Call, None, Outcome::Unbound);
        self.pending = false;
    }

    pub(crate) fn record(&mut self, b: &Block, at: Pos, author: Author, did: Did, paths: &[String]) {
        let mut shown = String::new();
        for p in paths.iter().take(RECORD_PATHS_MAX) {
            if !shown.is_empty() {
                shown.push(' ');
            }
            shown.push_str(p);
        }
        let changed = u16::try_from(paths.len()).unwrap_or(u16::MAX);
        // The incarnation's oldest past its 64, then the board's oldest past 1,024.
        if self.records.iter().filter(|r| r.tag == b.tag).count() >= RECORDS_PER_BLOCK
            && let Some(i) = self.records.iter().position(|r| r.tag == b.tag)
        {
            self.records.remove(i);
        }
        push_capped(&mut self.records, Record { tag: b.tag, rev: b.rev, at, author, did, changed, paths: shown.into() }, RECORDS_MAX);
    }

    pub(crate) fn tombstone(&mut self, b: &Block, ended: Pos) {
        push_capped(&mut self.tombstones, Tombstone { id: b.id.clone(), tag: b.tag, rev: b.rev, ended }, TOMBSTONES_MAX);
    }
}
