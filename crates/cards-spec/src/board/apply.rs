//! The agent's ops on the board (§1.3, §1.4): one at a time, in call order,
//! each applied on its own or skipped and marked.

use super::*;
use crate::block::{
    Appended, Collapse, DiffState, FileState, GROUP_MAX, GROUP_MIN, GroupBody, HunkState, ITEM_IDS_MAX, LIVE_MAX, Merge, MetricState, Move,
    Parts, Put, REVISIONS_MAX, Remove, Replace, SetValue, StatusState, StepState, Target, append_spec, order_spec, remove_spec, set_spec,
};
use crate::fold::State as CardState;

/// The call being applied.
pub(crate) struct Cx<'a> {
    /// Its position: a transcript block placed "here" anchors at it.
    pub call: usize,
    pub tool_id: &'a str,
    /// The card fold at the commit: `card:` ops resolve against it (§1.7).
    pub cards: &'a Fold,
    /// A pending call's ops (the overlay).
    pub pending: bool,
}

/// What an op's id names now.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Found {
    Live,
    NotThere,
    CardOpen,
    /// A resolved card without a block.
    NoBlock,
}

impl State {
    /// A bound call's ops, in array order (§1.7).
    pub(crate) fn call(&mut self, cx: &Cx, ops: &[Op]) {
        self.pending = cx.pending;
        for (i, op) in ops.iter().enumerate() {
            let at = Pos { event: cx.call, op: Some(u8::try_from(i).unwrap_or(u8::MAX)) };
            match op {
                Op::Put(p) => self.put(cx, at, i, p),
                Op::Patch(p) => self.patch(cx, at, &p.id, &p.parts),
                Op::Replace(r) => self.replace(cx, at, i, r),
                Op::Collapse(c) => self.collapse(cx, at, c),
                Op::Merge(m) => self.merge(cx, at, i, m),
                Op::Move(m) => self.move_to(cx, at, m),
                Op::Remove(r) => self.remove(cx, at, r),
            }
        }
        self.pending = false;
    }

    fn find(&self, id: &Ref, cards: &Fold) -> Found {
        if self.blocks.contains_key(id) {
            return Found::Live;
        }
        match id {
            Ref::Block(_) => Found::NotThere,
            // Decision (pending Haseeb), D1: an op on a resolved card that no
            // `replace` converted finds no block (`replace` converts it).
            Ref::Card(c) => match cards.bound(c).map(|c| c.state) {
                None | Some(CardState::Plain) => Found::NotThere,
                Some(CardState::Pending | CardState::Open) => Found::CardOpen,
                Some(_) => Found::NoBlock,
            },
        }
    }

    /// Whether `id` names a live block; if not, `did` is skipped, marked.
    fn live_or_skip(&mut self, cx: &Cx, at: Pos, did: Did, id: &Ref) -> bool {
        let why = match self.find(id, cx.cards) {
            Found::Live => return true,
            Found::NotThere => Why::NotThere,
            Found::CardOpen => Why::CardOpen,
            Found::NoBlock => Why::NoBlock,
        };
        self.skip(at, did, id, why);
        false
    }

    /// A new incarnation of `id` (§1.2): the next creation number, its tag.
    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, cx: &Cx, at: Pos, op: usize, id: Ref, kind: Kind, title: Option<String>, body: Body, place: Place) -> Block {
        self.creations += 1;
        let tag = Tag::new(&self.generation, &id, self.creations, cx.tool_id, op);
        let items = items_of(&body);
        Block {
            id,
            place,
            group: None,
            collapsed: false,
            folded_at: None,
            tag,
            creation: self.creations,
            created: at,
            kind,
            title,
            body,
            rev: 1,
            replaced: None,
            items,
            writers: Writers::default(),
            changed_your_edit: None,
            skipped: None,
            pending: self.pending,
        }
    }

    /// Adds `b`, listed at `index` of `place` (or not, if a member), with
    /// its r1 record.
    fn add(&mut self, b: Block, at: Pos, did: Did, index: Option<usize>) {
        if let Some(i) = index {
            let list = self.lists.entry(b.place).or_default();
            list.insert(i.min(list.len()), b.id.clone());
        }
        self.record(&b, at, Author::Agent, did, &[]);
        self.blocks.insert(b.id.clone(), b);
    }

    /// Takes `id` out of its list: where it was.
    fn unlist(&mut self, id: &Ref) -> Option<(Place, usize)> {
        let place = self.blocks.get(id)?.place;
        let list = self.lists.get_mut(&place)?;
        let i = list.iter().position(|r| r == id)?;
        list.remove(i);
        if list.is_empty() {
            self.lists.remove(&place);
        }
        Some((place, i))
    }

    fn list_at(&mut self, place: Place, index: usize, id: Ref) {
        let list = self.lists.entry(place).or_default();
        list.insert(index.min(list.len()), id);
    }

    fn end_of(&self, place: Place) -> usize {
        self.lists.get(&place).map_or(0, Vec::len)
    }

    /// `at.after` (§1.2): right after `x` in its list, if `x` is a live block
    /// in a place `want` takes. A group member is in no list, so it never
    /// resolves. Resolved once.
    fn after(&self, x: &Ref, want: impl Fn(Place) -> bool) -> Option<(Place, usize)> {
        let b = self.blocks.get(x)?;
        if !want(b.place) {
            return None;
        }
        let i = self.lists.get(&b.place)?.iter().position(|r| r == x)?;
        Some((b.place, i + 1))
    }

    fn put(&mut self, cx: &Cx, at: Pos, op: usize, p: &Put) {
        let id = Ref::Block(p.id.clone());
        if self.blocks.contains_key(&id) {
            // Create only: a retried put never undoes an edit (§1.3).
            self.logged(at, Author::Agent, Did::Put, Some(id), Outcome::Exists);
            return;
        }
        if self.blocks.len() >= LIVE_MAX {
            self.skip(at, Did::Put, &id, Why::Full);
            return;
        }
        let home = match p.target {
            Target::Transcript => Place::Transcript(cx.call),
            Target::Rail => Place::Rail(p.slot.unwrap_or(Slot::Upper)),
        };
        let found = p.at.as_ref().map(|a| self.after(&a.after, |pl| same_list_kind(pl, home)));
        let (place, index, outcome) = match found {
            Some(Some((pl, i))) => (pl, i, Outcome::Applied),
            // `at` that doesn't resolve sends a put here, marked (§1.2).
            Some(None) => (home, self.end_of(home), Outcome::Here),
            None => (home, self.end_of(home), Outcome::Applied),
        };
        let b = self.start(cx, at, op, id.clone(), p.kind, p.title.clone(), p.body.clone(), place);
        self.add(b, at, Did::Put, Some(index));
        self.logged(at, Author::Agent, Did::Put, Some(id), outcome);
    }

    fn replace(&mut self, cx: &Cx, at: Pos, op: usize, r: &Replace) {
        let id = &r.id;
        match self.find(id, cx.cards) {
            Found::Live if self.blocks[id].kind == Kind::Group => self.skip(at, Did::Replace, id, Why::Group),
            Found::Live => {
                let old = self.blocks.remove(id).expect("live");
                self.tombstone(&old, at);
                let title = replaced_title(old.title, r.title.clone());
                let mut b = self.start(cx, at, op, id.clone(), r.kind, title, r.body.clone(), old.place);
                // The slot's: place, group, fold (§1.2).
                b.group = old.group;
                b.collapsed = old.collapsed;
                b.folded_at = old.folded_at;
                b.replaced = Some(at);
                self.add(b, at, Did::Replace, None);
                self.logged(at, Author::Agent, Did::Replace, Some(id.clone()), Outcome::Applied);
            }
            // A resolved card's first block incarnation (§1.5), where the
            // card draws.
            Found::NoBlock if self.blocks.len() >= LIVE_MAX => self.skip(at, Did::Replace, id, Why::Full),
            Found::NoBlock => {
                let Ref::Card(c) = id else { unreachable!("only a card has no block") };
                let place = Place::Transcript(cx.cards.bound(c).expect("resolved").at);
                let b = self.start(cx, at, op, id.clone(), r.kind, r.title.clone(), r.body.clone(), place);
                let index = self.end_of(place);
                self.add(b, at, Did::Replace, Some(index));
                self.logged(at, Author::Agent, Did::Replace, Some(id.clone()), Outcome::Applied);
            }
            Found::NotThere => self.skip(at, Did::Replace, id, Why::NotThere),
            Found::CardOpen => self.skip(at, Did::Replace, id, Why::CardOpen),
        }
    }

    fn patch(&mut self, cx: &Cx, at: Pos, id: &Ref, parts: &Parts) {
        if !self.live_or_skip(cx, at, Did::Patch, id) {
            return;
        }
        if self.blocks[id].rev >= REVISIONS_MAX {
            self.skip(at, Did::Patch, id, Why::Revisions);
            return;
        }
        let mut b = self.blocks.remove(id).expect("live");
        let next = b.rev + 1;
        let kind = b.kind;
        // Each changed path, and whether it overwrites (an append doesn't).
        let mut changed: Vec<(&Path, bool)> = Vec::new();
        let mut skipped: Vec<(&Path, PathWhy)> = Vec::new();
        let mut note = |p, writes: bool, r: Result<bool, PathWhy>| match r {
            Ok(true) => changed.push((p, writes)),
            Ok(false) => {}
            Err(w) => skipped.push((p, w)),
        };
        // The four phases (§1.4), each path on its own.
        for p in &parts.remove {
            let r = if remove_spec(kind, Author::Agent, p) { path(&mut b, |b| remove_item(b, p, next)) } else { Err(PathWhy::NotKind) };
            note(p, true, r);
        }
        for (p, a) in &parts.append {
            let r = if append_spec(kind, Author::Agent, p).is_some() { path(&mut b, |b| append(b, p, a)) } else { Err(PathWhy::NotKind) };
            note(p, false, r);
        }
        for (p, v) in &parts.set {
            let r = if fits_set(kind, Author::Agent, p, v) { path(&mut b, |b| set_field(b, p, v)) } else { Err(PathWhy::NotKind) };
            note(p, true, r);
        }
        for (p, ids) in &parts.order {
            let r = if order_spec(kind, Author::Agent, p) { path(&mut b, |b| order_list(b, p, ids)) } else { Err(PathWhy::NotKind) };
            note(p, true, r);
        }
        for (p, _) in changed.iter().filter(|(_, writes)| *writes) {
            self.agent_wrote(&mut b, p);
        }
        b.pending |= self.pending;
        let outcome = if !changed.is_empty() {
            b.rev = next;
            let paths: Vec<String> = changed.iter().map(|(p, _)| p.to_string()).collect();
            self.record(&b, at, Author::Agent, Did::Patch, &paths);
            Outcome::Applied
        } else if skipped.is_empty() {
            Outcome::NoChange
        } else {
            Outcome::Skipped(Why::Paths)
        };
        self.blocks.insert(id.clone(), b);
        self.logged(at, Author::Agent, Did::Patch, Some(id.clone()), outcome);
        for (p, w) in skipped {
            self.logged(at, Author::Agent, Did::Patch, Some(id.clone()), Outcome::SkippedPath(p.clone(), w));
        }
    }

    /// The agent wrote `p` on `b`: it is the last writer now. Over a path the
    /// user wrote last, that edit's key is overwritten, and the header says
    /// so (§3.4 step 6). Decision (pending Haseeb), D6: a `set` of the path,
    /// an `order` of the list, or a `remove` of the item overwrites.
    fn agent_wrote(&mut self, b: &mut Block, p: &Path) {
        let users: Vec<(Path, Writer)> = if p.item().is_some() && p.field().is_none() {
            // A removed item: every write inside it.
            b.writers.remove_item(p.as_str())
        } else {
            b.writers.remove(p).map(|w| (p.clone(), w)).into_iter().collect()
        };
        for (path, w) in users {
            b.changed_your_edit = Some(path);
            if let Some(o) = self.outcomes.iter_mut().find(|o| o.at == w.at)
                && let Some(c) = o.codes.get_mut(usize::from(w.key))
                && *c == Code::Applied
            {
                *c = Code::Overwritten;
            }
        }
    }

    fn collapse(&mut self, cx: &Cx, at: Pos, c: &Collapse) {
        if c.run {
            // The tool run this call sits in (§1.3): the desk's grouping,
            // titled by the agent. Keyed by the call's position, never a
            // copy of its tool id (§1.8: an outer id has no bound).
            let summary: Box<str> = c.summary.clone().unwrap_or_default().into();
            match self.runs.iter_mut().find(|r| r.at.event == cx.call) {
                Some(r) => {
                    r.summary = summary;
                    r.at = at;
                }
                None => push_capped(&mut self.runs, Run { summary, at }, RUNS_MAX),
            }
            self.logged(at, Author::Agent, Did::Run, None, Outcome::Applied);
            return;
        }
        let fold = !c.open.unwrap_or(false);
        for id in c.id.iter().chain(&c.ids) {
            if !self.live_or_skip(cx, at, Did::Collapse, id) {
                continue;
            }
            let pending = self.pending;
            let b = self.blocks.get_mut(id).expect("live");
            let changed = b.collapsed != fold;
            b.collapsed = fold;
            b.folded_at = Some(at);
            b.pending |= pending;
            let outcome = if changed { Outcome::Applied } else { Outcome::NoChange };
            self.logged(at, Author::Agent, Did::Collapse, Some(id.clone()), outcome);
        }
    }

    /// Why `id` can't join a group at `place` (§1.3), if it can't.
    fn member(&self, id: &Ref, place: Option<Place>) -> Option<Why> {
        let Ref::Block(bid) = id else { return Some(Why::Card) };
        let Some(b) = self.blocks.get(id) else { return Some(Why::NotThere) };
        if b.kind == Kind::Group {
            Some(Why::Group)
        } else if self.visibility.dismissed(bid) {
            Some(Why::Dismissed)
        } else if b.group.is_some() {
            Some(Why::InGroup)
        } else if place.is_some_and(|p| !same_list_kind(p, b.place)) {
            Some(Why::Elsewhere)
        } else {
            None
        }
    }

    fn merge(&mut self, cx: &Cx, at: Pos, op: usize, m: &Merge) {
        let into = Ref::Block(m.into.clone());
        let group = self.blocks.get(&into).map(|g| (g.kind == Kind::Group, g.place, g.members().len(), g.rev));
        if let Some((false, ..)) = group {
            self.skip(at, Did::Merge, &into, Why::NotGroup);
            return;
        }
        if let Some((_, _, _, rev)) = group
            && rev >= REVISIONS_MAX
        {
            self.skip(at, Did::Merge, &into, Why::Revisions);
            return;
        }
        // A new group sits at its first member's place (Reading: the first
        // that qualifies), and the rest must share its target and slot.
        let mut place = group.map(|g| g.1);
        let room = GROUP_MAX - group.map_or(0, |g| g.2);
        let mut joined: Vec<BlockId> = Vec::new();
        for id in &m.ids {
            match self.member(id, place) {
                Some(why) => self.skip(at, Did::Merge, id, why),
                None if joined.len() == room => self.skip(at, Did::Merge, id, Why::GroupFull),
                None => {
                    place = place.or(Some(self.blocks[id].place));
                    joined.push(id.block().expect("a member is a block").clone());
                }
            }
        }
        match group {
            Some(_) if joined.is_empty() => self.logged(at, Author::Agent, Did::Merge, Some(into), Outcome::NoChange),
            Some((_, gplace, ..)) => {
                for j in &joined {
                    self.join(&Ref::Block(j.clone()), &m.into, gplace);
                }
                let mut g = self.blocks.remove(&into).expect("live");
                if let Body::Group(body) = &mut g.body {
                    body.members.extend(joined);
                }
                g.rev += 1;
                g.pending |= self.pending;
                self.record(&g, at, Author::Agent, Did::Merge, &["members".to_string()]);
                self.blocks.insert(into.clone(), g);
                self.logged(at, Author::Agent, Did::Merge, Some(into), Outcome::Applied);
            }
            None if joined.len() < GROUP_MIN => self.skip(at, Did::Merge, &into, Why::TooFew),
            None if self.blocks.len() >= LIVE_MAX => self.skip(at, Did::Merge, &into, Why::Full),
            None => {
                // The group takes its first member's entry; the members leave theirs.
                let first = Ref::Block(joined[0].clone());
                let (gplace, index) = self.unlist(&first).expect("a member is listed");
                self.list_at(gplace, index, into.clone());
                for j in &joined {
                    self.join(&Ref::Block(j.clone()), &m.into, gplace);
                }
                let body = Body::Group(GroupBody { members: joined });
                let mut g = self.start(cx, at, op, into.clone(), Kind::Group, m.title.clone(), body, gplace);
                // Groups are created folded (§1.3).
                g.collapsed = true;
                self.add(g, at, Did::Merge, None);
                self.logged(at, Author::Agent, Did::Merge, Some(into), Outcome::Applied);
            }
        }
    }

    /// `id` leaves its list and joins group `g` at `place`.
    fn join(&mut self, id: &Ref, g: &BlockId, place: Place) {
        self.unlist(id);
        let pending = self.pending;
        let b = self.blocks.get_mut(id).expect("live");
        b.group = Some(g.clone());
        b.place = place;
        b.pending |= pending;
    }

    /// `id` leaves its group (by `did` at `at`). A group left with one
    /// member dissolves: the member takes its place, and the group's
    /// incarnation ends (§1.3).
    fn leave(&mut self, id: &Ref, at: Pos, did: Did) {
        let Some(g) = self.blocks.get_mut(id).and_then(|b| b.group.take()) else { return };
        let gid = Ref::Block(g);
        let mut group = self.blocks.remove(&gid).expect("a member's group is live");
        if let Body::Group(body) = &mut group.body {
            body.members.retain(|m| id.block() != Some(m));
        }
        group.pending |= self.pending;
        match group.members() {
            [last] => {
                let last = Ref::Block(last.clone());
                self.blocks.insert(gid.clone(), group);
                let (place, index) = self.unlist(&gid).expect("a group is listed");
                let group = self.blocks.remove(&gid).expect("live");
                self.list_at(place, index, last.clone());
                let pending = self.pending;
                let b = self.blocks.get_mut(&last).expect("a member is live");
                b.group = None;
                b.place = place;
                b.pending |= pending;
                self.tombstone(&group, at);
            }
            _ => {
                if group.rev < REVISIONS_MAX {
                    group.rev += 1;
                    self.record(&group, at, Author::Agent, did, &["members".to_string()]);
                }
                self.blocks.insert(gid, group);
            }
        }
    }

    fn move_to(&mut self, cx: &Cx, at: Pos, m: &Move) {
        let id = &m.id;
        if !self.live_or_skip(cx, at, Did::Move, id) {
            return;
        }
        if let Ref::Block(b) = id
            && self.visibility.dismissed(b)
        {
            self.skip(at, Did::Move, id, Why::Dismissed);
            return;
        }
        let dest: Option<Place> = match (m.target, m.slot) {
            (_, Some(s)) => Some(Place::Rail(s)),
            (Some(Target::Rail), None) => Some(Place::Rail(Slot::Upper)),
            (Some(Target::Transcript), None) => Some(Place::Transcript(cx.call)),
            (None, None) if m.at.is_none() => Some(Place::Transcript(cx.call)),
            // Decision (pending Haseeb), D5: `at` alone places in its block's list.
            (None, None) => None,
        };
        let after = match &m.at {
            Some(a) => match self.after(&a.after, |p| dest.is_none_or(|d| same_list_kind(p, d))) {
                Some((place, _)) => Some((place, a.after.clone())),
                None => {
                    self.skip(at, Did::Move, id, Why::After);
                    return;
                }
            },
            None => None,
        };
        let place = after.as_ref().map_or_else(|| dest.expect("a place"), |(p, _)| *p);
        // A card stays in the transcript (§1.5), however `at` would place it.
        if matches!(id, Ref::Card(_)) && matches!(place, Place::Rail(_)) {
            self.skip(at, Did::Move, id, Why::Rail);
            return;
        }
        let b = &self.blocks[id];
        let was = (b.place, b.group.clone(), self.lists.get(&b.place).and_then(|l| l.iter().position(|r| r == id)));
        // Leave the list (or the group's members), land, then let a group
        // left with one member dissolve.
        let member = was.1.is_some();
        if member {
            self.detach(id);
        } else {
            self.unlist(id);
        }
        let index = match &after {
            Some((p, x)) => self.lists.get(p).and_then(|l| l.iter().position(|r| r == x)).map_or(self.end_of(*p), |i| i + 1),
            None => self.end_of(place),
        };
        self.list_at(place, index, id.clone());
        let pending = self.pending;
        let b = self.blocks.get_mut(id).expect("live");
        b.place = place;
        b.pending |= pending;
        // A group moves with its members (§1.3).
        let members = b.members().to_vec();
        for mid in members {
            if let Some(mb) = self.blocks.get_mut(&Ref::Block(mid)) {
                mb.place = place;
                mb.pending |= pending;
            }
        }
        if member {
            self.dissolve(id, was.1.as_ref().expect("member"), at, Did::Move);
        }
        let outcome = if !member && was.0 == place && was.2 == Some(index) { Outcome::NoChange } else { Outcome::Applied };
        self.logged(at, Author::Agent, Did::Move, Some(id.clone()), outcome);
    }

    /// `id` leaves its group's members, before it lands somewhere else.
    fn detach(&mut self, id: &Ref) {
        let Some(g) = self.blocks.get(id).and_then(|b| b.group.clone()) else { return };
        if let Some(Body::Group(body)) = self.blocks.get_mut(&Ref::Block(g)).map(|g| &mut g.body) {
            body.members.retain(|m| id.block() != Some(m));
        }
    }

    /// After `id` left group `g`'s members: its revision, or its end with
    /// one member left.
    fn dissolve(&mut self, id: &Ref, g: &BlockId, at: Pos, did: Did) {
        // Put the membership back, then leave through the one path.
        if let Some(Body::Group(body)) = self.blocks.get_mut(&Ref::Block(g.clone())).map(|g| &mut g.body) {
            body.members.push(id.block().expect("a member is a block").clone());
        }
        if let Some(b) = self.blocks.get_mut(id) {
            b.group = Some(g.clone());
        }
        let place = self.blocks[id].place;
        self.leave(id, at, did);
        if let Some(b) = self.blocks.get_mut(id) {
            b.place = place;
        }
    }

    fn remove(&mut self, cx: &Cx, at: Pos, r: &Remove) {
        for id in r.id.iter().chain(&r.ids) {
            match self.find(id, cx.cards) {
                Found::Live => {
                    self.end(id, at, Did::Remove);
                    self.logged(at, Author::Agent, Did::Remove, Some(id.clone()), Outcome::Applied);
                }
                // On an absent id, a no-op (§1.3).
                Found::NotThere => self.logged(at, Author::Agent, Did::Remove, Some(id.clone()), Outcome::NoChange),
                Found::CardOpen => self.skip(at, Did::Remove, id, Why::CardOpen),
                Found::NoBlock => self.skip(at, Did::Remove, id, Why::NoBlock),
            }
        }
    }

    /// Ends `id`'s incarnation (§1.3): a group releases its members at its
    /// place, in member order; a member leaves its group.
    fn end(&mut self, id: &Ref, at: Pos, did: Did) {
        if self.blocks[id].group.is_some() {
            self.leave(id, at, did);
        } else if let Some((place, index)) = self.unlist(id) {
            let members: Vec<BlockId> = self.blocks[id].members().to_vec();
            let pending = self.pending;
            for (k, m) in members.into_iter().enumerate() {
                let mid = Ref::Block(m);
                self.list_at(place, index + k, mid.clone());
                if let Some(mb) = self.blocks.get_mut(&mid) {
                    mb.group = None;
                    mb.place = place;
                    mb.pending |= pending;
                }
            }
        }
        let b = self.blocks.remove(id).expect("live");
        self.tombstone(&b, at);
    }
}

/// Decision (pending Haseeb), D2: a `replace` without a title keeps the
/// block's ("a new title if given", §1.3).
fn replaced_title(old: Option<String>, new: Option<String>) -> Option<String> {
    new.or(old)
}

/// Whether two places are one target, and in the rail one slot.
fn same_list_kind(a: Place, b: Place) -> bool {
    match (a, b) {
        (Place::Transcript(_), Place::Transcript(_)) => true,
        (Place::Rail(x), Place::Rail(y)) => x == y,
        _ => false,
    }
}

/// A body's items, all live.
fn items_of(body: &Body) -> Items {
    let mut items = Items::default();
    let ids: Vec<&ItemId> = match body {
        Body::Plan(p) => p.steps.iter().map(|s| &s.id).collect(),
        Body::Diff(d) => d.hunks.iter().map(|h| &h.id).collect(),
        _ => Vec::new(),
    };
    for id in ids {
        items.insert(id.clone(), None);
    }
    items
}

/// Whether the agent's `set` of `p` to `v` fits a block of `kind`: its spec
/// takes the value (B1 normalized it under some kind's).
pub(crate) fn fits_set(kind: Kind, author: Author, p: &Path, v: &SetValue) -> bool {
    let Some(spec) = set_spec(kind, author, p) else { return false };
    serde_json::to_value(v).is_ok_and(|v| spec.check(&v, p.as_str(), p).is_ok())
}

/// One path, all or nothing (§1.4): `f` applied, then the stored body
/// checked ([`check_stored`]); on any failure the block is as it was.
pub(crate) fn path(b: &mut Block, f: impl FnOnce(&mut Block) -> Result<bool, PathWhy>) -> Result<bool, PathWhy> {
    let (body, title, items) = (b.body.clone(), b.title.clone(), b.items.clone());
    let r = f(b).and_then(|changed| check_stored(&b.body).map(|()| changed));
    if r.is_err() {
        b.body = body;
        b.title = title;
        b.items = items;
    }
    r
}

/// What a stored body keeps (§1.5), whoever wrote it last: the kind's caps
/// a patch can pass (list counts, a diff's lines, the byte cap) and the
/// rules across its fields that B1 checks on a whole body: a status's text
/// is one line, and a diff draws hunks or the session's edits, not both.
/// The fold checks it after every path ([`path`]); it doesn't re-run an
/// agent's validation on the body, since the user's own decisions (a step's
/// stop, a hunk kept) are stored state no agent writes.
///
/// Decision (pending Haseeb), D3: checked per path, so the path that would
/// pass a cap is the one skipped. Decision D7: the maxima only, so a patch
/// may remove a plan's last step (1–20 is a put's).
pub fn check_stored(body: &Body) -> Result<(), PathWhy> {
    use crate::block::{DIFF_LINES_MAX, HUNKS_MAX, PLAN_STEPS_MAX};
    match body {
        Body::Plan(p) if p.steps.len() > PLAN_STEPS_MAX => return Err(PathWhy::Full),
        Body::Diff(d) if d.from.is_some() && !d.hunks.is_empty() => return Err(PathWhy::FromEdits),
        Body::Diff(d) if d.hunks.len() > HUNKS_MAX || d.hunks.iter().map(|h| h.lines.len()).sum::<usize>() > DIFF_LINES_MAX => {
            return Err(PathWhy::Full);
        }
        Body::Status(s) if s.text.contains('\n') => return Err(PathWhy::NotKind),
        _ => {}
    }
    if body.bytes() > body.kind().max_bytes() { Err(PathWhy::TooBig) } else { Ok(()) }
}

fn put_if<T: PartialEq>(slot: &mut T, v: T) -> bool {
    let changed = *slot != v;
    *slot = v;
    changed
}

fn text(v: &SetValue) -> Result<&str, PathWhy> {
    match v {
        SetValue::Text(s) => Ok(s),
        _ => Err(PathWhy::NotKind),
    }
}

fn optional(v: &SetValue) -> Result<Option<String>, PathWhy> {
    let s = text(v)?;
    Ok((!s.is_empty()).then(|| s.to_string()))
}

fn word<T>(v: &SetValue, parse: fn(&str) -> Option<T>) -> Result<T, PathWhy> {
    parse(text(v)?).ok_or(PathWhy::NotKind)
}

/// An item that isn't live: removed at a revision, or never there.
fn missing(items: &Items, id: &str) -> PathWhy {
    match ItemId::parse(id).and_then(|i| items.get(&i).flatten()) {
        Some(r) => PathWhy::Removed(r),
        None => PathWhy::NotThere,
    }
}

/// `set` of one path: `Ok(true)` if it changed the block.
pub(crate) fn set_field(b: &mut Block, p: &Path, v: &SetValue) -> Result<bool, PathWhy> {
    if p.as_str() == "title" {
        return Ok(put_if(&mut b.title, optional(v)?));
    }
    let items = &b.items;
    Ok(match (&mut b.body, p.head(), p.item(), p.field()) {
        (Body::Plan(pl), "steps", Some(x), Some(f)) => {
            let s = pl.steps.iter_mut().find(|s| s.id.as_str() == x).ok_or_else(|| missing(items, x))?;
            match (f, v) {
                ("label", v) => put_if(&mut s.label, text(v)?.to_string()),
                ("state", v) => put_if(&mut s.state, word(v, StepState::parse)?),
                ("note", v) => put_if(&mut s.note, optional(v)?),
                ("stop", SetValue::Flag(f)) => put_if(&mut s.stop, *f),
                _ => return Err(PathWhy::NotKind),
            }
        }
        (Body::Status(s), f, None, None) => match (f, v) {
            // A patch's `text` keeps its lines (B1 can't tell a note's from a
            // status's): a status's is folded to one, as its put's is.
            ("text", v) => put_if(&mut s.text, crate::sanitize::ui_sanitize(text(v)?, crate::sanitize::Lines::Single)),
            ("detail", v) => put_if(&mut s.detail, optional(v)?),
            ("progress", SetValue::Progress(n)) => put_if(&mut s.progress, Some(*n)),
            ("state", v) => put_if(&mut s.state, word(v, StatusState::parse)?),
            _ => return Err(PathWhy::NotKind),
        },
        (Body::Diff(d), "file", None, None) => put_if(&mut d.file, text(v)?.to_string()),
        (Body::Diff(d), "state", None, None) => put_if(&mut d.state, word(v, DiffState::parse)?),
        (Body::Diff(d), "hunks", Some(x), Some(f)) => {
            let h = d.hunks.iter_mut().find(|h| h.id.as_str() == x).ok_or_else(|| missing(items, x))?;
            match f {
                "at" => put_if(&mut h.at, optional(v)?),
                "state" => put_if(&mut h.state, word(v, HunkState::parse)?),
                _ => return Err(PathWhy::NotKind),
            }
        }
        (Body::Note(n), "text", None, None) => put_if(&mut n.text, text(v)?.to_string()),
        (Body::Metric(m), f, None, None) => match f {
            "label" => put_if(&mut m.label, text(v)?.to_string()),
            "value" => put_if(&mut m.value, text(v)?.to_string()),
            "was" => put_if(&mut m.was, optional(v)?),
            "unit" => put_if(&mut m.unit, optional(v)?),
            "state" => put_if(&mut m.state, word(v, MetricState::parse)?),
            _ => return Err(PathWhy::NotKind),
        },
        (Body::File(fl), f, None, None) => match (f, v) {
            ("path", v) => put_if(&mut fl.path, text(v)?.to_string()),
            ("line", SetValue::Whole(n)) => put_if(&mut fl.line, Some(*n)),
            ("note", v) => put_if(&mut fl.note, optional(v)?),
            ("state", v) => put_if(&mut fl.state, word(v, FileState::parse)?),
            _ => return Err(PathWhy::NotKind),
        },
        _ => return Err(PathWhy::NotKind),
    })
}

/// The new items of an append: one already live is a no-op (appending is
/// idempotent by id, §1.6); a spent id or the 64th id skips the path.
/// Reading: the whole path, as §1.4 makes each path all or nothing.
fn fresh<'a, T>(items: &Items, new: &'a [T], id: impl Fn(&T) -> &ItemId) -> Result<Vec<&'a T>, PathWhy> {
    let mut out = Vec::new();
    for t in new {
        match items.get(id(t)) {
            Some(None) => {}
            Some(Some(rev)) => return Err(PathWhy::Removed(rev)),
            None => out.push(t),
        }
    }
    if items.len() + out.len() > ITEM_IDS_MAX {
        return Err(PathWhy::Ids);
    }
    Ok(out)
}

/// The agent's `append` of one list.
fn append(b: &mut Block, p: &Path, a: &Appended) -> Result<bool, PathWhy> {
    let items = &mut b.items;
    match (&mut b.body, a, p.item()) {
        (Body::Plan(pl), Appended::Steps(new), None) => {
            let new = fresh(items, new, |s| &s.id)?;
            for s in &new {
                items.insert(s.id.clone(), None);
            }
            pl.steps.extend(new.iter().map(|s| (*s).clone()));
            Ok(!new.is_empty())
        }
        (Body::Diff(d), Appended::Hunks(new), None) => {
            let new = fresh(items, new, |h| &h.id)?;
            for h in &new {
                items.insert(h.id.clone(), None);
            }
            d.hunks.extend(new.iter().map(|h| (*h).clone()));
            Ok(!new.is_empty())
        }
        (Body::Diff(d), Appended::Lines(lines), Some(x)) => {
            let h = d.hunks.iter_mut().find(|h| h.id.as_str() == x).ok_or_else(|| missing(items, x))?;
            h.lines.extend(lines.iter().cloned());
            Ok(true)
        }
        (Body::Metric(m), Appended::Series(new), None) => {
            m.series.extend(new.iter().copied());
            // An append rolls off the oldest (§1.5).
            let over = m.series.len().saturating_sub(crate::block::SERIES_MAX);
            m.series.drain(..over);
            Ok(true)
        }
        _ => Err(PathWhy::NotKind),
    }
}

/// The agent's `remove` of one item: its id is spent at revision `next`.
fn remove_item(b: &mut Block, p: &Path, next: u32) -> Result<bool, PathWhy> {
    let (Some(x), None) = (p.item(), p.field()) else { return Err(PathWhy::NotKind) };
    let at = match &mut b.body {
        Body::Plan(pl) if p.head() == "steps" => pl.steps.iter().position(|s| s.id.as_str() == x).map(|i| pl.steps.remove(i).id),
        Body::Diff(d) if p.head() == "hunks" => d.hunks.iter().position(|h| h.id.as_str() == x).map(|i| d.hunks.remove(i).id),
        _ => return Err(PathWhy::NotKind),
    };
    let id = at.ok_or_else(|| missing(&b.items, x))?;
    b.items.insert(id, Some(next));
    Ok(true)
}

/// `order` of one list (§1.4): the named live ids in that order, then the
/// rest in their relative order. Named ids that aren't live are ignored.
pub(crate) fn order_list(b: &mut Block, p: &Path, ids: &[ItemId]) -> Result<bool, PathWhy> {
    fn reorder<T>(list: &mut Vec<T>, ids: &[ItemId], id: impl Fn(&T) -> &ItemId) -> bool {
        let before: Vec<ItemId> = list.iter().map(&id).cloned().collect();
        let mut named = Vec::with_capacity(list.len());
        for x in ids {
            if let Some(i) = list.iter().position(|t| id(t) == x) {
                named.push(list.remove(i));
            }
        }
        named.append(list);
        *list = named;
        list.iter().map(id).ne(before.iter())
    }
    match (&mut b.body, p.as_str()) {
        (Body::Plan(pl), "steps") => Ok(reorder(&mut pl.steps, ids, |s| &s.id)),
        (Body::Diff(d), "hunks") => Ok(reorder(&mut d.hunks, ids, |h| &h.id)),
        _ => Err(PathWhy::NotKind),
    }
}
