//! `memo`: §3.5's table and settle rule, each case as the event sequence
//! the daemon sends for it (§3.4's rows).

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;
use crate::fold::{self, Event as F};
use crate::{CardDoc, Envelope, Input, parse_answer, parse_input};

const ID: &str = "c4f9x2a";

fn id(s: &str) -> CardId {
    CardId::parse(s).unwrap()
}

fn ask() -> Value {
    json!({ "question": "Which approach?", "options": ["Refactor the parser", "Rewrite from scratch", "Leave it"] })
}

fn doc() -> CardDoc {
    match parse_input(&ask()).unwrap() {
        Input::Create { doc, .. } => doc,
        Input::Close(_) => unreachable!(),
    }
}

/// A card bound to `card` on tool call `tool`.
fn bound(tool: &str, card: &str) -> [F; 2] {
    let line = Envelope::showing(id(card), &doc(), false).to_string();
    [F::Call { tool_id: tool.into(), name: "cards_ui".into(), input: ask() }, F::Result { tool_id: tool.into(), text: line }]
}

fn answer(choice: &str) -> Reply {
    Reply::Answer(parse_answer(&doc(), &json!({ "choice": choice })).unwrap())
}

fn change(choice: &str) -> Reply {
    Reply::Change(parse_answer(&doc(), &json!({ "choice": choice })).unwrap())
}

fn text(card: &str, reply: &Reply) -> String {
    message::build(&id(card), &doc(), reply)
}

/// One desk's view of one session: the history it holds, its memo, a clock.
struct Desk {
    history: Vec<F>,
    memo: Memo,
    now: Instant,
}

impl Desk {
    fn new() -> Self {
        Desk { history: bound("t1", ID).to_vec(), memo: Memo::default(), now: Instant::now() }
    }

    fn fold(&self) -> Fold {
        fold::fold(&self.history)
    }

    fn press(&mut self, reply: Reply) -> Option<String> {
        let card = self.fold().bound(&id(ID)).cloned().unwrap();
        self.memo.press(&card, reply, self.now)
    }

    fn on(&mut self, e: Event<'_>) {
        self.memo.on(e, self.now);
    }

    /// The history gains this user message.
    fn lands(&mut self, text: &str) {
        self.history.push(F::User { text: text.into() });
    }

    fn view(&mut self, settled: bool, queued: Option<&str>, streaming: bool) {
        let f = self.fold();
        self.memo.observe(&View { fold: &f, settled, queued, streaming }, self.now);
    }

    /// A settled view, idle, with no queued message.
    fn settles(&mut self) {
        self.view(true, None, false);
    }

    fn after(&mut self, secs: u64) {
        self.now += Duration::from_secs(secs);
        self.memo.tick(self.now);
    }

    fn state(&self) -> Option<State> {
        self.memo.get(&id(ID)).map(|e| e.state)
    }

    fn cause(&self) -> Option<String> {
        self.memo.get(&id(ID)).and_then(|e| e.cause.as_ref()).map(Cause::line)
    }

    fn may_resend(&self) -> bool {
        self.memo.may_resend(&id(ID), &self.fold())
    }

    fn resend(&mut self, settled: bool) -> Resend {
        let f = self.fold();
        self.memo.resend(&id(ID), &View { fold: &f, settled, queued: None, streaming: false }, self.now)
    }

    /// Pressed, and echoed as a steer while streaming: queued.
    fn queued(&mut self, driver: bool) -> String {
        let t = self.press(answer("Rewrite from scratch")).unwrap();
        self.on(Event::Echo { text: &t, driver });
        assert_eq!(self.state(), Some(State::Queued));
        t
    }
}

/// d1 and d2: echoed, then in the history; the entry stays, in the chat,
/// so the values survive a crash before the save.
#[test]
fn an_answer_echoed_then_in_the_history_is_in_the_chat() {
    let mut d = Desk::new();
    let t = d.press(answer("Rewrite from scratch")).unwrap();
    assert_eq!(t, text(ID, &answer("Rewrite from scratch")));
    assert_eq!(d.state(), Some(State::Sending));
    d.on(Event::Echo { text: &t, driver: false });
    assert_eq!(d.state(), Some(State::Queued));
    d.on(Event::Delivered { text: &t });
    assert_eq!(d.state(), Some(State::InChat), "SteeringDelivered");
    d.lands(&t);
    d.view(false, None, true);
    d.settles();
    assert_eq!((d.state(), d.may_resend()), (Some(State::InChat), false));
    // Changing the answer replaces the entry.
    let t2 = d.press(change("Leave it")).unwrap();
    assert_eq!((d.state(), d.memo.entries().len()), (Some(State::Sending), 1));
    assert_eq!(d.memo.get(&id(ID)).unwrap().seen, [t]);
    assert!(t2.starts_with(&format!("[card {ID} changed] ")));
}

/// One answer per card in flight; none to a card that can't take one.
#[test]
fn a_press_sends_nothing_while_an_answer_is_out() {
    let mut d = Desk::new();
    let t = d.press(answer("Leave it")).unwrap();
    assert_eq!(d.press(answer("Rewrite from scratch")), None, "sending");
    d.on(Event::Echo { text: &t, driver: false });
    assert_eq!(d.press(answer("Rewrite from scratch")), None, "queued");
    d.on(Event::Idle);
    assert_eq!((d.state(), d.press(answer("Rewrite from scratch"))), (Some(State::Checking), None), "checking");
    d.view(true, Some(&t), false);
    assert_eq!((d.state(), d.press(answer("Rewrite from scratch"))), (Some(State::Held), None), "held");

    let pending = fold::fold(&bound("t1", ID)[..1]);
    assert_eq!(Memo::default().press(&pending.cards()[0], answer("Leave it"), d.now), None, "pending");
    let mut closed = bound("t1", ID).to_vec();
    closed.push(F::Call { tool_id: "t2".into(), name: "cards_ui".into(), input: json!({ "close": ID }) });
    closed.push(F::Result { tool_id: "t2".into(), text: format!("Card {ID} closed (cards/1).") });
    let f = fold::fold(&closed);
    assert_eq!(Memo::default().press(f.bound(&id(ID)).unwrap(), answer("Leave it"), d.now), None, "closed");
}

/// d4: Cancel before the engine read it: `Dequeued`, then `Idle`, then
/// the history later. Unconfirmed only on the settled view.
#[test]
fn cancel_before_the_drain() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.on(Event::Dequeued { text: &t });
    assert_eq!(d.state(), Some(State::Checking));
    d.on(Event::Idle);
    d.view(false, None, false);
    assert_eq!((d.state(), d.may_resend()), (Some(State::Checking), false), "no settled view yet");
    d.settles();
    assert_eq!(d.state(), Some(State::Unconfirmed));
    assert_eq!(d.cause().as_deref(), Some("The agent was stopped before reading it"));
    assert!(d.may_resend());
    assert_eq!(d.resend(true), Resend::Send(t));
    assert_eq!(d.state(), Some(State::Sending));
}

/// d4's order: `Idle` comes before the refreshed history. No Resend until
/// a settled view, and the one that comes has it.
#[test]
fn idle_before_the_settled_view_offers_no_resend() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.on(Event::Idle);
    assert_eq!((d.state(), d.may_resend()), (Some(State::Checking), false));
    d.view(false, None, false);
    assert_eq!((d.state(), d.may_resend()), (Some(State::Checking), false), "the stale history lacks it");
    d.lands(&t);
    d.settles();
    assert_eq!((d.state(), d.may_resend()), (Some(State::InChat), false));
}

#[test]
fn a_reattach_with_and_without_the_answer() {
    let mut with = Desk::new();
    let t = with.queued(false);
    with.on(Event::Attached);
    assert_eq!(with.state(), Some(State::Checking));
    with.lands(&t);
    with.settles();
    assert_eq!(with.state(), Some(State::InChat));

    let mut without = Desk::new();
    without.queued(false);
    without.on(Event::Attached);
    without.settles();
    assert_eq!(without.state(), Some(State::Unconfirmed));
    assert_eq!(without.cause().as_deref(), Some("The connection dropped or the daemon restarted. It may not have arrived"));
    assert!(without.may_resend());

    // Sending at the drop settles after the reconnect too (§1.6).
    let mut sending = Desk::new();
    sending.press(answer("Leave it")).unwrap();
    sending.on(Event::Attached);
    assert_eq!(sending.state(), Some(State::Checking));
}

/// Only the digest's `queued_message` has it: rule 3.
#[test]
fn a_reattach_with_only_the_queued_message() {
    for (streaming, want) in [(true, State::Queued), (false, State::Held)] {
        let mut d = Desk::new();
        let t = d.queued(false);
        d.on(Event::Attached);
        d.view(true, Some(&t), streaming);
        assert_eq!((d.state(), d.may_resend()), (Some(want), false), "streaming {streaming}");
    }
}

/// d5 (the turn fails: the slot is kept, then `Idle`) and d6 (a failed
/// compaction keeps it too): held, with no Resend, until it goes in or a
/// Cancel withdraws it.
#[test]
fn held_after_an_error_and_after_a_failed_compaction() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.on(Event::Idle);
    d.view(true, Some(&t), false);
    assert_eq!((d.state(), d.may_resend()), (Some(State::Held), false));
    d.on(Event::Idle);
    d.on(Event::Notice);
    d.after(30);
    assert_eq!(d.state(), Some(State::Held), "held waits for the agent's next turn");
    d.lands(&t);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat), "it went in after the next turn");

    // d6: `Steered{delivered: false}` while compacting is no driver echo.
    let mut c = Desk::new();
    let t = c.press(answer("Leave it")).unwrap();
    c.on(Event::Echo { text: &t, driver: false });
    c.on(Event::Idle);
    c.view(true, Some(&t), false);
    assert_eq!(c.state(), Some(State::Held));
    c.on(Event::Dequeued { text: &t });
    c.settles();
    assert_eq!(c.cause().as_deref(), Some("The agent was stopped before reading it"));
}

/// d3: a later steer replaced ours in the slot, and the turn ended.
#[test]
fn a_slot_replaced_at_the_terminal_edge() {
    let mut d = Desk::new();
    d.queued(false);
    d.on(Event::Echo { text: "and also run the tests", driver: false });
    d.on(Event::Idle);
    d.view(true, Some("and also run the tests"), false);
    assert_eq!(d.state(), Some(State::Unconfirmed));
    assert_eq!(d.cause().as_deref(), Some("It didn't reach the chat. Another message may have replaced it"));
    assert!(d.may_resend());
}

/// d8: a full driver FIFO drops it, and says so only by a broadcast
/// notice; or nothing comes at all.
#[test]
fn fifo_overflow_by_notice_and_by_timeout() {
    let mut d = Desk::new();
    d.press(answer("Leave it")).unwrap();
    d.on(Event::Notice);
    assert_eq!(d.state(), Some(State::Checking));
    d.settles();
    assert_eq!(d.cause().as_deref(), Some("No reply from the daemon. A driver's queue may be full"));

    let mut t = Desk::new();
    t.press(answer("Leave it")).unwrap();
    t.after(9);
    assert_eq!(t.state(), Some(State::Sending));
    t.after(1);
    assert_eq!(t.state(), Some(State::Checking), "no echo in 10 s");
    t.settles();
    assert_eq!(t.cause().as_deref(), Some("No reply from the daemon. A driver's queue may be full"));

    // No settled view either: it can't tell.
    let mut u = Desk::new();
    u.press(answer("Leave it")).unwrap();
    u.after(10);
    u.view(false, None, false);
    u.after(9);
    assert_eq!(u.state(), Some(State::Checking));
    u.after(1);
    assert_eq!((u.state(), u.cause().as_deref()), (Some(State::Unconfirmed), Some("Couldn't check whether it arrived")));
    assert!(u.may_resend());
}

/// The notice names no input: it checks every entry sending, and nothing
/// else.
#[test]
fn the_notice_is_a_hint_not_a_receipt() {
    let mut d = Desk::new();
    d.history.extend(bound("t2", "c81kq0z"));
    let card2 = d.fold().bound(&id("c81kq0z")).cloned().unwrap();
    let t = d.queued(false);
    d.memo.press(&card2, answer("Leave it"), d.now).unwrap();
    d.on(Event::Notice);
    assert_eq!(d.state(), Some(State::Queued));
    assert_eq!(d.memo.get(&id("c81kq0z")).unwrap().state, State::Checking);
    d.on(Event::Delivered { text: &t });
    assert_eq!(d.state(), Some(State::InChat));
}

/// d9: the driver was revoked with ours in its FIFO.
#[test]
fn driver_revoked_listing_ours() {
    let mut d = Desk::new();
    let t = d.queued(true);
    d.on(Event::DriverRevoked { undelivered: &["someone else's".into()] });
    assert_eq!(d.state(), Some(State::Queued), "not listed");
    d.on(Event::DriverRevoked { undelivered: std::slice::from_ref(&t) });
    assert_eq!(d.state(), Some(State::Checking));
    d.settles();
    assert_eq!(d.cause().as_deref(), Some("The driver stopped before reading it"));
}

/// Rule 5: after a reattach the desk can't see the driver's FIFO, so an
/// answer last seen as a driver echo may still be in it.
#[test]
fn a_driver_echo_across_a_reattach() {
    let mut d = Desk::new();
    d.queued(true);
    d.on(Event::Attached);
    d.settles();
    assert_eq!(d.cause().as_deref(), Some("It may still be in the driver's queue"));

    // A revocation since the echo, even one not listing it, ends that.
    let mut r = Desk::new();
    r.queued(true);
    r.on(Event::DriverRevoked { undelivered: &[] });
    r.on(Event::Attached);
    r.settles();
    assert_eq!(r.cause().as_deref(), Some("The connection dropped or the daemon restarted. It may not have arrived"));
}

/// The daemon crashed after the answer was in the chat: a history saved
/// before it lacks it; one saved after keeps it.
#[test]
fn a_daemon_only_crash_after_the_answer_was_in_the_chat() {
    let mut d = Desk::new();
    let t = d.queued(true);
    d.lands(&t);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat));
    d.on(Event::Attached);
    assert_eq!(d.state(), Some(State::InChat));
    d.history.pop();
    d.view(false, None, false);
    assert_eq!(d.state(), Some(State::InChat), "a stale view proves nothing");
    d.settles();
    assert_eq!(d.state(), Some(State::Unconfirmed));
    assert_eq!(
        d.cause().as_deref(),
        Some("The connection dropped or the daemon restarted. It may not have arrived"),
        "not the driver's queue: it left it"
    );
    assert_eq!(d.resend(true), Resend::Send(t.clone()));

    let mut kept = Desk::new();
    let t = kept.queued(false);
    kept.lands(&t);
    kept.settles();
    kept.on(Event::Attached);
    kept.settles();
    assert_eq!((kept.state(), kept.may_resend()), (Some(State::InChat), false));
}

/// §3.5 rule 4's example: saved with no answers; A in the chat but never
/// saved; B sent with `seen = [A]`; the daemon crashed back to no answers.
/// A shorter F is still a prefix of `seen`: Resend.
#[test]
fn a_rollback_below_seen_offers_resend() {
    let mut d = Desk::new();
    let a = text(ID, &answer("Leave it"));
    d.lands(&a);
    let b = d.press(change("Rewrite from scratch")).unwrap();
    assert_eq!(d.memo.get(&id(ID)).unwrap().seen, [a]);
    d.on(Event::Echo { text: &b, driver: false });
    d.history.pop();
    d.on(Event::Attached);
    d.settles();
    assert_eq!(d.state(), Some(State::Unconfirmed));
    assert!(d.may_resend());
    assert_eq!(d.resend(true), Resend::Send(b.clone()));
    // B arrives: F is [B], ours past the (empty) common prefix.
    d.lands(&b);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat));
}

#[test]
fn our_first_sighting_keeps_the_entry_and_a_superseding_answer_drops_it() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.lands(&t);
    d.view(false, None, true);
    assert_eq!(d.state(), Some(State::InChat));
    d.lands(&text(ID, &change("Leave it")));
    d.view(false, None, true);
    assert_eq!(d.state(), None, "a later answer");

    // Ours absent, and an answer we never saw: superseded, settled or not.
    let mut e = Desk::new();
    e.queued(false);
    e.lands(&text(ID, &answer("Leave it")));
    e.view(false, None, true);
    assert_eq!(e.state(), None);

    // Unless ours is still the queued message (rule 3 before rule 4).
    let mut q = Desk::new();
    let t = q.queued(false);
    q.lands(&text(ID, &answer("Leave it")));
    q.view(true, Some(&t), true);
    assert_eq!(q.state(), Some(State::Queued));
    q.lands(&t);
    q.settles();
    assert_eq!(q.state(), Some(State::InChat), "it went in after the other");
}

#[test]
fn no_resend_once_the_first_copy_is_in_the_chat() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.on(Event::Dequeued { text: &t });
    d.settles();
    assert!(d.may_resend());
    d.lands(&t);
    d.view(false, None, false);
    assert_eq!((d.state(), d.may_resend(), d.resend(true)), (Some(State::InChat), false, Resend::No));
}

/// Two desks on one session: A from one, then B from the other, then both
/// reattach. Neither ever offers to send again.
#[test]
fn two_desks_a_then_b_then_reattach() {
    let (mut one, mut two) = (Desk::new(), Desk::new());
    let a = one.queued(false);
    for d in [&mut one, &mut two] {
        d.lands(&a);
        d.settles();
    }
    let b = two.press(change("Leave it")).unwrap();
    two.on(Event::Echo { text: &b, driver: false });
    for d in [&mut one, &mut two] {
        d.lands(&b);
        d.view(false, None, true);
    }
    assert_eq!((one.state(), two.state()), (None, Some(State::InChat)), "one's A is superseded");
    for d in [&mut one, &mut two] {
        d.on(Event::Attached);
        d.settles();
        assert!(!d.may_resend());
    }
    assert_eq!((one.state(), two.state()), (None, Some(State::InChat)));
}

#[test]
fn the_gate_is_checked_again_on_the_click() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.on(Event::Idle);
    d.settles();
    assert!(d.may_resend());
    assert_eq!(d.resend(false), Resend::Checking, "not settled");
    assert_eq!(d.state(), Some(State::Checking));
    d.settles();
    assert_eq!(
        (d.state(), d.cause().as_deref()),
        (Some(State::Unconfirmed), Some("It didn't reach the chat. Another message may have replaced it"))
    );
    // Another desk's answer arrived since the button was painted.
    d.lands(&text(ID, &answer("Leave it")));
    assert_eq!(d.resend(true), Resend::No);
    assert!(!d.memo.may_resend(&id(ID), &d.fold()));
    d.settles();
    assert_eq!(d.state(), None, "superseded");
    let _ = t;
}

/// Compaction or `/new` with ours queued: the card leaves the history.
#[test]
fn compaction_or_new_mid_queue() {
    let mut d = Desk::new();
    d.queued(false);
    d.history.clear();
    d.view(false, None, true);
    assert_eq!(d.state(), Some(State::Queued), "a stale view proves nothing");
    d.view(true, None, true);
    assert_eq!(d.state(), None, "gone");
}

#[test]
fn a_refusal_answers_the_oldest_sending_and_retry_sends_it_again() {
    let mut d = Desk::new();
    d.history.extend(bound("t2", "c81kq0z"));
    let t = d.press(answer("Leave it")).unwrap();
    d.after(1);
    let card2 = d.fold().bound(&id("c81kq0z")).cloned().unwrap();
    d.memo.press(&card2, answer("Leave it"), d.now).unwrap();
    d.on(Event::Refused { reason: "input owned by #2" });
    assert_eq!((d.state(), d.cause().as_deref()), (Some(State::NotSent), Some("Not sent: input owned by #2")));
    assert_eq!(d.memo.get(&id("c81kq0z")).unwrap().state, State::Sending);
    assert!(!d.may_resend(), "not sent is Retry, not Resend");
    let f = d.fold();
    assert_eq!(d.memo.retry(&id(ID), &f, d.now), Some(t));
    assert_eq!(d.state(), Some(State::Sending));
    assert_eq!(d.memo.retry(&id(ID), &f, d.now), None, "only from not sent");
}

/// §3.5's bound: 32 a session. A new entry evicts the oldest in the chat,
/// then the oldest unconfirmed, then the oldest.
#[test]
fn eviction_at_32() {
    let ids: Vec<String> = (0..CAP + 3).map(|i| format!("c{i:06}")).collect();
    let history: Vec<F> = ids.iter().enumerate().flat_map(|(i, c)| bound(&format!("t{i}"), c)).collect();
    let f = fold::fold(&history);
    let now = Instant::now();
    let mut memo = Memo::default();
    let press = |memo: &mut Memo, i: usize| memo.press(f.bound(&id(&ids[i])).unwrap(), answer("Leave it"), now).unwrap();
    for i in 0..CAP {
        press(&mut memo, i);
    }
    let unconfirmed = text(&ids[5], &answer("Leave it"));
    memo.on(Event::Refused { reason: "r" }, now);
    memo.on(Event::Echo { text: &unconfirmed, driver: false }, now);
    memo.on(Event::Dequeued { text: &unconfirmed }, now);
    for i in [9, 7] {
        memo.on(Event::Echo { text: &text(&ids[i], &answer("Leave it")), driver: false }, now);
        memo.on(Event::Delivered { text: &text(&ids[i], &answer("Leave it")) }, now);
    }
    // A settled history with the two in the chat: 5 settles unconfirmed.
    let mut landed = history.clone();
    landed.extend([7, 9].map(|i| F::User { text: text(&ids[i], &answer("Leave it")) }));
    memo.observe(&View { fold: &fold::fold(&landed), settled: true, queued: None, streaming: false }, now);
    let state = |memo: &Memo, i: usize| memo.get(&id(&ids[i])).map(|e| e.state);
    assert_eq!((state(&memo, 0), state(&memo, 5), state(&memo, 7)), (Some(State::NotSent), Some(State::Unconfirmed), Some(State::InChat)));
    press(&mut memo, CAP);
    assert_eq!((memo.entries().len(), state(&memo, 7), state(&memo, 9)), (CAP, None, Some(State::InChat)), "the oldest in the chat");
    press(&mut memo, CAP + 1);
    assert_eq!(state(&memo, 9), None);
    press(&mut memo, CAP + 2);
    assert_eq!((state(&memo, 5), state(&memo, 0)), (None, Some(State::NotSent)), "then the oldest unconfirmed");
    // Then the oldest of any state.
    let g = fold::fold(&[history, bound("tx", "czzzzzz").to_vec()].concat());
    memo.press(g.bound(&id("czzzzzz")).unwrap(), answer("Leave it"), now).unwrap();
    assert_eq!((memo.entries().len(), state(&memo, 0)), (CAP, None));
}
