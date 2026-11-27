//! The memo for block edits (blocks.md §3.7, B4): what this desk knows of
//! its own hand-backs until the history shows them, as pure transitions.
//! It answers "did my message reach the chat?", never "what did it do?":
//! that is the board's ([`crate::board`], the edit's outcome), and "in the
//! chat" never means "applied".
//!
//! The delivery machine is the card memo's ([`super`]): the same states,
//! events, timers, and rules 3, 5 and 6 of the settle rule, and the same
//! [`Resend`] gate's shape. The desk feeds this memo what it feeds the
//! card memo: the session's events ([`Memo::on`]), each view it computes
//! ([`Memo::observe`]) and the clock ([`Memo::tick`]). What differs is the
//! subject:
//!
//! - **The entry** is one hand-back, matched by the exact text of its edit:
//!   its body, every line but the notice lines ([`message::split`]). The
//!   notice lines are the desires' and are built afresh at every send
//!   (§3.5), so an event's text, the queued slot's and a history message's
//!   are matched by their body, whatever trailer rides on them, and a
//!   Resend or a Retry sends the body with the trailer the desk builds at
//!   that moment.
//! - **The binding** is (block id, tag, base) as sent ([`Entry::id`],
//!   [`Entry::of`], [`Entry::base`]). Agent ticks never move it: the base
//!   is what was sent, not the block's current revision, and a Resend sends
//!   the same bytes.
//! - **What a view shows** ([`Edits`]): the history's edits of the entry's
//!   incarnation, by their body, and whether the incarnation is live on
//!   the board folded from the same history. Ours past what was seen is in
//!   the chat (the seen-prefix rule, as for cards), and so are identical
//!   texts past it.
//! - **Supersession is by coverage** (§3.7). An edit's coverage keys are
//!   its intents ([`crate::board::keys`]): each appended item, each `set`
//!   path, each ordered list. A later edit of the same incarnation covers
//!   an entry only if its keys include every key of the entry's edit; an
//!   appended hunk line or series number has no key, so nothing covers an
//!   edit with one. Covered, the entry is **retired** ([`Retired`]) where a
//!   card's answer would be dropped: settled for good, with no Resend or
//!   Retry, kept until evicted. A disjoint or partial later edit leaves it,
//!   with its Resend. Rule 3 still comes first: ours in the queued slot
//!   goes in after the other, and isn't covered by it. This desk's own
//!   covering hand-back retires its earlier entries as it is accepted
//!   ([`Retired::Here`]); a covering edit in the chat that isn't (another
//!   desk's) on the view that shows it ([`Retired::Elsewhere`]).
//! - **An ended incarnation** (replaced, removed, compacted away) doesn't
//!   drop the entry, as a card that leaves the history drops its own (rule
//!   1): the message is still on its way or in the chat, and the delivery
//!   entry carries on until it settles. Its Resend and Retry are off,
//!   since a resend would apply nothing.
//! - **One edit in flight per incarnation** (§3.7): a hand-back while one
//!   is sending, queued, held or checking is refused ([`Refusal::InFlight`]).
//!   Another incarnation is another block: a replaced block takes a
//!   hand-back while the edit of its predecessor is still on its way.
//! - **Bounds** (§1.8): [`CAP`] entries. A new hand-back past it evicts the
//!   oldest retired entry, else the oldest in the chat. **Recoverable
//!   entries never go** (sending, queued, held, checking, unconfirmed, not
//!   sent): with [`CAP`] of them, Hand back is off ([`Memo::may_hand_back`],
//!   [`FULL`]) until one is resent or discarded ([`Memo::forget`]).
//!
//! **The seen prefix, bounded.** A card's entry keeps the texts it had seen.
//! An edit's keeps, of its incarnation's edits in the history when it was
//! sent, how many there were, a 128-bit digest of each of the latest
//! [`SEEN_MAX`], and one digest chained over the rest ([`Seen`]), so an
//! entry is its body (line 1 ≤ 200 characters, line 2 ≤ 8 KiB) and a few
//! hundred bytes. The prefix is exact while the history agrees with what
//! was seen up to the kept digests (a growing history, a rollback within
//! them). Where it doesn't (a compaction, or a rollback deeper than
//! [`SEEN_MAX`] of the incarnation's edits), the memo can't tell which of
//! the history's edits it had seen: ours is matched anywhere in what the
//! history holds, and only an edit after it can cover it, so an edit the
//! memo had seen never retires it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use super::{Cause, Delivery, Event, Flight, Look, Resend, Slot, State, hear, settle};
use crate::Error;
use crate::block::message::{self, Edit, Summary, Trailer};
use crate::block::{Appended, Ref, Tag};
use crate::board::{self, Key};
use crate::digest::sha256;
use crate::fold::Event as Folded;

/// Block-edit entries per session (§1.8, §3.7, "Bounds").
pub const CAP: usize = 32;
/// The digests of the latest edits an entry keeps, of those it had seen
/// (module docs).
pub const SEEN_MAX: usize = 8;
/// Why Hand back is off, with [`CAP`] recoverable entries (§1.8).
pub const FULL: &str = "32 edits not confirmed · resend or discard one";

/// What the desk holds of the history, for the memo (§3.7): the history's
/// edits of an incarnation, and whether the board folded from the same
/// history holds it live. [`History`] reads a history the desk holds whole;
/// a desk that keeps its history otherwise answers the same.
pub trait Edits {
    /// The bodies ([`message::read`]'s `body`) of the history's genuine
    /// user messages whose body is an edit of incarnation `of` of block
    /// `id`, as §3.3's grammar reads it, in history order.
    fn of(&self, id: &Ref, of: &Tag) -> Vec<&str>;
    /// Whether that incarnation is live on the board: a hand-back to it can
    /// still apply.
    fn live(&self, id: &Ref, of: &Tag) -> bool;
}

/// The [`Edits`] of a history held whole: its events, as
/// [`crate::fold::events`] reads them, and the board folded from them.
/// Each edit's body is borrowed from its message, indexed once.
pub struct History<'a> {
    by: BTreeMap<(Ref, Tag), Vec<&'a str>>,
    board: &'a board::State,
}

impl<'a> History<'a> {
    pub fn new(events: &'a [Folded], board: &'a board::State) -> Self {
        let mut by: BTreeMap<(Ref, Tag), Vec<&'a str>> = BTreeMap::new();
        for e in events {
            if let Folded::User { text } = e {
                let read = message::read(text);
                if let Some(edit) = read.edit {
                    by.entry((edit.id, edit.of)).or_default().push(read.body);
                }
            }
        }
        History { by, board }
    }
}

impl Edits for History<'_> {
    fn of(&self, id: &Ref, of: &Tag) -> Vec<&str> {
        self.by.get(&(id.clone(), *of)).cloned().unwrap_or_default()
    }

    fn live(&self, id: &Ref, of: &Tag) -> bool {
        self.board.block(id).is_some_and(|b| b.tag == *of)
    }
}

/// What the desk holds of the session, for its hand-backs: the history's
/// edits ([`Edits`]) and, when the history matches the latest
/// `Conversation` digest (`settled`), that digest's `queued_message`
/// (whole: the memo reads its body) and whether it is streaming. As the
/// card memo's [`super::View`].
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub edits: &'a dyn Edits,
    pub settled: bool,
    pub queued: Option<&'a str>,
    pub streaming: bool,
}

impl View<'_> {
    fn slot(&self) -> Slot<'_> {
        Slot { settled: self.settled, queued: self.queued, streaming: self.streaming }
    }
}

/// Why an entry was retired (§3.7, "Supersession is by coverage"): a later
/// edit of its incarnation covers every key of its edit. It is settled for
/// good: no event moves it, and it has no Resend or Retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retired {
    /// This desk's later hand-back, as it was accepted.
    Here,
    /// A later edit in the chat: another desk's (the row says so).
    Elsewhere,
}

/// Why a hand-back is refused: nothing is sent, and the memo is as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The incarnation isn't live on the view: the edit would apply
    /// nothing.
    Gone,
    /// An edit of this incarnation is in flight: the block shows
    /// "Sending…" and takes no new hold (§3.7).
    InFlight,
    /// [`CAP`] recoverable entries: Hand back is off ([`FULL`]).
    Full,
    /// The edit's message can't be built ([`message::build`]).
    Edit(Error),
}

/// What an entry had seen of its incarnation's edits in the history when
/// it was sent, bounded (module docs): how many, a digest chained over all
/// but the latest [`SEEN_MAX`], and each of those latest ones' digest.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    n: usize,
    older: [u8; 16],
    last: Vec<[u8; 16]>,
}

fn digest(body: &str) -> [u8; 16] {
    let h = sha256(body.as_bytes());
    h[..16].try_into().expect("16 of 32 bytes")
}

/// The chain digest of `bodies`, in order.
fn chain(bodies: &[&str]) -> [u8; 16] {
    bodies.iter().fold([0; 16], |c, b| {
        let mut pre = c.to_vec();
        pre.extend_from_slice(&digest(b));
        sha256(&pre)[..16].try_into().expect("16 of 32 bytes")
    })
}

impl Seen {
    fn of(f: &[&str]) -> Self {
        let skip = f.len().saturating_sub(SEEN_MAX);
        Seen { n: f.len(), older: chain(&f[..skip]), last: f[skip..].iter().map(|b| digest(b)).collect() }
    }

    /// How far the history's edits `f` agree with what was seen: the
    /// seen-prefix rule's common prefix. `None` where `f` differs before the
    /// latest [`SEEN_MAX`] seen, which the memo can't place (module docs).
    fn agrees(&self, f: &[&str]) -> Option<usize> {
        let skip = self.n - self.last.len();
        if f.len() < skip || chain(&f[..skip]) != self.older {
            return None;
        }
        let mut k = skip;
        while k < self.n && k < f.len() && digest(f[k]) == self.last[k - skip] {
            k += 1;
        }
        Some(k)
    }
}

/// An edit's intents (§3.7's coverage keys), and whether it has one no key
/// names: an appended hunk line or series number, which nothing covers.
struct Intents {
    keys: BTreeSet<Key>,
    keyless: bool,
}

impl Intents {
    fn of(e: &Edit) -> Self {
        let keys: BTreeSet<Key> = board::keys(e).into_iter().collect();
        let items: usize = e.parts.append.values().map(|a| if let Appended::Steps(s) = a { s.len() } else { 0 }).sum();
        let named = keys.iter().filter(|k| matches!(k, Key::Item(_))).count();
        let keyless = e.parts.append.values().any(|a| !matches!(a, Appended::Steps(_))) || named < items;
        Intents { keys, keyless }
    }

    /// Whether a later edit with these intents covers an earlier one with
    /// `theirs`: every key of `theirs` among its own, and none of `theirs`
    /// unnamed.
    fn covers(&self, theirs: &Intents) -> bool {
        !theirs.keyless && theirs.keys.is_subset(&self.keys)
    }
}

/// One hand-back in flight (§3.7, "Delivery"). The session is the memo's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The binding, as sent: the block, its incarnation, and the revision
    /// the user edited from. Agent ticks never move it.
    pub id: Ref,
    pub of: Tag,
    pub base: u32,
    /// The edit's body as sent, exactly: line 1 and line 2, without notice
    /// lines. Events, the slot and the history match it.
    pub text: String,
    seen: Seen,
    pub state: State,
    /// Set when unconfirmed or not sent.
    pub cause: Option<Cause>,
    /// Set when a later edit covers it: settled for good.
    pub retired: Option<Retired>,
    flight: Flight,
}

impl Delivery for Entry {
    fn state(&self) -> State {
        self.state
    }

    fn flight(&self) -> &Flight {
        &self.flight
    }

    fn parts(&mut self) -> (&mut State, &mut Option<Cause>, &mut Flight) {
        (&mut self.state, &mut self.cause, &mut self.flight)
    }

    /// An edit's message is its body, whatever notice lines ride on it.
    fn carries(&self, sent: &str) -> bool {
        message::split(sent).0 == self.text
    }

    fn moves(&self) -> bool {
        self.retired.is_none()
    }
}

impl Entry {
    /// An edit of its incarnation is out: the block shows "Sending…", and
    /// another hand-back to it is refused.
    pub fn in_flight(&self) -> bool {
        self.retired.is_none() && matches!(self.state, State::Sending | State::Queued | State::Held | State::Checking)
    }

    /// Never evicted (§1.8): neither retired nor in the chat.
    pub fn recoverable(&self) -> bool {
        self.retired.is_none() && self.state != State::InChat
    }

    fn edits_incarnation(&self, id: &Ref, of: &Tag) -> bool {
        self.id == *id && self.of == *of
    }

    /// Its edit's intents. Its body is one [`message::build`] wrote, so it
    /// reads back.
    fn intents(&self) -> Option<Intents> {
        message::parse(&self.text).map(|e| Intents::of(&e))
    }

    /// Whether any of `later` (edits' bodies) covers this entry's edit.
    fn covered(&self, later: &[&str]) -> bool {
        if later.is_empty() {
            return false;
        }
        let Some(ours) = self.intents() else { return false };
        later.iter().filter_map(|b| message::parse(b)).any(|e| Intents::of(&e).covers(&ours))
    }

    /// What `f` (the history's edits of its incarnation) shows of it: ours
    /// past the seen prefix, and whether a later edit covers it (rules 2
    /// and 4, by coverage).
    fn look(&self, f: &[&str]) -> Look {
        let (from, placed) = match self.seen.agrees(f) {
            Some(k) => (k, true),
            None => (0, false),
        };
        match f[from..].iter().rposition(|t| *t == self.text) {
            Some(i) => Look { ours: true, superseded: self.covered(&f[from + i + 1..]) },
            // Only edits past what was seen can supersede it: where the
            // memo can't place those, none does.
            None => Look { ours: false, superseded: placed && self.covered(&f[from..]) },
        }
    }

    /// Follow a view: in the chat, covered, or the shared settle rule. An
    /// ended incarnation drops nothing (module docs).
    fn follow(&mut self, view: &View<'_>, now: Instant) {
        if self.retired.is_some() {
            return;
        }
        let f = view.edits.of(&self.id, &self.of);
        let look = self.look(&f);
        // Ours in the chat is in the chat, covered or not.
        if look.ours && self.state != State::InChat {
            self.to(State::InChat, now);
        }
        if !settle(self, look, view.slot(), now) {
            self.retired = Some(Retired::Elsewhere);
        }
    }

    /// The memo's half of the Resend gate: unconfirmed, the incarnation
    /// live, and the history neither holds ours nor covers it. The desk
    /// adds `input_gate()`.
    fn gate(&self, edits: &dyn Edits) -> bool {
        if self.retired.is_some() || self.state != State::Unconfirmed || !edits.live(&self.id, &self.of) {
            return false;
        }
        let look = self.look(&edits.of(&self.id, &self.of));
        !look.ours && !look.superseded
    }

    /// Sent again: its body, with `trailer`.
    fn send(&mut self, trailer: &Trailer, now: Instant) -> String {
        self.sending(now);
        format!("{}{}", self.text, trailer.text())
    }
}

/// One session's hand-backs, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Memo {
    entries: Vec<Entry>,
}

impl Memo {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry of the hand-back `text`: its body, or a message that
    /// carries it (any trailer).
    pub fn get(&self, text: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.carries(text))
    }

    /// The edit of incarnation `of` of `id` in flight, if any: the block
    /// shows "Sending…".
    pub fn in_flight(&self, id: &Ref, of: &Tag) -> Option<&Entry> {
        self.entries.iter().find(|e| e.edits_incarnation(id, of) && e.in_flight())
    }

    /// Hand back is on: fewer than [`CAP`] entries, or one that is settled
    /// and can go (§1.8). Else it shows [`FULL`].
    pub fn may_hand_back(&self) -> bool {
        self.entries.len() < CAP || self.entries.iter().any(|e| !e.recoverable())
    }

    /// A hand-back of `edit` (the desk's, [`Edit::new`]'s), its line 1 from
    /// `summary`: the message to send, its body and `trailer`, or why not
    /// (nothing changes then). The incarnation must be live on `edits` and
    /// have no edit in flight, and there must be room. A new entry sees the
    /// incarnation's edits in the history; the same edit again replaces its
    /// entry; this desk's earlier entries of the incarnation it covers are
    /// retired here; past [`CAP`], the oldest retired entry goes, else the
    /// oldest in the chat, never a recoverable one.
    pub fn hand_back(
        &mut self,
        edit: &Edit,
        summary: &Summary<'_>,
        trailer: &Trailer,
        edits: &dyn Edits,
        now: Instant,
    ) -> Result<String, Refusal> {
        let (id, of) = (&edit.id, &edit.of);
        if !edits.live(id, of) {
            return Err(Refusal::Gone);
        }
        if self.in_flight(id, of).is_some() {
            return Err(Refusal::InFlight);
        }
        let text = message::build(edit, summary).map_err(Refusal::Edit)?;
        let mine = Intents::of(edit);
        let covers =
            |e: &Entry| e.retired.is_none() && e.edits_incarnation(id, of) && e.intents().is_some_and(|theirs| mine.covers(&theirs));
        let kept = self.entries.iter().filter(|e| e.text != text);
        if kept.clone().count() >= CAP && !kept.clone().any(|e| !e.recoverable() || covers(e)) {
            return Err(Refusal::Full);
        }
        self.entries.retain(|e| e.text != text);
        for e in &mut self.entries {
            if covers(e) {
                e.retired = Some(Retired::Here);
            }
        }
        if self.entries.len() >= CAP {
            let oldest = |f: fn(&Entry) -> bool| self.entries.iter().position(f);
            let i = oldest(|e| e.retired.is_some()).or_else(|| oldest(|e| e.state == State::InChat)).expect("a settled entry to go");
            self.entries.remove(i);
        }
        self.entries.push(Entry {
            id: id.clone(),
            of: *of,
            base: edit.base,
            text: text.clone(),
            seen: Seen::of(&edits.of(id, of)),
            state: State::Sending,
            cause: None,
            retired: None,
            flight: Flight::new(now),
        });
        Ok(format!("{text}{}", trailer.text()))
    }

    /// Retry a hand-back not sent, while its incarnation is live: its body,
    /// with `trailer`.
    pub fn retry(&mut self, text: &str, trailer: &Trailer, edits: &dyn Edits, now: Instant) -> Option<String> {
        let e = self.entries.iter_mut().find(|e| e.carries(text) && e.retired.is_none() && e.state == State::NotSent)?;
        if !edits.live(&e.id, &e.of) {
            return None;
        }
        Some(e.send(trailer, now))
    }

    /// Whether to show Resend on hand-back `text` now (with the desk's
    /// `input_gate()`).
    pub fn may_resend(&self, text: &str, edits: &dyn Edits) -> bool {
        self.get(text).is_some_and(|e| e.gate(edits))
    }

    /// A Resend click, against a settled view: the settle rules on that view
    /// first, then the gate again. Only the exact edit text goes, with
    /// `trailer`, the desires' at this moment (§3.7).
    pub fn resend(&mut self, text: &str, trailer: &Trailer, view: &View<'_>, now: Instant) -> Resend {
        let Some(e) = self.entries.iter_mut().find(|e| e.carries(text) && e.retired.is_none() && e.state == State::Unconfirmed) else {
            return Resend::No;
        };
        if !view.settled {
            let trigger = e.flight.trigger;
            e.check(trigger, now);
            return Resend::Checking;
        }
        e.follow(view, now);
        if !e.gate(view.edits) {
            return Resend::No;
        }
        Resend::Send(e.send(trailer, now))
    }

    /// A session event.
    pub fn on(&mut self, event: Event<'_>, now: Instant) {
        hear(&mut self.entries, event, now);
    }

    /// The clock, as the card memo's.
    pub fn tick(&mut self, now: Instant) {
        super::clock(&mut self.entries, now);
    }

    /// When the oldest entry sending was sent ([`super::Memo::oldest_sending`]).
    pub fn oldest_sending(&self) -> Option<Instant> {
        super::oldest_sending(&self.entries)
    }

    /// A view the desk computed: ours in the chat, covered, or the settle
    /// rule for each entry.
    pub fn observe(&mut self, view: &View<'_>, now: Instant) {
        for e in &mut self.entries {
            e.follow(view, now);
        }
    }

    /// Discard (§3.7): drop hand-back `text`'s entry, whatever its state,
    /// and nothing else. Its row then shows what the board says, with no
    /// Resend or Retry. Returns the entry dropped, if there was one.
    pub fn forget(&mut self, text: &str) -> Option<Entry> {
        let i = self.entries.iter().position(|e| e.carries(text))?;
        Some(self.entries.remove(i))
    }
}

#[cfg(test)]
mod tests;
