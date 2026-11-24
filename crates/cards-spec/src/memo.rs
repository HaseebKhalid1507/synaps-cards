//! The card memo (§3.5): what this desk knows of its own answers until the
//! history shows them, as pure transitions. The fold says what the history
//! holds; the memo covers the gap, and says so when an answer may be lost.
//!
//! The desk keeps one memo per session ("keyed by session"). It feeds the
//! memo the session's events ([`Memo::on`]), each fold it computes
//! ([`Memo::observe`], with whether the view is settled), and the clock
//! ([`Memo::tick`]), and it draws each card from the fold and its
//! [`Entry`]. Nothing here sends: [`Memo::press`], [`Memo::retry`] and
//! [`Memo::resend`] return the text, and the desk sends it.
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

use std::time::{Duration, Instant};

use crate::fold::{Card, Fold, State as Folded};
use crate::message::{self, Reply};
use crate::{CardId, Digest, digest};

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

    fn to(&mut self, state: State, now: Instant) {
        self.state = state;
        self.cause = None;
        self.since = now;
        self.recheck = false;
        // In the chat, it has left any driver's queue: rule 5 no longer
        // applies if a crash loses it.
        if state == State::InChat {
            self.driver = false;
        }
    }

    fn check(&mut self, trigger: Trigger, now: Instant) {
        self.to(State::Checking, now);
        self.trigger = trigger;
    }

    fn unconfirmed(&mut self, cause: Cause, now: Instant) {
        self.to(State::Unconfirmed, now);
        self.cause = Some(cause);
    }

    fn send(&mut self, now: Instant) -> String {
        self.to(State::Sending, now);
        (self.driver, self.revoked) = (false, false);
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
        // Rule 2: ours past the seen prefix is in the chat, unless an
        // answer follows it.
        let (f, k) = self.split(card);
        if let Some(i) = f[k..].iter().rposition(|t| *t == self.text) {
            if k + i + 1 < f.len() {
                return false;
            }
            if self.state != State::InChat {
                self.to(State::InChat, now);
            }
            return true;
        }
        let ours_queued = view.settled && view.queued == Some(self.text.as_str());
        if self.state == State::InChat && view.settled && !ours_queued {
            // A settled history without it: the daemon lost it (a crash
            // before the save).
            self.check(Trigger::Attached, now);
        }
        // Held, and an `Idle` came since: the slot no longer holding ours
        // means another message replaced it, or nothing carries it now.
        if self.state == State::Held && self.recheck && view.settled {
            self.recheck = false;
            if !ours_queued {
                self.check(Trigger::Idle, now);
            }
        }
        // Rule 3, for an unconfirmed entry too: the view shows ours in the
        // slot, so it is on its way, not lost.
        if matches!(self.state, State::Checking | State::Unconfirmed) && ours_queued {
            self.to(if view.streaming { State::Queued } else { State::Held }, now);
            return true;
        }
        if self.state == State::Checking && view.settled {
            // Rule 4: an answer we didn't know about.
            if k < f.len() {
                return false;
            }
            // Rules 5 and 6.
            let cause = if self.driver && !self.revoked { Cause::DriverQueue } else { Cause::Lost(self.trigger) };
            self.unconfirmed(cause, now);
            return true;
        }
        // Any other state: an answer we didn't know about supersedes ours,
        // unless ours is still in the queued slot.
        ours_queued || k == f.len()
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
            trigger: Trigger::Timeout,
            driver: false,
            revoked: false,
            recheck: false,
            since: now,
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
            let trigger = e.trigger;
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
        use State::*;
        let mut each = |states: &[State], text: Option<&str>, f: &mut dyn FnMut(&mut Entry)| {
            for e in self.entries.iter_mut().filter(|e| states.contains(&e.state) && text.is_none_or(|t| e.text == t)) {
                f(e);
            }
        };
        match event {
            Event::Echo { text, driver } => each(&[Sending, Checking], Some(text), &mut |e| {
                e.to(Queued, now);
                (e.driver, e.revoked) = (driver, false);
            }),
            Event::Delivered { text } => each(&[Queued, Checking], Some(text), &mut |e| e.to(InChat, now)),
            Event::Refused { reason } => {
                if let Some(e) = self.entries.iter_mut().filter(|e| e.state == Sending).min_by_key(|e| e.since) {
                    e.to(NotSent, now);
                    e.cause = Some(Cause::NotSent(reason.to_string()));
                }
            }
            Event::Dequeued { text } => each(&[Queued, Held], Some(text), &mut |e| e.check(Trigger::Dequeued, now)),
            Event::DriverRevoked { undelivered } => {
                for e in &mut self.entries {
                    e.revoked = true;
                    if e.state == Queued && undelivered.contains(&e.text) {
                        e.check(Trigger::DriverRevoked, now);
                    }
                }
            }
            Event::Idle => {
                each(&[Queued], None, &mut |e| e.check(Trigger::Idle, now));
                each(&[Held], None, &mut |e| e.recheck = true);
            }
            Event::Attached => each(&[Sending, Queued, Held, Checking], None, &mut |e| e.check(Trigger::Attached, now)),
            Event::Notice => each(&[Sending], None, &mut |e| e.check(Trigger::Notice, now)),
        }
    }

    /// The clock: no echo within [`ECHO_WAIT`] is checked; no settled view
    /// within [`CHECK_WAIT`] is unconfirmed.
    pub fn tick(&mut self, now: Instant) {
        for e in &mut self.entries {
            let waited = now.saturating_duration_since(e.since);
            match e.state {
                State::Sending if waited >= ECHO_WAIT => e.check(Trigger::Timeout, now),
                State::Checking if waited >= CHECK_WAIT => e.unconfirmed(Cause::Unchecked, now),
                _ => {}
            }
        }
    }

    /// A fold the desk computed: ours seen, superseded, closed or gone;
    /// and, on a settled view, the settle rule for each entry checking.
    pub fn observe(&mut self, view: &View<'_>, now: Instant) {
        self.entries.retain_mut(|e| e.follow(view, now));
    }
}

#[cfg(test)]
mod tests;
