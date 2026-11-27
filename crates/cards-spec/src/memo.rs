//! The card memo (§3.5): what this desk knows of its own answers until the
//! history shows them, as pure transitions. The fold says what the history
//! holds; the memo covers the gap, and says so when an answer may be lost.
//!
//! The desk keeps one memo per session ("keyed by session"). It feeds the
//! memo the session's events ([`Memo::on`]), each fold it computes
//! ([`Memo::observe`], with whether the view is settled), and the clock
//! ([`Memo::tick`]), and it draws each card from the fold and its
//! [`Entry`]. Nothing here sends: [`Memo::press`], [`Memo::retry`] and
//! [`Memo::resend`] return the text, and the desk sends it. A desk that
//! knows more than an entry's `seen` can drop that one entry itself
//! ([`Memo::forget`]).
//!
//! §3.5 states every transition, including these, which S3's review
//! settled:
//! - **`Attached` while sending or held** moves the entry to checking, as
//!   it does a queued one. An answer unconfirmed at a drop settles after
//!   the reconnect (§1.6). A held slot may not survive a daemon restart,
//!   and if it did, rule 3 finds it again.
//! - **A late echo** moves only a checking entry. An unconfirmed one isn't
//!   moved by an echo, but by a settled view: the fold (rule 2), or a
//!   `queued_message` that is ours (rule 3).
//! - **Rule 3 holds for an unconfirmed entry too**, and a Resend click runs
//!   the settle rules on its settled view before it sends: when the slot
//!   holds ours, nothing is sent.
//! - **An `Idle` while held** has the next settled view check it. Ours
//!   still in the slot stays held; a slot that another message replaced, or
//!   that nothing carries any more, settles as `Idle` does.
//! - **The binding.** An entry answers one card: its id, its call's tool id
//!   and its doc's digest. Once the id names another card (replaced,
//!   §2.3), the entry is dropped, and no press, Retry or Resend sends to it.
//! - **"Bound and open"** in the Resend gate means a card that may still
//!   be answered (open, answered or skipped), so a change can be resent.
//! - **A queued `queued_message` beats a superseding answer** on every
//!   settled view, not only in the settle rule (rule 3 before rule 4).
//!
//! **Two subjects** (blocks.md §3.7, B4): a card's answer ([`Memo`], this
//! module's, as S3 reviewed it) and a block's edit ([`block::Memo`]). They
//! share the delivery machine: the states, the events, the timers, and
//! rules 3, 5 and 6 of the settle rule, written once here. Each subject
//! says what its own message is (an event's text, the queued slot's) and
//! what a view shows of it: ours past what was seen, and whether something
//! there supersedes it (rules 1, 2 and 4). A card's entry superseded is
//! dropped; an edit's is retired, by coverage. The card memo keeps its own
//! [`CAP`], under its own rule.

use std::time::{Duration, Instant};

use crate::fold::{Card, Fold, State as Folded};
use crate::message::{self, Reply};
use crate::{CardId, Digest, digest};

pub mod block;

/// Entries per session (§3.5, "Bounds").
pub const CAP: usize = 32;
/// How long a sent answer waits for its echo before it is checked.
pub const ECHO_WAIT: Duration = Duration::from_secs(10);
/// How long a check waits for a settled view.
pub const CHECK_WAIT: Duration = Duration::from_secs(10);

/// An entry's state (§3.5). A dropped entry is removed: its card shows the
/// fold's state, with no Resend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Sending,
    Queued,
    Held,
    Checking,
    InChat,
    Unconfirmed,
    NotSent,
}

/// What moved an entry to checking: its cause if the settle rule finds
/// nothing (rule 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Dequeued,
    DriverRevoked,
    Notice,
    Timeout,
    Attached,
    Idle,
}

/// Why an entry is unconfirmed, or not sent: the card's one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// Rule 6, by what triggered the check.
    Lost(Trigger),
    /// Rule 5: a driver echo, and no `DriverRevoked` since.
    DriverQueue,
    /// No settled view within [`CHECK_WAIT`].
    Unchecked,
    /// `Refused`, `NotAttached` or a reload `Error`, with its reason.
    NotSent(String),
}

impl Cause {
    /// The cause line, as §3.5 words it.
    pub fn line(&self) -> String {
        match self {
            Cause::Lost(Trigger::Dequeued) => "The agent was stopped before reading it".into(),
            Cause::Lost(Trigger::DriverRevoked) => "The driver stopped before reading it".into(),
            Cause::Lost(Trigger::Notice | Trigger::Timeout) => "No reply from the daemon. A driver's queue may be full".into(),
            Cause::Lost(Trigger::Attached) => "The connection dropped or the daemon restarted. It may not have arrived".into(),
            Cause::Lost(Trigger::Idle) => "It didn't reach the chat. Another message may have replaced it".into(),
            Cause::DriverQueue => "It may still be in the driver's queue".into(),
            Cause::Unchecked => "Couldn't check whether it arrived".into(),
            Cause::NotSent(reason) => format!("Not sent: {reason}"),
        }
    }
}

/// One card's answer in flight (§3.5, "The entry"). The session is the
/// memo's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub card: CardId,
    /// The card's binding with `card`: its call and its doc
    /// ([`Entry::answers`]).
    tool_id: String,
    digest: Digest,
    /// The message sent, exactly: events and the fold match it by text.
    pub text: String,
    /// The values, which the card shows until the fold has them.
    pub reply: Reply,
    /// The card's answers in the fold when it was sent, in order.
    pub seen: Vec<String>,
    pub state: State,
    /// Set when unconfirmed or not sent.
    pub cause: Option<Cause>,
    flight: Flight,
}

/// What the delivery machine keeps of an entry in flight, either subject's
/// (§3.5's table).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Flight {
    /// What moved it to checking.
    trigger: Trigger,
    /// Its echo was a driver's (`Steered{delivered: false}` while idle and
    /// not compacting).
    driver: bool,
    /// A `DriverRevoked` came after that echo.
    revoked: bool,
    /// Held, and an `Idle` came: the next settled view checks the slot.
    recheck: bool,
    /// When it entered its state: the timers run from here.
    since: Instant,
}

impl Flight {
    fn new(now: Instant) -> Self {
        Flight { trigger: Trigger::Timeout, driver: false, revoked: false, recheck: false, since: now }
    }
}

/// An entry of either subject, to the delivery machine: its state, its
/// cause and its flight, and which messages are its own.
trait Delivery {
    fn state(&self) -> State;
    fn flight(&self) -> &Flight;
    fn parts(&mut self) -> (&mut State, &mut Option<Cause>, &mut Flight);
    /// Whether `sent` (an event's text, or the queued slot's) is this
    /// entry's message.
    fn carries(&self, sent: &str) -> bool;
    /// Whether events and the clock still move it: a retired edit is
    /// settled for good (§3.7).
    fn moves(&self) -> bool {
        true
    }

    fn to(&mut self, state: State, now: Instant) {
        let (s, cause, f) = self.parts();
        *s = state;
        *cause = None;
        f.since = now;
        f.recheck = false;
        // In the chat, it has left any driver's queue: rule 5 no longer
        // applies if a crash loses it.
        if state == State::InChat {
            f.driver = false;
        }
    }

    fn check(&mut self, trigger: Trigger, now: Instant) {
        self.to(State::Checking, now);
        self.parts().2.trigger = trigger;
    }

    fn unconfirmed(&mut self, cause: Cause, now: Instant) {
        self.to(State::Unconfirmed, now);
        *self.parts().1 = Some(cause);
    }

    /// Sent again: sending, with no echo yet.
    fn sending(&mut self, now: Instant) {
        self.to(State::Sending, now);
        let f = self.parts().2;
        (f.driver, f.revoked) = (false, false);
    }
}

/// What a view shows of an entry's subject, for the shared settle rule:
/// `ours` past the seen prefix, or not; and whether something there
/// supersedes it (for ours, something after it, rule 2; else, something
/// we never saw, rule 4).
#[derive(Clone, Copy, Debug)]
struct Look {
    ours: bool,
    superseded: bool,
}

/// The queued slot of a view, either subject's.
#[derive(Clone, Copy, Debug)]
struct Slot<'a> {
    settled: bool,
    queued: Option<&'a str>,
    streaming: bool,
}

/// The settle rule after rule 1 (§3.5, rules 2 to 6), on what `look` says
/// the view shows: `false` when the entry is superseded (a card's is
/// dropped, an edit's retired).
fn settle<E: Delivery>(e: &mut E, look: Look, view: Slot<'_>, now: Instant) -> bool {
    // Rule 2: ours past the seen prefix is in the chat, unless something
    // follows it that supersedes it.
    if look.ours {
        if look.superseded {
            return false;
        }
        if e.state() != State::InChat {
            e.to(State::InChat, now);
        }
        return true;
    }
    let ours_queued = view.settled && view.queued.is_some_and(|q| e.carries(q));
    if e.state() == State::InChat && view.settled && !ours_queued {
        // A settled history without it: the daemon lost it (a crash
        // before the save).
        e.check(Trigger::Attached, now);
    }
    // Held, and an `Idle` came since: the slot no longer holding ours
    // means another message replaced it, or nothing carries it now.
    if e.state() == State::Held && e.flight().recheck && view.settled {
        e.parts().2.recheck = false;
        if !ours_queued {
            e.check(Trigger::Idle, now);
        }
    }
    // Rule 3, for an unconfirmed entry too: the view shows ours in the
    // slot, so it is on its way, not lost.
    if matches!(e.state(), State::Checking | State::Unconfirmed) && ours_queued {
        e.to(if view.streaming { State::Queued } else { State::Held }, now);
        return true;
    }
    if e.state() == State::Checking && view.settled {
        // Rule 4: something we didn't know about supersedes ours.
        if look.superseded {
            return false;
        }
        // Rules 5 and 6.
        let f = e.flight();
        let cause = if f.driver && !f.revoked { Cause::DriverQueue } else { Cause::Lost(f.trigger) };
        e.unconfirmed(cause, now);
        return true;
    }
    // Any other state: something we didn't know about supersedes ours,
    // unless ours is still in the queued slot.
    ours_queued || !look.superseded
}

/// A session event, for each entry it moves (§3.5's table).
fn hear<E: Delivery>(entries: &mut [E], event: Event<'_>, now: Instant) {
    use State::*;
    let mut each = |states: &[State], text: Option<&str>, f: &mut dyn FnMut(&mut E)| {
        for e in entries.iter_mut().filter(|e| e.moves() && states.contains(&e.state()) && text.is_none_or(|t| e.carries(t))) {
            f(e);
        }
    };
    match event {
        Event::Echo { text, driver } => each(&[Sending, Checking], Some(text), &mut |e| {
            e.to(Queued, now);
            let f = e.parts().2;
            (f.driver, f.revoked) = (driver, false);
        }),
        Event::Delivered { text } => each(&[Queued, Checking], Some(text), &mut |e| e.to(InChat, now)),
        Event::Refused { reason } => {
            if let Some(e) = entries.iter_mut().filter(|e| e.moves() && e.state() == Sending).min_by_key(|e| e.flight().since) {
                e.to(NotSent, now);
                *e.parts().1 = Some(Cause::NotSent(reason.to_string()));
            }
        }
        Event::Dequeued { text } => each(&[Queued, Held], Some(text), &mut |e| e.check(Trigger::Dequeued, now)),
        Event::DriverRevoked { undelivered } => {
            for e in entries.iter_mut().filter(|e| e.moves()) {
                e.parts().2.revoked = true;
                if e.state() == Queued && undelivered.iter().any(|t| e.carries(t)) {
                    e.check(Trigger::DriverRevoked, now);
                }
            }
        }
        Event::Idle => {
            each(&[Queued], None, &mut |e| e.check(Trigger::Idle, now));
            each(&[Held], None, &mut |e| e.parts().2.recheck = true);
        }
        Event::Attached => each(&[Sending, Queued, Held, Checking], None, &mut |e| e.check(Trigger::Attached, now)),
        Event::Notice => each(&[Sending], None, &mut |e| e.check(Trigger::Notice, now)),
    }
}

/// The clock: no echo within [`ECHO_WAIT`] is checked; no settled view
/// within [`CHECK_WAIT`] is unconfirmed.
fn clock<E: Delivery>(entries: &mut [E], now: Instant) {
    for e in entries.iter_mut().filter(|e| e.moves()) {
        let waited = now.saturating_duration_since(e.flight().since);
        match e.state() {
            State::Sending if waited >= ECHO_WAIT => e.check(Trigger::Timeout, now),
            State::Checking if waited >= CHECK_WAIT => e.unconfirmed(Cause::Unchecked, now),
            _ => {}
        }
    }
}

/// The oldest entry sending, of either subject: when it was sent. A
/// `Refused` names no text, so it answers the oldest sending entry: across
/// a session's two memos, the one whose oldest is older.
fn oldest_sending<E: Delivery>(entries: &[E]) -> Option<Instant> {
    entries.iter().filter(|e| e.moves() && e.state() == State::Sending).map(|e| e.flight().since).min()
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

    /// A card's answer is its text, exactly.
    fn carries(&self, sent: &str) -> bool {
        sent == self.text
    }
}

impl Entry {
    /// An answer to this card is out: another press does nothing.
    pub fn in_flight(&self) -> bool {
        matches!(self.state, State::Sending | State::Queued | State::Held | State::Checking)
    }

    /// Whether this entry answers `card`: its id still names the card that
    /// was pressed, the same call and doc. A card bound again to the id
    /// since (replaced, §2.3) is another card. One with the same tool id and
    /// doc would be the same question, so they count as one.
    pub fn answers(&self, card: &Card) -> bool {
        card.id.as_ref() == Some(&self.card)
            && card.tool_id == self.tool_id
            && card.doc.as_ref().is_some_and(|doc| digest(doc, card.wait) == self.digest)
    }

    fn send(&mut self, now: Instant) -> String {
        self.sending(now);
        self.text.clone()
    }

    /// The card's answers in `card`, and how far they agree with `seen`.
    fn split<'a>(&self, card: &'a Card) -> (Vec<&'a str>, usize) {
        let f: Vec<&str> = card.answers.iter().map(|a| a.text.as_str()).collect();
        let k = f.iter().zip(&self.seen).take_while(|(a, b)| **a == b.as_str()).count();
        (f, k)
    }

    /// Follow the fold; `false` drops the entry.
    fn follow(&mut self, view: &View<'_>, now: Instant) -> bool {
        // Rule 1. Only a settled view says a card is gone: a stale one may
        // just not have it yet. Closed, or replaced (its id names another
        // card now), it is dropped on any view.
        let Some(card) = view.fold.bound(&self.card) else {
            return !view.settled;
        };
        if card.state == Folded::Closed || !self.answers(card) {
            return false;
        }
        // Rule 2: ours past the seen prefix, unless an answer follows it;
        // rule 4: an answer we didn't know about.
        let (f, k) = self.split(card);
        let look = match f[k..].iter().rposition(|t| *t == self.text) {
            Some(i) => Look { ours: true, superseded: k + i + 1 < f.len() },
            None => Look { ours: false, superseded: k < f.len() },
        };
        settle(self, look, view.slot(), now)
    }

    /// The memo's half of the Resend gate: the card may still be answered,
    /// and its answers are a prefix of `seen`. The desk adds
    /// `input_gate()`.
    fn gate(&self, fold: &Fold) -> bool {
        let Some(card) = fold.bound(&self.card).filter(|c| c.answerable() && self.answers(c)) else {
            return false;
        };
        let (f, k) = self.split(card);
        self.state == State::Unconfirmed && k == f.len()
    }
}

/// A session event the memo reads (§3.4, §3.5), as D2a decodes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// `TurnStarted{user_text}` or `Steered{text}`. `driver`: a
    /// `Steered{delivered: false}` while idle and not compacting.
    Echo { text: &'a str, driver: bool },
    /// `SteeringDelivered{message}`.
    Delivered { text: &'a str },
    /// `Refused`, `NotAttached` or a reload `Error` for a `submit` or
    /// `steer`: they name no text, so they answer the oldest sending entry.
    Refused { reason: &'a str },
    /// `Dequeued{text}`.
    Dequeued { text: &'a str },
    /// `DriverRevoked{undelivered_steering}`.
    DriverRevoked { undelivered: &'a [String] },
    /// `Idle`.
    Idle,
    /// `Attached` on a reconnect.
    Attached,
    /// The broadcast "steering queue full" `SystemNotice`: a hint, not a
    /// receipt.
    Notice,
}

/// What the desk holds of the session: the fold of its history and, when
/// the history matches the latest `Conversation` digest (`settled`), that
/// digest's `queued_message` and whether it is streaming.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    pub fold: &'a Fold,
    pub settled: bool,
    pub queued: Option<&'a str>,
    pub streaming: bool,
}

impl View<'_> {
    fn slot(&self) -> Slot<'_> {
        Slot { settled: self.settled, queued: self.queued, streaming: self.streaming }
    }
}

/// What a Resend click does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resend {
    /// Send this text again.
    Send(String),
    /// The view isn't settled: the card shows "Checking…", and nothing is
    /// sent.
    Checking,
    /// The gate doesn't hold: nothing is sent.
    No,
}

/// One session's entries, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Memo {
    entries: Vec<Entry>,
}

impl Memo {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, card: &CardId) -> Option<&Entry> {
        self.entries.iter().find(|e| e.card == *card)
    }

    fn get_mut(&mut self, card: &CardId) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.card == *card)
    }

    /// A press on `card` with `reply`: the text to send, or `None` when the
    /// card can't be answered (a replaced card can't) or an answer to it is
    /// still in flight. A new
    /// entry replaces the card's last one (a change of answer); past
    /// [`CAP`] it evicts the oldest entry in the chat, else the oldest
    /// unconfirmed one, else the oldest.
    pub fn press(&mut self, card: &Card, reply: Reply, now: Instant) -> Option<String> {
        let (Some(id), Some(doc)) = (card.id.as_ref(), card.doc.as_ref()) else {
            return None;
        };
        if !card.answerable() || self.get(id).is_some_and(|e| e.in_flight() && e.answers(card)) {
            return None;
        }
        let text = message::build(id, doc, &reply);
        self.entries.retain(|e| e.card != *id);
        if self.entries.len() >= CAP {
            let oldest = |s: State| self.entries.iter().position(|e| e.state == s);
            let i = oldest(State::InChat).or_else(|| oldest(State::Unconfirmed)).unwrap_or(0);
            self.entries.remove(i);
        }
        self.entries.push(Entry {
            card: id.clone(),
            tool_id: card.tool_id.clone(),
            digest: digest(doc, card.wait),
            text: text.clone(),
            reply,
            seen: card.answers.iter().map(|a| a.text.clone()).collect(),
            state: State::Sending,
            cause: None,
            flight: Flight::new(now),
        });
        Some(text)
    }

    /// Retry a card not sent, while it may still be answered: the same text.
    pub fn retry(&mut self, card: &CardId, fold: &Fold, now: Instant) -> Option<String> {
        let e = self.get_mut(card).filter(|e| e.state == State::NotSent)?;
        if !fold.bound(card).is_some_and(|c| c.answerable() && e.answers(c)) {
            return None;
        }
        Some(e.send(now))
    }

    /// Whether to show Resend on `card` now (with the desk's
    /// `input_gate()`).
    pub fn may_resend(&self, card: &CardId, fold: &Fold) -> bool {
        self.get(card).is_some_and(|e| e.gate(fold))
    }

    /// A Resend click, against a settled view: the settle rules on that
    /// view first, then the gate again. A view that shows ours in the slot
    /// (rule 3) moves it to queued or held, and nothing is sent.
    pub fn resend(&mut self, card: &CardId, view: &View<'_>, now: Instant) -> Resend {
        let Some(i) = self.entries.iter().position(|e| e.card == *card && e.state == State::Unconfirmed) else {
            return Resend::No;
        };
        let e = &mut self.entries[i];
        if !view.settled {
            let trigger = e.flight.trigger;
            e.check(trigger, now);
            return Resend::Checking;
        }
        if !e.follow(view, now) {
            self.entries.remove(i);
            return Resend::No;
        }
        if e.state != State::Unconfirmed || !e.gate(view.fold) {
            return Resend::No;
        }
        Resend::Send(e.send(now))
    }

    /// A session event.
    pub fn on(&mut self, event: Event<'_>, now: Instant) {
        hear(&mut self.entries, event, now);
    }

    /// The clock: no echo within [`ECHO_WAIT`] is checked; no settled view
    /// within [`CHECK_WAIT`] is unconfirmed.
    pub fn tick(&mut self, now: Instant) {
        clock(&mut self.entries, now);
    }

    /// When the oldest entry sending was sent: a `Refused` answers it,
    /// across a session's card and block memos alike (B4).
    pub fn oldest_sending(&self) -> Option<Instant> {
        oldest_sending(&self.entries)
    }

    /// A fold the desk computed: ours seen, superseded, closed or gone;
    /// and, on a settled view, the settle rule for each entry checking.
    pub fn observe(&mut self, view: &View<'_>, now: Instant) {
        self.entries.retain_mut(|e| e.follow(view, now));
    }

    /// Drop `card`'s entry on the desk's own word (§3.5, "What drops an
    /// entry"), and nothing else: every other entry stays as it was, in
    /// its order. For a desk that knows more than the entry's `seen`. For
    /// example, a change whose `seen` counts an earlier answer the desk
    /// knows a turn's end lost: another desk's copy of that answer then
    /// reads as the one `seen` expected (rule 4 doesn't fire), and only
    /// the desk can tell it supersedes ours. The card then shows the
    /// fold's state, with no Resend or Retry, and a press starts a new
    /// entry that sees the fold's answers. Returns the entry dropped, if
    /// the card had one.
    pub fn forget(&mut self, card: &CardId) -> Option<Entry> {
        let i = self.entries.iter().position(|e| e.card == *card)?;
        Some(self.entries.remove(i))
    }
}

#[cfg(test)]
mod tests;
