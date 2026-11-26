//! What a genuine user message does (§1.7 step 3): its edit (§3.3, §3.4),
//! then its notices (§3.5).

use std::collections::BTreeSet;

use super::apply::{order_list, path, set_field};
use super::*;
use crate::block::message::Visibility as Seen;
use crate::block::{Appended, ITEM_IDS_MAX, PLAN_STEPS_MAX, REVISIONS_MAX, SetValue, Step, StepState};

/// One intent of an edit (§3.7's coverage keys), in the order the fold
/// applies them: each appended item (`steps.u-k3x7qa`), each `set` path,
/// each ordered list (`order steps`). An outcome keeps one [`Code`] per key,
/// in this order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Item(Path),
    Set(Path),
    Order(Path),
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Key::Item(p) | Key::Set(p) => write!(f, "{p}"),
            Key::Order(p) => write!(f, "order {p}"),
        }
    }
}

/// An edit's keys, in the fold's order (see [`Key`]).
pub fn keys(e: &Edit) -> Vec<Key> {
    let mut out = Vec::new();
    for (p, a) in &e.parts.append {
        if let Appended::Steps(steps) = a {
            out.extend(steps.iter().filter_map(|s| Path::parse(&format!("{}.{}", p.head(), s.id.as_str())).map(Key::Item)));
        }
    }
    out.extend(e.parts.set.keys().cloned().map(Key::Set));
    out.extend(e.parts.order.keys().cloned().map(Key::Order));
    out
}

impl State {
    /// The desk's pre-check at hand-back (§3.2): what `e` would do on this
    /// board now, without doing it. `None` if it isn't an edit of this board
    /// at all (paths no user writes on the block's kind: ordinary text).
    pub fn precheck(&self, e: &Edit) -> Option<EditOutcome> {
        let mut scratch = State::new(self.generation);
        if let Some(b) = self.blocks.get(&e.id) {
            scratch.blocks.insert(e.id.clone(), b.clone());
        }
        scratch.edit(Pos::event(usize::MAX), e);
        scratch.outcomes.pop_back()
    }

    /// A user's edit, at its place in history (§3.3's admission, §3.4's
    /// guards).
    pub(crate) fn edit(&mut self, at: Pos, e: &Edit) {
        let id = e.id.clone();
        let whole = match self.blocks.get(&id) {
            None => Some(EditResult::Gone),
            Some(b) if b.tag != e.of || e.base > b.rev => Some(EditResult::Rewritten),
            // Paths no user writes on this kind: ordinary text (§3.3).
            Some(b) if e.admit(b.kind).is_err() => return,
            Some(b) if b.rev >= REVISIONS_MAX => Some(EditResult::Revisions),
            Some(_) => None,
        };
        if let Some(result) = whole {
            let why = match result {
                EditResult::Gone => Why::Gone,
                EditResult::Rewritten => Why::Rewritten,
                _ => Why::Revisions,
            };
            self.keep_outcome(EditOutcome { at, id: id.clone(), result, codes: Vec::new() });
            self.logged(at, Author::User, Did::Edit, Some(id), Outcome::Skipped(why));
            return;
        }
        let mut b = self.blocks.remove(&id).expect("live");
        let keys = keys(e);
        let mut codes: Vec<Code> = Vec::with_capacity(keys.len());
        // A collided id skips every other path that names it (§1.4).
        let mut collided: BTreeSet<&ItemId> = BTreeSet::new();
        for a in e.parts.append.values() {
            if let Appended::Steps(steps) = a {
                for s in steps {
                    let code = add_step(&mut b, s);
                    if code == Code::Collided {
                        collided.insert(&s.id);
                    }
                    codes.push(code);
                }
            }
        }
        for (p, v) in &e.parts.set {
            codes.push(guarded_set(&mut b, p, v));
        }
        for (p, ids) in &e.parts.order {
            let code =
                if ids.iter().any(|i| collided.contains(i)) { Code::Collided } else { code_of(path(&mut b, |b| order_list(b, p, ids))) };
            codes.push(code);
        }
        let mut changed: Vec<String> = Vec::new();
        for (k, (key, code)) in keys.iter().zip(&codes).enumerate() {
            if *code != Code::Applied {
                continue;
            }
            let p = match key {
                Key::Item(p) | Key::Set(p) | Key::Order(p) => p.clone(),
            };
            changed.push(key.to_string());
            b.writers.insert(p, Writer { author: Author::User, at, key: u16::try_from(k).unwrap_or(u16::MAX) });
        }
        let outcome = if changed.is_empty() {
            Outcome::NoChange
        } else {
            // One revision, authored by the user (§1.4).
            b.rev += 1;
            self.record(&b, at, Author::User, Did::Edit, &changed);
            Outcome::Applied
        };
        self.blocks.insert(id.clone(), b);
        self.keep_outcome(EditOutcome { at, id: id.clone(), result: EditResult::Applied, codes });
        self.logged(at, Author::User, Did::Edit, Some(id), outcome);
    }

    fn keep_outcome(&mut self, o: EditOutcome) {
        self.outcomes.push_back(o);
        while self.outcomes.len() > OUTCOMES_MAX {
            self.outcomes.pop_front();
        }
    }

    /// A message's notices (§3.5), after its edit.
    pub(crate) fn notices(&mut self, at: Pos, t: &Trailer) {
        let twice = t.named_twice();
        for line in &t.lines {
            for n in &line.notices {
                let v = &mut self.visibility;
                v.highest = v.highest.max(n.version);
                let id = Some(Ref::Block(n.id.clone()));
                if twice.contains(&n.id) {
                    self.logged(at, Author::User, Did::Notice, id, Outcome::Twice);
                    continue;
                }
                let effective = v.effective_version(&n.id);
                if n.version <= effective {
                    // Of two equal versions, the first in history wins.
                    self.logged(at, Author::User, Did::Notice, id, Outcome::Stale { version: n.version, later: effective });
                    continue;
                }
                // Recorded whether or not a block is live under the id.
                v.records.insert(n.id.clone(), Vis { dismissed: line.visibility == Seen::Dismissed, version: n.version });
                self.evict();
                self.logged(at, Author::User, Did::Notice, id, Outcome::Applied);
            }
        }
    }

    /// Past [`VISIBILITY_MAX`] records (§1.8): shown records go first, then
    /// dismissals of ids with no live block, the oldest version first. A
    /// live block's dismissal never goes. An evicted version raises the
    /// floor.
    fn evict(&mut self) {
        while self.visibility.records.len() > VISIBILITY_MAX {
            let live = |id: &BlockId| self.blocks.contains_key(&Ref::Block(id.clone()));
            let records = &self.visibility.records;
            let oldest = |dismissed: bool| {
                records.iter().filter(|(id, r)| r.dismissed == dismissed && !(dismissed && live(id))).min_by_key(|(id, r)| (r.version, *id))
            };
            let Some((id, r)) = oldest(false).or_else(|| oldest(true)) else { break };
            let (id, version) = (id.clone(), r.version);
            self.visibility.records.remove(&id);
            self.visibility.floor = self.visibility.floor.max(version);
        }
    }
}

/// A user's added step (§3.4): its id unused in the incarnation, the list
/// within 20.
fn add_step(b: &mut Block, s: &Step) -> Code {
    if b.items.contains_key(&s.id) {
        return Code::Collided;
    }
    let Body::Plan(plan) = &b.body else { return Code::NotThere };
    if plan.steps.len() >= PLAN_STEPS_MAX || b.items.len() >= ITEM_IDS_MAX {
        return Code::Full;
    }
    code_of(path(b, |b| {
        if let Body::Plan(plan) = &mut b.body {
            plan.steps.push(s.clone());
        }
        b.items.insert(s.id.clone(), None);
        Ok(true)
    }))
}

/// A user's `set` of one path, under its guard (§3.4).
fn guarded_set(b: &mut Block, p: &Path, v: &SetValue) -> Code {
    let item = p.item();
    match (&b.body, p.head(), item, p.field(), v) {
        (Body::Plan(plan), "steps", Some(x), Some(f @ ("state" | "stop")), v) => {
            let Some(step) = plan.steps.iter().find(|s| s.id.as_str() == x) else { return Code::NotThere };
            let ran = matches!(step.state, StepState::Doing | StepState::Done | StepState::Failed);
            if ran {
                // Execution truth is the agent's: `deploy already ran`.
                return Code::AlreadyRan;
            }
            if f == "state" {
                // A skip applies to a todo step, an unskip to a skipped one;
                // otherwise there is nothing to do. Decision (pending
                // Haseeb), D4: that is `unchanged`, not a conflict.
                let want = match v {
                    SetValue::Text(w) => StepState::parse(w),
                    _ => None,
                };
                if want == Some(step.state) {
                    return Code::Unchanged;
                }
            }
        }
        (Body::Diff(d), "hunks", Some(x), Some("state"), _) => {
            if d.state != crate::block::DiffState::Proposed {
                return Code::NotProposal;
            }
            if !d.hunks.iter().any(|h| h.id.as_str() == x) {
                return Code::NotThere;
            }
        }
        _ => {}
    }
    code_of(path(b, |b| set_field(b, p, v)))
}

fn code_of(r: Result<bool, PathWhy>) -> Code {
    match r {
        Ok(true) => Code::Applied,
        Ok(false) => Code::Unchanged,
        Err(PathWhy::TooBig) => Code::TooBig,
        Err(PathWhy::Full | PathWhy::Ids) => Code::Full,
        Err(PathWhy::NotThere | PathWhy::Removed(_) | PathWhy::NotKind) => Code::NotThere,
    }
}
