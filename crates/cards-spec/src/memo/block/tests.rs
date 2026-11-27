//! `memo::block` (B4): S3's memo suite (`memo/tests.rs`) run again for
//! block edits, case by case, where §3.7 says the same as §3.5 and where it
//! differs (supersession by coverage retires, an ended incarnation carries
//! on); then §3.7's matrix; then B4's own cases: matching without notice
//! lines, a Resend's rebuilt trailer, two appends of different items, and
//! a memo full of recoverable entries.
//!
//! Every message this desk sends carries a trailer its desires built at
//! that send, a version newer each time; the daemon's events, the queued
//! slot and the history carry the whole message. So every case below also
//! matches by the edit's body, never by its notice lines.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;
use crate::block::message::{Notice, NoticeLine, Visibility};
use crate::block::{BlockId, Generation, Parts, parse_call};
use crate::board::Board;

const PLAN: &str = "plan";

fn r(id: &str) -> Ref {
    Ref::parse(id).unwrap()
}

fn generation() -> Generation {
    Generation::of(&json!({"role":"user","content":"start"}))
}

fn plan(steps: &[&str]) -> Value {
    json!({ "steps": steps.iter().map(|s| json!({"id": s, "label": format!("Do {s}")})).collect::<Vec<_>>() })
}

fn put_plan(id: &str, steps: &[&str]) -> Value {
    json!({"op":"put","id":id,"kind":"plan","title":"Close the refresh race","body":plan(steps)})
}

fn put_note(id: &str) -> Value {
    json!({"op":"put","id":id,"kind":"note","title":"Root cause","body":{"text":"First guess"}})
}

/// `set` one path.
fn set(path: &str, v: Value) -> Value {
    json!({ "set": { path: v } })
}

/// Skip step `step`: the edit most cases send.
fn skip(step: &str) -> Value {
    set(&format!("steps.{step}.state"), json!("skipped"))
}

/// The trailer a desk's desires build at a send (§3.5): a dismissal of
/// `tests`, at version `v`.
fn trailer(v: u32) -> Trailer {
    let line = NoticeLine::new(Visibility::Dismissed, vec![Notice { id: BlockId::parse("tests").unwrap(), version: v }]).unwrap();
    Trailer::new(vec![line]).unwrap()
}

fn summary() -> Summary<'static> {
    Summary { title: Some("Close the refresh race"), back_to: None, pieces: vec!["an edit"] }
}

/// One desk's view of one session: the history it holds, its block memo, a
/// clock, and its desires' version (each send builds its trailer afresh).
struct Desk {
    history: Vec<Event2>,
    memo: Memo,
    now: Instant,
    tools: usize,
    desires: u32,
    /// The body of the hand-back each case follows.
    sent: String,
}

type Event2 = crate::fold::Event;

impl Seen {
    /// The seen-prefix rule's common prefix of `f`, read as the memo reads
    /// a history ([`Reading::placed`]).
    fn agrees(&self, f: &[&str]) -> Option<usize> {
        let mut r = Reading::new(self, "", None);
        f.iter().for_each(|b| r.read(b));
        r.placed()
    }
}

impl Desk {
    /// A session with a plan of four steps and a note, put by the agent.
    fn new() -> Self {
        Desk::with(json!([put_plan(PLAN, &["fix", "test", "deploy", "bandaid"]), put_note("cause")]))
    }

    fn with(ops: Value) -> Self {
        let mut d = Desk { history: Vec::new(), memo: Memo::default(), now: Instant::now(), tools: 0, desires: 0, sent: String::new() };
        d.agent(ops);
        d
    }

    /// One bound agent call of `ops`.
    fn agent(&mut self, ops: Value) {
        self.tools += 1;
        let t = format!("toolu_{:02}", self.tools);
        let receipt = parse_call(&json!({ "ops": ops })).unwrap().receipt().to_string();
        self.history.push(Event2::Call { tool_id: t.clone(), name: "cards_block".into(), input: json!({ "ops": ops }) });
        self.history.push(Event2::Result { tool_id: t, text: receipt });
    }

    fn board(&self) -> board::State {
        Board::fold(generation(), &self.history).view(&self.history)
    }

    /// Block `id`'s live incarnation and revision.
    fn live(&self, id: &str) -> (Tag, u32) {
        let s = self.board();
        let b = s.block(&r(id)).unwrap();
        (b.tag, b.rev)
    }

    /// An edit of `id`'s live incarnation from its current revision.
    fn edit(&self, id: &str, parts: Value) -> Edit {
        let (tag, rev) = self.live(id);
        Edit::new(r(id), rev, tag, &parts).unwrap()
    }

    /// The body of `edit`'s message.
    fn body(edit: &Edit) -> String {
        message::build(edit, &summary()).unwrap()
    }

    /// The trailer this desk's desires build now.
    fn trailer(&self) -> Trailer {
        trailer(self.desires)
    }

    /// The whole message of the followed hand-back, sent now.
    fn whole(&self) -> String {
        format!("{}{}", self.sent, self.trailer().text())
    }

    /// Hand back `edit`: a new trailer, the whole message.
    fn send(&mut self, edit: &Edit) -> Result<String, Refusal> {
        self.desires += 1;
        let s = self.board();
        let h = History::new(&self.history, &s);
        let out = self.memo.hand_back(edit, &summary(), &trailer(self.desires), &h, self.now);
        if out.is_ok() {
            self.sent = Desk::body(edit);
        }
        out
    }

    /// Hand back `parts` on the plan.
    fn hand_back(&mut self, parts: Value) -> Result<String, Refusal> {
        let e = self.edit(PLAN, parts);
        self.send(&e)
    }

    /// Whether a hand-back of `parts` on the plan is admitted now (what
    /// enables Hand back).
    fn may_hand_back(&self, parts: Value) -> Result<(), Refusal> {
        let e = self.edit(PLAN, parts);
        let s = self.board();
        self.memo.may_hand_back(&e, &summary(), &History::new(&self.history, &s))
    }

    fn on(&mut self, e: Event<'_>) {
        self.memo.on(e, self.now);
    }

    /// The history gains this user message.
    fn lands(&mut self, text: &str) {
        self.history.push(Event2::User { text: text.into() });
    }

    fn view(&mut self, settled: bool, queued: Option<&str>, streaming: bool) {
        let s = self.board();
        let h = History::new(&self.history, &s);
        self.memo.observe(&View { edits: &h, settled, queued, streaming }, self.now);
    }

    fn settles(&mut self) {
        self.view(true, None, false);
    }

    fn after(&mut self, secs: u64) {
        self.now += Duration::from_secs(secs);
        self.memo.tick(self.now);
    }

    fn entry(&self) -> Option<&Entry> {
        self.memo.get(&self.sent)
    }

    fn state(&self) -> Option<State> {
        self.entry().map(|e| e.state)
    }

    fn retired(&self) -> Option<Retired> {
        self.entry().and_then(|e| e.retired)
    }

    fn cause(&self) -> Option<String> {
        self.entry().and_then(|e| e.cause.as_ref()).map(Cause::line)
    }

    fn may_resend(&self) -> bool {
        let s = self.board();
        let h = History::new(&self.history, &s);
        self.memo.may_resend(&self.sent, &h)
    }

    fn resend(&mut self, settled: bool) -> Resend {
        self.resend_view(settled, None, false)
    }

    /// A click against a settled view with this slot.
    fn resend_on(&mut self, queued: Option<&str>, streaming: bool) -> Resend {
        self.resend_view(true, queued, streaming)
    }

    fn resend_view(&mut self, settled: bool, queued: Option<&str>, streaming: bool) -> Resend {
        let s = self.board();
        let h = History::new(&self.history, &s);
        let t = trailer(self.desires);
        self.memo.resend(&self.sent, &t, &View { edits: &h, settled, queued, streaming }, self.now)
    }

    fn retry(&mut self) -> Option<String> {
        let s = self.board();
        let h = History::new(&self.history, &s);
        let t = trailer(self.desires);
        self.memo.retry(&self.sent, &t, &h, self.now)
    }

    /// Handed back, echoed, `Idle`, and a settled view without it:
    /// unconfirmed. The whole message.
    fn unconfirmed(&mut self) -> String {
        let t = self.queued(false);
        self.on(Event::Idle);
        self.settles();
        assert_eq!(self.state(), Some(State::Unconfirmed));
        t
    }

    /// Handed back (skip bandaid), and echoed as a steer while streaming:
    /// queued. The whole message.
    fn queued(&mut self, driver: bool) -> String {
        let t = self.hand_back(skip("bandaid")).unwrap();
        self.on(Event::Echo { text: &t, driver });
        assert_eq!(self.state(), Some(State::Queued));
        t
    }
}

/// Another desk's edit of the plan, from its current revision: the whole
/// message, its own line 1 and trailer.
fn theirs(d: &Desk, parts: Value) -> String {
    let line = Summary { title: None, back_to: None, pieces: vec!["their edit"] };
    format!("{}{}", message::build(&d.edit(PLAN, parts), &line).unwrap(), trailer(900).text())
}

// ── S3's suite, run again for block edits ─────────────────────────────

/// d1 and d2: echoed, then in the history; the entry stays, in the chat.
/// A later hand-back covering it retires it here (a card's change replaces
/// its entry); the new one has seen it.
#[test]
fn an_edit_echoed_then_in_the_history_is_in_the_chat() {
    let mut d = Desk::new();
    let t = d.hand_back(skip("bandaid")).unwrap();
    assert_eq!(t, d.whole());
    assert!(t.ends_with("\n[block tests v1 dismissed]"), "{t}");
    let first = d.sent.clone();
    assert_eq!(d.state(), Some(State::Sending));
    d.on(Event::Echo { text: &t, driver: false });
    assert_eq!(d.state(), Some(State::Queued));
    d.on(Event::Delivered { text: &t });
    assert_eq!(d.state(), Some(State::InChat), "SteeringDelivered");
    d.lands(&t);
    d.view(false, None, true);
    d.settles();
    assert_eq!((d.state(), d.may_resend()), (Some(State::InChat), false));
    // Unskipping it covers it: retired here, the new entry sending.
    let t2 = d.hand_back(set("steps.bandaid.state", json!("todo"))).unwrap();
    assert_eq!(d.memo.get(&first).map(|e| (e.state, e.retired)), Some((State::InChat, Some(Retired::Here))));
    assert_eq!((d.state(), d.memo.entries().len()), (Some(State::Sending), 2));
    assert_eq!(d.entry().unwrap().seen.n, 1, "it has seen the first");
    assert!(t2.starts_with("[block plan edited r2] "), "{t2}");
    // Retired is settled for good: the covering edit landing, then a
    // rollback of both, move nothing of it.
    let here = Some((State::InChat, Some(Retired::Here)));
    d.lands(&t2);
    d.settles();
    assert_eq!(d.memo.get(&first).map(|e| (e.state, e.retired)), here, "still this desk's");
    d.history.truncate(d.history.len() - 2);
    d.on(Event::Attached);
    d.settles();
    d.after(30);
    assert_eq!(d.memo.get(&first).map(|e| (e.state, e.retired)), here, "no view, event or clock moves it");
}

/// One edit in flight per incarnation; none to an incarnation that isn't
/// live.
#[test]
fn a_hand_back_sends_nothing_while_an_edit_is_out() {
    let mut d = Desk::new();
    let t = d.hand_back(skip("bandaid")).unwrap();
    let other = || skip("deploy");
    assert_eq!(d.hand_back(other()), Err(Refusal::InFlight), "sending");
    d.on(Event::Echo { text: &t, driver: false });
    assert_eq!(d.hand_back(other()), Err(Refusal::InFlight), "queued");
    d.on(Event::Idle);
    assert_eq!((d.state(), d.hand_back(other())), (Some(State::Checking), Err(Refusal::InFlight)), "checking");
    d.view(true, Some(&t), false);
    assert_eq!((d.state(), d.hand_back(other())), (Some(State::Held), Err(Refusal::InFlight)), "held");
    assert!(d.memo.in_flight(&r(PLAN), &d.live(PLAN).0).is_some(), "Sending… on the block");
    // Another block takes one.
    let cause = d.edit("cause", set("text", json!("The retry loop")));
    assert!(d.send(&cause).is_ok(), "another block");
    assert_eq!(d.memo.entries().len(), 2);

    // An incarnation the agent replaced, or removed: gone.
    let mut g = Desk::new();
    let old = g.edit(PLAN, skip("bandaid"));
    g.agent(json!([{"op":"replace","id":PLAN,"kind":"plan","body":plan(&["fix"])}]));
    assert_eq!(g.send(&old), Err(Refusal::Gone), "replaced");
    let mut h = Desk::new();
    let note = h.edit("cause", set("text", json!("x")));
    h.agent(json!([{"op":"remove","id":"cause"}]));
    assert_eq!(h.send(&note), Err(Refusal::Gone), "removed");
    assert!(g.memo.entries().is_empty() && h.memo.entries().is_empty(), "nothing kept");
}

/// d4: Cancel before the engine read it: `Dequeued`, then `Idle`, then the
/// history later. Unconfirmed only on the settled view.
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
    assert_eq!(d.resend(true), Resend::Send(d.whole()));
    assert_eq!(d.state(), Some(State::Sending));
}

/// d4's order: `Idle` comes before the refreshed history.
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
fn a_reattach_with_and_without_the_edit() {
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

    let mut sending = Desk::new();
    sending.hand_back(skip("bandaid")).unwrap();
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

/// d5 and d6: held, with no Resend, until it goes in or a Cancel withdraws
/// it.
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

    let mut c = Desk::new();
    let t = c.hand_back(skip("bandaid")).unwrap();
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

/// d8: a full driver FIFO, by notice or by timeout; or no settled view.
#[test]
fn fifo_overflow_by_notice_and_by_timeout() {
    let mut d = Desk::new();
    d.hand_back(skip("bandaid")).unwrap();
    d.on(Event::Notice);
    assert_eq!(d.state(), Some(State::Checking));
    d.settles();
    assert_eq!(d.cause().as_deref(), Some("No reply from the daemon. A driver's queue may be full"));

    let mut t = Desk::new();
    t.hand_back(skip("bandaid")).unwrap();
    t.after(9);
    assert_eq!(t.state(), Some(State::Sending));
    t.after(1);
    assert_eq!(t.state(), Some(State::Checking), "no echo in 10 s");
    t.settles();
    assert_eq!(t.cause().as_deref(), Some("No reply from the daemon. A driver's queue may be full"));

    let mut u = Desk::new();
    u.hand_back(skip("bandaid")).unwrap();
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
    let t = d.queued(false);
    let plan_body = d.sent.clone();
    let cause = d.edit("cause", set("text", json!("The retry loop")));
    d.send(&cause).unwrap();
    d.on(Event::Notice);
    assert_eq!(d.memo.get(&plan_body).unwrap().state, State::Queued);
    assert_eq!(d.state(), Some(State::Checking), "the note's, sending");
    d.on(Event::Delivered { text: &t });
    assert_eq!(d.memo.get(&plan_body).unwrap().state, State::InChat);
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

/// Rule 5 across a reattach, and a revocation since the echo.
#[test]
fn a_driver_echo_across_a_reattach() {
    let mut d = Desk::new();
    d.queued(true);
    d.on(Event::Attached);
    d.settles();
    assert_eq!(d.cause().as_deref(), Some("It may still be in the driver's queue"));

    let mut r = Desk::new();
    r.queued(true);
    r.on(Event::DriverRevoked { undelivered: &[] });
    r.on(Event::Attached);
    r.settles();
    assert_eq!(r.cause().as_deref(), Some("The connection dropped or the daemon restarted. It may not have arrived"));
}

/// The daemon crashed after the edit was in the chat: a history saved
/// before it lacks it; one saved after keeps it.
#[test]
fn a_daemon_only_crash_after_the_edit_was_in_the_chat() {
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
    assert_eq!(d.resend(true), Resend::Send(d.whole()));

    let mut kept = Desk::new();
    let t = kept.queued(false);
    kept.lands(&t);
    kept.settles();
    kept.on(Event::Attached);
    kept.settles();
    assert_eq!((kept.state(), kept.may_resend()), (Some(State::InChat), false));
}

/// §3.5 rule 4's example, for an edit: A (another desk's) in the chat but
/// never saved; B sent having seen A; the daemon crashed back to before A.
/// A shorter history is still a prefix of what B saw: Resend, the same
/// body; then B lands, past the (empty) common prefix.
#[test]
fn a_rollback_below_seen_offers_resend() {
    let mut d = Desk::new();
    let a = theirs(&d, set("steps.fix.label", json!("Fix the race")));
    d.lands(&a);
    let b = d.hand_back(skip("bandaid")).unwrap();
    assert_eq!(d.entry().unwrap().seen.n, 1);
    d.on(Event::Echo { text: &b, driver: false });
    d.history.pop();
    d.on(Event::Attached);
    d.settles();
    assert_eq!(d.state(), Some(State::Unconfirmed));
    assert!(d.may_resend());
    assert_eq!(d.resend(true), Resend::Send(d.whole()));
    assert!(d.whole().starts_with("[block plan edited r2] "), "the base as sent, not the board's r1 now");
    d.lands(&b);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat));
}

/// Ours seen keeps the entry; a later covering edit retires it (a card's
/// later answer drops its entry). Ours absent and a covering edit we never
/// saw: retired, settled or not. Unless ours is still the queued message
/// (rule 3 before rule 4).
#[test]
fn our_first_sighting_keeps_the_entry_and_a_covering_edit_retires_it() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.lands(&t);
    d.view(false, None, true);
    assert_eq!(d.state(), Some(State::InChat));
    let covering = theirs(&d, set("steps.bandaid.state", json!("todo")));
    d.lands(&covering);
    d.view(false, None, true);
    assert_eq!((d.state(), d.retired()), (Some(State::InChat), Some(Retired::Elsewhere)), "a later covering edit");

    let mut e = Desk::new();
    e.queued(false);
    let covering = theirs(&e, skip("bandaid"));
    e.lands(&covering);
    e.view(false, None, true);
    assert_eq!((e.state(), e.retired()), (Some(State::Queued), Some(Retired::Elsewhere)));
    assert!(!e.may_resend());
    assert!(e.memo.in_flight(&r(PLAN), &e.live(PLAN).0).is_none(), "retired: not in flight");
    assert!(e.hand_back(skip("deploy")).is_ok(), "the block takes a hand-back");

    let mut q = Desk::new();
    let t = q.queued(false);
    let covering = theirs(&q, skip("bandaid"));
    q.lands(&covering);
    q.view(true, Some(&t), true);
    assert_eq!((q.state(), q.retired()), (Some(State::Queued), None));
    q.lands(&t);
    q.settles();
    assert_eq!((q.state(), q.retired()), (Some(State::InChat), None), "it went in after the other");
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

/// Two desks on one session: A from one, then B covering it from the
/// other, then both reattach. Neither offers to send again; one's A is
/// retired (a card's would be dropped).
#[test]
fn two_desks_a_then_b_then_reattach() {
    let (mut one, mut two) = (Desk::new(), Desk::new());
    let a = one.queued(false);
    for d in [&mut one, &mut two] {
        d.lands(&a);
        d.settles();
    }
    let b = two.hand_back(set("steps.bandaid.state", json!("todo"))).unwrap();
    two.on(Event::Echo { text: &b, driver: false });
    for d in [&mut one, &mut two] {
        d.lands(&b);
        d.view(false, None, true);
    }
    assert_eq!((one.retired(), two.state(), two.retired()), (Some(Retired::Elsewhere), Some(State::InChat), None));
    for d in [&mut one, &mut two] {
        d.on(Event::Attached);
        d.settles();
        assert!(!d.may_resend());
    }
    assert_eq!((one.state(), one.retired(), two.state()), (Some(State::InChat), Some(Retired::Elsewhere), Some(State::InChat)));
}

/// The gate is checked again on the click; another desk's covering edit
/// closes it on the history alone, before any view.
#[test]
fn the_gate_is_checked_again_on_the_click() {
    let mut d = Desk::new();
    d.queued(false);
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
    let covering = theirs(&d, skip("bandaid"));
    d.lands(&covering);
    assert!(!d.may_resend(), "the gate, on the history alone");
    assert_eq!(d.state(), Some(State::Unconfirmed));
    assert_eq!(d.resend(true), Resend::No);
    assert_eq!(d.retired(), Some(Retired::Elsewhere), "covered");
}

/// Rule 4: the check finds a covering edit we never saw, not ours:
/// retired on that view, before any cause shows.
#[test]
fn a_check_that_finds_a_covering_edit_we_never_saw_retires_it() {
    for trigger in [Event::Idle, Event::Attached] {
        let mut d = Desk::new();
        d.queued(false);
        d.on(trigger);
        assert_eq!(d.state(), Some(State::Checking));
        let covering = theirs(&d, skip("bandaid"));
        d.lands(&covering);
        d.settles();
        assert_eq!((d.retired(), d.cause(), d.may_resend()), (Some(Retired::Elsewhere), None, false), "{trigger:?}");
        // Retired is settled for good: nothing moves it.
        d.on(Event::Attached);
        d.after(60);
        assert_eq!((d.state(), d.retired()), (Some(State::Checking), Some(Retired::Elsewhere)), "{trigger:?}");
    }
}

/// Rule 3 for an unconfirmed entry: the check timed out, then a settled
/// view shows ours in the slot. No Resend, and the click sends nothing.
#[test]
fn a_settled_view_with_ours_queued_after_the_check_timed_out() {
    for (streaming, want) in [(false, State::Held), (true, State::Queued)] {
        let mut d = Desk::new();
        let t = d.hand_back(skip("bandaid")).unwrap();
        d.after(10);
        d.after(10);
        assert_eq!((d.state(), d.cause().as_deref()), (Some(State::Unconfirmed), Some("Couldn't check whether it arrived")));
        assert!(d.may_resend());
        d.view(true, Some(&t), streaming);
        assert_eq!((d.state(), d.may_resend()), (Some(want), false), "streaming {streaming}");
        assert_eq!(d.resend_on(Some(&t), streaming), Resend::No);
        assert_eq!(d.hand_back(skip("deploy")), Err(Refusal::InFlight), "still in flight");
        d.lands(&t);
        d.settles();
        assert_eq!(d.state(), Some(State::InChat), "it went in");
    }
}

/// The click runs the settle rules on its own settled view: ours in the
/// slot sends nothing; ours in the chat with a covering edit after it is
/// retired. The block removed (a card closed is rule 1) keeps the entry,
/// with no Resend.
#[test]
fn a_resend_click_on_a_view_with_ours_queued_sends_nothing() {
    for (streaming, want) in [(true, State::Queued), (false, State::Held)] {
        let mut d = Desk::new();
        let t = d.unconfirmed();
        assert!(d.may_resend(), "painted");
        assert_eq!(d.resend_on(Some(&t), streaming), Resend::No, "streaming {streaming}");
        assert_eq!((d.state(), d.may_resend()), (Some(want), false));
    }

    let mut d = Desk::new();
    let t = d.unconfirmed();
    d.lands(&t);
    let covering = theirs(&d, set("steps.bandaid.state", json!("todo")));
    d.lands(&covering);
    assert_eq!(
        (d.resend_on(Some(&t), false), d.state(), d.retired()),
        (Resend::No, Some(State::InChat), Some(Retired::Elsewhere)),
        "rule 2"
    );

    let mut d = Desk::new();
    d.unconfirmed();
    d.agent(json!([{"op":"remove","id":PLAN}]));
    assert_eq!((d.resend_on(None, false), d.state()), (Resend::No, Some(State::Unconfirmed)), "the block gone: kept, no Resend");
}

/// d5, then d3 in the next turn: held after an error; another steer
/// replaces the slot during the next turn; the turn ends.
#[test]
fn held_then_the_slot_replaced_in_the_next_turn() {
    const OTHER: &str = "and also run the tests";
    for failed in [false, true] {
        let mut d = Desk::new();
        let t = d.queued(false);
        d.on(Event::Idle);
        d.view(true, Some(&t), false);
        assert_eq!(d.state(), Some(State::Held));
        d.on(Event::Echo { text: "next", driver: false });
        d.lands("next");
        d.view(true, Some(&t), true);
        d.on(Event::Echo { text: OTHER, driver: false });
        d.view(true, Some(OTHER), true);
        assert_eq!(d.state(), Some(State::Held), "the turn runs: nothing settles before its Idle");
        if !failed {
            d.lands(OTHER);
        }
        d.on(Event::Idle);
        d.view(true, failed.then_some(OTHER), false);
        assert_eq!(
            (d.state(), d.cause().as_deref()),
            (Some(State::Unconfirmed), Some("It didn't reach the chat. Another message may have replaced it")),
            "failed {failed}"
        );
        assert_eq!(d.resend(true), Resend::Send(d.whole()));
    }
}

/// Held survives an `Idle` whose settled view still shows it, and a view
/// without it and no `Idle`; then it goes in.
#[test]
fn held_survives_an_idle_that_still_shows_it_and_lands_later() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.on(Event::Idle);
    d.view(true, Some(&t), false);
    d.on(Event::Idle);
    d.view(false, None, false);
    assert_eq!(d.state(), Some(State::Held), "a stale view proves nothing");
    d.view(true, Some(&t), false);
    d.after(30);
    assert_eq!((d.state(), d.may_resend()), (Some(State::Held), false), "the next turn failed too: still held");
    d.view(true, None, false);
    assert_eq!(d.state(), Some(State::Held), "no Idle: the slot was taken to go in");
    d.lands(&t);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat));
}

/// The incarnation ended (where a card's id bound again drops its entry):
/// the agent replaced the block. The entry carries on, with no Resend or
/// Retry; the new incarnation is another block and takes a hand-back; ours
/// landing after the replace is in the chat (the board says it applied
/// nothing).
#[test]
fn a_block_replaced_keeps_the_entry_with_no_resend() {
    let replace = || json!([{"op":"replace","id":PLAN,"kind":"plan","title":"Again","body":plan(&["fix","test","deploy","bandaid"])}]);

    let mut q = Desk::new();
    let t = q.queued(false);
    q.agent(replace());
    q.settles();
    assert_eq!(q.state(), Some(State::Queued), "queued: carries on");
    let body = q.sent.clone();
    assert!(q.hand_back(skip("bandaid")).is_ok(), "the new incarnation takes one");
    q.lands(&t);
    q.settles();
    assert_eq!(q.memo.get(&body).map(|e| e.state), Some(State::InChat), "delivered after the replace");
    let s = q.board();
    let outcome = s.outcomes().last().unwrap();
    assert_eq!(outcome.result, board::EditResult::Rewritten, "the board: not applied");
    // The new incarnation's entry: an edit of the old one, landing late,
    // covers every key of it, and isn't of its incarnation.
    let (old_tag, _) = Desk::new().live(PLAN);
    let late = Edit::new(r(PLAN), 1, old_tag, &json!({"set": {"steps.bandaid.state": "skipped", "steps.fix.label": "Old"}})).unwrap();
    let late = message::build(&late, &Summary { title: None, back_to: None, pieces: vec!["late"] }).unwrap();
    q.lands(&late);
    q.on(Event::Idle);
    q.settles();
    let now = q.memo.entries().last().unwrap();
    assert_eq!((now.retired, now.of), (None, q.live(PLAN).0), "another incarnation's edit covers nothing of this one");

    let mut u = Desk::new();
    u.unconfirmed();
    assert!(u.may_resend());
    u.agent(replace());
    assert!(!u.may_resend(), "unconfirmed: no Resend");
    assert_eq!((u.resend(true), u.state()), (Resend::No, Some(State::Unconfirmed)));

    let mut n = Desk::new();
    n.hand_back(skip("bandaid")).unwrap();
    n.on(Event::Refused { reason: "r" });
    n.agent(replace());
    assert_eq!((n.retry(), n.state()), (None, Some(State::NotSent)), "not sent: no Retry");
}

/// Compaction or `/new` with ours queued: its block leaves the history.
/// The entry carries on (a card's is dropped), with no Resend.
#[test]
fn compaction_or_new_mid_queue() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.history.clear();
    d.view(false, None, true);
    assert_eq!(d.state(), Some(State::Queued), "a stale view proves nothing");
    d.view(true, None, true);
    assert_eq!(d.state(), Some(State::Queued), "carries on");
    d.on(Event::Idle);
    d.settles();
    assert_eq!((d.state(), d.may_resend()), (Some(State::Unconfirmed), false), "lost, and nothing to resend to");
    let _ = t;
}

#[test]
fn a_refusal_answers_the_oldest_sending_and_retry_sends_it_again() {
    let mut d = Desk::new();
    let t = d.hand_back(skip("bandaid")).unwrap();
    let plan_body = d.sent.clone();
    d.after(1);
    let cause = d.edit("cause", set("text", json!("The retry loop")));
    d.send(&cause).unwrap();
    let cause_body = d.sent.clone();
    assert_eq!(d.memo.oldest_sending(), Some(d.now - Duration::from_secs(1)));
    d.on(Event::Refused { reason: "input owned by #2" });
    d.sent = plan_body;
    assert_eq!((d.state(), d.cause().as_deref()), (Some(State::NotSent), Some("Not sent: input owned by #2")));
    assert_eq!(d.memo.get(&cause_body).unwrap().state, State::Sending);
    assert!(!d.may_resend(), "not sent is Retry, not Resend");
    let again = d.retry().unwrap();
    assert_eq!(message::split(&again).0, message::split(&t).0, "the same edit");
    assert_eq!(again, d.whole(), "with the trailer built now");
    assert_eq!(d.state(), Some(State::Sending));
    assert_eq!(d.retry(), None, "only from not sent");
}

/// Seven entries, one in each state, oldest first: unconfirmed, held, not
/// sent, in the chat, queued, checking, sending. Seven notes, so none
/// waits on another's flight.
fn seven() -> (Vec<String>, Desk) {
    let notes: Vec<Value> = (0..7).map(|i| put_note(&format!("n{i}"))).collect();
    let mut d = Desk::with(Value::Array(notes));
    let mut bodies = Vec::new();
    for i in 0..7 {
        let e = d.edit(&format!("n{i}"), set("text", json!(format!("Note {i}"))));
        let t = d.send(&e).unwrap();
        bodies.push(d.sent.clone());
        match i {
            0 => {
                d.on(Event::Echo { text: &t, driver: false });
                d.on(Event::Idle);
                d.settles();
            }
            1 => {
                d.on(Event::Echo { text: &t, driver: false });
                d.on(Event::Idle);
                d.view(true, Some(&t), false);
            }
            2 => d.on(Event::Refused { reason: "r" }),
            3 => {
                d.on(Event::Echo { text: &t, driver: false });
                d.on(Event::Delivered { text: &t });
            }
            4 => d.on(Event::Echo { text: &t, driver: false }),
            5 => d.after(10),
            _ => {}
        }
    }
    let states: Vec<State> = d.memo.entries().iter().map(|e| e.state).collect();
    use State::*;
    assert_eq!(states, [Unconfirmed, Held, NotSent, InChat, Queued, Checking, Sending]);
    (bodies, d)
}

/// Discard: that entry goes, whatever its state, and every other stays
/// exactly as it was, in its order. Again, or with no entry: nothing.
#[test]
fn forget_drops_one_entry_and_leaves_every_other_as_it_was() {
    let (bodies, d) = seven();
    for (i, body) in bodies.iter().enumerate() {
        let mut m = d.memo.clone();
        assert_eq!(m.forget(body).as_ref(), Some(&d.memo.entries()[i]), "{i}: its entry");
        let others: Vec<&Entry> = d.memo.entries().iter().filter(|e| e.text != *body).collect();
        assert_eq!(m.entries().iter().collect::<Vec<_>>(), others, "{i}: every other entry as it was");
        assert_eq!(m.forget(body), None, "{i}: once");
        assert_eq!(m.entries().len(), 6);
    }
    let mut m = d.memo.clone();
    assert_eq!(m.forget("[block plan edited r1] x\n{}"), None);
    assert_eq!(m.entries(), d.memo.entries(), "no entry: nothing moves");
}

/// Discarded, the row shows what the board says, no Resend or Retry, and
/// later views bring nothing back; a hand-back is a new entry that sees
/// the history's edits.
#[test]
fn a_discarded_edit_shows_the_board_and_a_hand_back_starts_afresh() {
    let mut d = Desk::new();
    let t = d.unconfirmed();
    let gone = d.memo.forget(&t).unwrap();
    assert_eq!(gone.text, d.sent, "found by the whole message too");
    assert_eq!((d.state(), d.may_resend(), d.resend(true), d.retry()), (None, false, Resend::No, None));
    d.settles();
    assert_eq!(d.state(), None, "a later view brings nothing back");
    let again = d.hand_back(skip("bandaid")).unwrap();
    assert_eq!(message::split(&again).0, gone.text, "the same edit");
    assert_eq!((d.state(), d.entry().unwrap().seen.n), (Some(State::Sending), 0));
}

// ── §3.7's matrix (B4) ─────────────────────────────────────────────────

/// The queued slot displaced: `Dequeued` names the whole message; Resend
/// sends the same body.
#[test]
fn matrix_the_queued_slot_displaced() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.on(Event::Dequeued { text: &t });
    d.settles();
    assert_eq!((d.state(), d.cause().as_deref()), (Some(State::Unconfirmed), Some("The agent was stopped before reading it")));
    assert_eq!(d.resend(true), Resend::Send(d.whole()));
}

/// A history rollback: in the chat, then a settled history without ours.
#[test]
fn matrix_a_history_rollback() {
    let mut d = Desk::new();
    let t = d.queued(false);
    d.lands(&t);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat));
    d.history.pop();
    d.view(false, None, false);
    assert_eq!(d.state(), Some(State::InChat), "a stale view proves nothing");
    d.settles();
    assert_eq!(
        (d.state(), d.cause().as_deref()),
        (Some(State::Unconfirmed), Some("The connection dropped or the daemon restarted. It may not have arrived")),
        "a settled history without it"
    );
    assert_eq!(d.resend(true), Resend::Send(d.whole()));
}

/// `DriverRevoked` lists ours, whole.
#[test]
fn matrix_driver_revoked() {
    let mut d = Desk::new();
    let t = d.queued(true);
    let other_trailer = format!("{}{}", d.sent, trailer(77).text());
    d.on(Event::DriverRevoked { undelivered: std::slice::from_ref(&other_trailer) });
    assert_eq!(d.state(), Some(State::Checking), "its body, whatever trailer the list holds");
    d.settles();
    assert_eq!(d.cause().as_deref(), Some("The driver stopped before reading it"));
    let _ = t;
}

/// An agent tick while sending: the block moves to r3, the binding stays
/// (r1); the edit lands and applies after the tick; lost, Resend sends the
/// same bytes, from r1.
#[test]
fn matrix_an_agent_tick_while_sending() {
    let mut d = Desk::new();
    let t = d.hand_back(skip("bandaid")).unwrap();
    let (tag, base) = (d.entry().unwrap().of, d.entry().unwrap().base);
    d.agent(json!([{"op":"patch","id":PLAN,"set":{"steps.fix.state":"doing"}}]));
    d.agent(json!([{"op":"patch","id":PLAN,"set":{"steps.fix.state":"done"}}]));
    assert_eq!(d.live(PLAN), (tag, 3), "ticked");
    d.on(Event::Echo { text: &t, driver: false });
    d.lands(&t);
    d.settles();
    let e = d.entry().unwrap();
    assert_eq!((e.state, e.base, e.of), (State::InChat, base, tag), "the binding as sent");
    let s = d.board();
    assert_eq!(s.outcomes().last().unwrap().result, board::EditResult::Applied, "the board applies it after the ticks");

    let mut l = Desk::new();
    let t = l.queued(false);
    l.agent(json!([{"op":"patch","id":PLAN,"set":{"steps.fix.state":"doing"}}]));
    l.on(Event::Idle);
    l.settles();
    assert_eq!(l.entry().unwrap().base, 1);
    assert_eq!(l.resend(true), Resend::Send(t.clone()), "the same bytes: r1, not r2");
    assert!(t.starts_with("[block plan edited r1] "));
}

/// Another desk's disjoint edit leaves ours, with its Resend; its covering
/// edit retires it, and says it was another's.
#[test]
fn matrix_another_desks_disjoint_edit_then_its_covering_edit() {
    let mut d = Desk::new();
    d.unconfirmed();
    let disjoint = theirs(&d, set("steps.test.label", json!("Run the tests")));
    d.lands(&disjoint);
    d.settles();
    assert_eq!((d.state(), d.retired(), d.may_resend()), (Some(State::Unconfirmed), None, true), "disjoint");
    // Partial: it names ours and more, but ours names a key it doesn't.
    let mut p = Desk::new();
    p.hand_back(json!({"set": {"steps.bandaid.state": "skipped", "steps.deploy.stop": true}})).unwrap();
    let t = p.whole();
    p.on(Event::Echo { text: &t, driver: false });
    p.on(Event::Idle);
    p.settles();
    let partial = theirs(&p, skip("bandaid"));
    p.lands(&partial);
    p.settles();
    assert_eq!((p.retired(), p.may_resend()), (None, true), "partial");
    // Covering: every key of ours.
    let covering = theirs(&d, json!({"set": {"steps.bandaid.state": "skipped", "steps.deploy.stop": true}}));
    d.lands(&covering);
    d.settles();
    assert_eq!((d.retired(), d.may_resend(), d.resend(true)), (Some(Retired::Elsewhere), false, Resend::No), "covering");
}

/// Discard: the entry goes, and frees its slot (see the full memo below).
#[test]
fn matrix_discard() {
    let mut d = Desk::new();
    let t = d.unconfirmed();
    assert!(d.memo.forget(&d.sent.clone()).is_some());
    assert_eq!((d.state(), d.may_resend()), (None, false));
    d.lands(&t);
    d.settles();
    assert_eq!(d.state(), None, "its message landing later brings nothing back");
}

/// Compaction while queued: the queued message moves to the successor,
/// where its id names no block. The memo follows its delivery as usual:
/// held in the slot, then in the chat (`delivered · its block was
/// compacted away`); no Resend at any point.
#[test]
fn matrix_compaction_while_queued() {
    let mut d = Desk::new();
    let t = d.queued(false);
    // The successor: a summary, no block.
    d.history = vec![Event2::User { text: "<summary>…</summary>".into() }];
    d.view(true, Some(&t), true);
    assert_eq!((d.state(), d.may_resend()), (Some(State::Queued), false), "in the successor's slot");
    d.on(Event::Idle);
    d.view(true, Some(&t), false);
    assert_eq!((d.state(), d.may_resend()), (Some(State::Held), false));
    d.lands(&t);
    d.settles();
    assert_eq!((d.state(), d.retired(), d.may_resend()), (Some(State::InChat), None, false), "delivered");
    assert!(d.board().block(&r(PLAN)).is_none(), "its block compacted away");
}

/// A reattach mid-send: sending at the drop, checked after the reconnect;
/// the history has it, or it doesn't and Resend sends the same body.
#[test]
fn matrix_a_reattach_mid_send() {
    let mut with = Desk::new();
    let t = with.hand_back(skip("bandaid")).unwrap();
    with.on(Event::Attached);
    assert_eq!(with.state(), Some(State::Checking));
    with.lands(&t);
    with.settles();
    assert_eq!(with.state(), Some(State::InChat));

    let mut without = Desk::new();
    without.hand_back(skip("bandaid")).unwrap();
    without.on(Event::Attached);
    without.settles();
    assert_eq!(
        (without.state(), without.cause().as_deref()),
        (Some(State::Unconfirmed), Some("The connection dropped or the daemon restarted. It may not have arrived"))
    );
    assert_eq!(without.resend(true), Resend::Send(without.whole()));
}

/// Two identical texts (the seen-prefix rule, as for card answers): the
/// first copy applied nothing (the step had run), and the same edit is
/// handed back again; or another desk sends the same bytes. A copy this
/// desk had seen never confirms ours; one past it does.
#[test]
fn matrix_two_identical_texts() {
    let mut d = Desk::new();
    d.agent(json!([{"op":"patch","id":PLAN,"set":{"steps.bandaid.state":"done"}}]));
    let first = d.hand_back(skip("bandaid")).unwrap();
    d.on(Event::Echo { text: &first, driver: false });
    d.lands(&first);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat));
    assert_eq!(d.board().block(&r(PLAN)).unwrap().rev, 2, "a conflict: no change, the same base");
    let second = d.hand_back(skip("bandaid")).unwrap();
    assert_eq!(message::split(&second).0, message::split(&first).0, "byte for byte the same edit");
    assert_eq!((d.memo.entries().len(), d.entry().unwrap().seen.n), (1, 1), "it replaced its entry, having seen the first");
    d.on(Event::Echo { text: &second, driver: false });
    d.on(Event::Idle);
    d.settles();
    assert_eq!(d.state(), Some(State::Unconfirmed), "the first copy, seen, doesn't confirm it");
    assert!(d.may_resend());
    // Another desk's copy past it does.
    let copy = format!("{}{}", d.sent, trailer(900).text());
    d.lands(&copy);
    d.settles();
    assert_eq!((d.state(), d.may_resend()), (Some(State::InChat), false));
}

// ── B4's own cases ─────────────────────────────────────────────────────

/// Matching without notice lines: every event, the slot and the history
/// carry the body with whatever trailer, or none, and each matches; a
/// body with a line that only looks like a notice doesn't.
#[test]
fn matching_without_notice_lines() {
    let mut d = Desk::new();
    let t = d.hand_back(skip("bandaid")).unwrap();
    let body = d.sent.clone();
    let shown = NoticeLine::new(Visibility::Shown, vec![Notice { id: BlockId::parse("plan").unwrap(), version: 4 }]).unwrap();
    let two = Trailer::new(vec![trailer(5).lines[0].clone(), shown]).unwrap();
    let others = [body.clone(), format!("{body}{}", trailer(42).text()), format!("{body}{}", two.text())];
    for (i, o) in others.iter().enumerate() {
        assert!(d.memo.get(o).is_some(), "{i}: found by it");
        assert_ne!(*o, t, "{i}: not the message sent");
    }
    d.on(Event::Echo { text: &others[0], driver: false });
    assert_eq!(d.state(), Some(State::Queued), "echoed bare");
    d.on(Event::Dequeued { text: &others[1] });
    assert_eq!(d.state(), Some(State::Checking), "dequeued with another trailer");
    d.view(true, Some(&others[2]), false);
    assert_eq!(d.state(), Some(State::Held), "the slot, with two notice lines");
    d.lands(&others[1]);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat), "the history, with another trailer");
    // A notice look-alike in the body is the body: no match.
    let mut e = Desk::new();
    e.queued(false);
    let look_alike = format!("{}\n[block tests v1 dismissed] ", e.sent);
    e.on(Event::Dequeued { text: &look_alike });
    assert_eq!(e.state(), Some(State::Queued), "a line that only looks like a notice stays in the body");
}

/// A Resend's rebuilt trailer: the desires changed since the send (a
/// dismissal settled, another made); the click sends the exact edit text
/// with the trailer built at that moment, and its echo is ours.
#[test]
fn a_resends_rebuilt_trailer() {
    let mut d = Desk::new();
    let t = d.unconfirmed();
    d.desires = 7;
    let Resend::Send(again) = d.resend(true) else { panic!("Resend") };
    assert_eq!(message::split(&again).0, d.sent, "the exact edit text");
    assert_eq!(message::split(&again).1, trailer(7), "the trailer built now");
    assert_ne!(again, t);
    let bare = d.sent.clone();
    let none =
        d.memo.clone().resend(&bare, &Trailer::default(), &View { edits: &NoEdits, settled: true, queued: None, streaming: false }, d.now);
    assert_eq!(none, Resend::No, "sending now: no second click");
    d.on(Event::Echo { text: &again, driver: false });
    assert_eq!(d.state(), Some(State::Queued));
    d.lands(&again);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat));
    // With no desires at all, the bare body.
    let mut b = Desk::new();
    b.unconfirmed();
    let s = b.board();
    let h = History::new(&b.history, &s);
    let sent = b.sent.clone();
    let out = b.memo.resend(&sent, &Trailer::default(), &View { edits: &h, settled: true, queued: None, streaming: false }, b.now);
    assert_eq!(out, Resend::Send(sent));
}

/// An empty history, for a view that holds nothing.
struct NoEdits;

impl Edits for NoEdits {
    fn each(&self, _: &Ref, _: &Tag, _: &mut dyn FnMut(&str)) {}

    fn live(&self, _: &Ref, _: &Tag) -> bool {
        false
    }
}

/// Two appends of different items: appending one step never retires the
/// append of another, this desk's or another's; appending the same item
/// does.
#[test]
fn two_appends_of_different_items() {
    let add = |id: &str, label: &str| json!({"append": {"steps": [{"id": id, "label": label}]}});
    let mut d = Desk::new();
    d.hand_back(add("u-k3x7qa", "Check staging logs")).unwrap();
    let first = d.whole();
    d.on(Event::Echo { text: &first, driver: false });
    d.on(Event::Idle);
    d.settles();
    let first_body = d.sent.clone();
    // This desk appends another.
    d.hand_back(add("u-m4p2zz", "Tell the on-call")).unwrap();
    assert_eq!(d.memo.get(&first_body).map(|e| (e.state, e.retired)), Some((State::Unconfirmed, None)), "ours: not retired");
    // Another desk appends a third, and it lands.
    let third = theirs(&d, add("u-r6t5bb", "Roll back"));
    d.lands(&third);
    d.settles();
    let e = d.memo.get(&first_body).unwrap();
    assert_eq!((e.state, e.retired), (State::Unconfirmed, None), "theirs: not retired");
    let s = d.board();
    let h = History::new(&d.history, &s);
    assert!(!d.memo.may_resend(&first_body, &h), "its Resend waits while our other append is sending (one flight)");
    // Ours lands: the first keeps its Resend.
    let second = d.whole();
    d.lands(&second);
    d.settles();
    let s = d.board();
    let h = History::new(&d.history, &s);
    assert!(d.memo.may_resend(&first_body, &h), "its Resend stays");
    // The same item appended elsewhere covers it.
    let same = theirs(&d, add("u-k3x7qa", "Check the staging logs"));
    d.lands(&same);
    d.settles();
    assert_eq!(d.memo.get(&first_body).unwrap().retired, Some(Retired::Elsewhere));
}

/// What nothing covers: an appended hunk line or series number has no key
/// (§3.7), so an edit with one is never covered, even by the same edit.
#[test]
fn an_append_with_no_key_is_never_covered() {
    let d = Desk::new();
    let (tag, _) = d.live(PLAN);
    let mut parts = Parts::default();
    parts.append.insert(crate::block::Path::parse("lines").unwrap(), crate::block::Appended::Lines(vec!["+ x".into()]));
    let lines = Edit { id: r(PLAN), base: 1, of: tag, parts };
    let skipping = d.edit(PLAN, skip("bandaid"));
    let (l, s) = (Intents::of(&lines), Intents::of(&skipping));
    assert!(l.keyless && l.keys.is_empty());
    assert!(!l.covers(&l) && !s.covers(&l), "nothing covers it");
    assert!(s.covers(&s) && !l.covers(&s), "a key it has covers; one it lacks doesn't");
}

/// A memo full of recoverable entries: Hand back is off, and nothing is
/// evicted; a settled entry goes first (retired, then in the chat, oldest
/// first), never a recoverable one; a discard frees a slot.
#[test]
fn a_memo_full_of_recoverable_entries() {
    let steps: Vec<String> = (0..20).map(|i| format!("s{i}")).collect();
    let names: Vec<&str> = steps.iter().map(String::as_str).collect();
    let mut d = Desk::with(json!([put_plan(PLAN, &names)]));
    // CAP edits, each a key of its own, each unconfirmed before the next.
    let key = |i: usize| format!("steps.s{}.{}", i % 20, if i < 20 { "label" } else { "stop" });
    let value = |i: usize| if i < 20 { json!(format!("Step {i}")) } else { json!(true) };
    let mut bodies = Vec::new();
    for i in 0..CAP {
        d.unconfirmed_with(set(&key(i), value(i)));
        bodies.push(d.sent.clone());
    }
    assert_eq!(d.memo.entries().len(), CAP);
    assert!(d.memo.entries().iter().all(Entry::recoverable));
    assert_eq!(d.may_hand_back(set(&key(CAP), value(CAP))), Err(Refusal::Full), "Hand back is off");
    assert_eq!((CAP, FULL), (32, "32 edits not confirmed · resend or discard one"), "§1.8");
    let before = d.memo.entries().to_vec();
    assert_eq!(d.hand_back(set(&key(CAP), value(CAP))), Err(Refusal::Full));
    assert_eq!(d.memo.entries(), before.as_slice(), "nothing evicted");

    // One in the chat, one retired by another desk: the retired one goes
    // first, then the one in the chat, then none.
    let in_chat = format!("{}{}", bodies[3], trailer(1).text());
    d.lands(&in_chat);
    let covering = theirs(&d, set(&key(7), json!("Theirs")));
    d.lands(&covering);
    d.settles();
    assert_eq!(d.memo.get(&bodies[3]).unwrap().state, State::InChat);
    assert_eq!(d.memo.get(&bodies[7]).unwrap().retired, Some(Retired::Elsewhere));
    assert_eq!(d.may_hand_back(set(&key(CAP), value(CAP))), Ok(()));
    d.hand_back(set(&key(CAP), value(CAP))).unwrap();
    assert!(d.memo.get(&bodies[7]).is_none() && d.memo.get(&bodies[3]).is_some(), "the retired one first");
    d.unconfirmed_now();
    d.hand_back(set(&key(CAP + 1), value(CAP + 1))).unwrap();
    assert!(d.memo.get(&bodies[3]).is_none(), "then the one in the chat");
    d.unconfirmed_now();
    assert_eq!(d.may_hand_back(set(&key(CAP + 2), value(CAP + 2))), Err(Refusal::Full));
    assert_eq!(d.hand_back(set(&key(CAP + 2), value(CAP + 2))), Err(Refusal::Full), "full again: none goes");
    // A discard frees one.
    d.memo.forget(&bodies[0]).unwrap();
    assert_eq!(d.may_hand_back(set(&key(CAP + 2), value(CAP + 2))), Ok(()));
    assert!(d.hand_back(set(&key(CAP + 2), value(CAP + 2))).is_ok());
    assert_eq!(d.memo.entries().len(), CAP);

    // A covering hand-back retires its own: room, though every entry was
    // recoverable.
    let mut c = Desk::with(json!([put_plan(PLAN, &names)]));
    for i in 0..CAP {
        c.unconfirmed_with(set(&key(i), value(i)));
    }
    let covering = json!({"set": {key(0): "Again", key(1): "Again too"}});
    assert!(c.hand_back(covering).is_ok(), "it covers two: they retire, one goes");
    assert_eq!(c.memo.entries().len(), CAP);
    assert_eq!(c.memo.entries().iter().filter(|e| e.retired == Some(Retired::Here)).count(), 1, "one retired left, the oldest gone");
}

impl Desk {
    /// Handed back `parts` on the plan, echoed, `Idle`, settled without it.
    fn unconfirmed_with(&mut self, parts: Value) {
        let t = self.hand_back(parts).unwrap();
        self.on(Event::Echo { text: &t, driver: false });
        self.on(Event::Idle);
        self.settles();
        assert_eq!(self.state(), Some(State::Unconfirmed));
    }

    /// The followed hand-back, made unconfirmed.
    fn unconfirmed_now(&mut self) {
        let t = self.whole();
        self.on(Event::Echo { text: &t, driver: false });
        self.on(Event::Idle);
        self.settles();
        assert_eq!(self.state(), Some(State::Unconfirmed));
    }
}

// ── The seen prefix, bounded ──────────────────────────────────────────

/// Past [`SEEN_MAX`] of the incarnation's edits: the prefix is exact while
/// the history agrees up to the kept digests; a rollback within them is
/// placed; one below them, or a compaction, can't be: that history is no
/// evidence either way, so it neither confirms ours nor retires it.
#[test]
fn the_seen_prefix_past_seen_max() {
    let n = SEEN_MAX + 2;
    let other = |d: &Desk, i: usize| theirs(d, set("steps.test.label", json!(format!("Test {i}"))));
    let base = || {
        let mut d = Desk::new();
        for i in 0..n {
            let t = other(&d, i);
            d.lands(&t);
        }
        d
    };
    // Growing: exact.
    let mut d = base();
    let t = d.queued(false);
    let seen = &d.entry().unwrap().seen;
    assert_eq!((seen.n, seen.last.len()), (n, SEEN_MAX));
    let f = |d: &Desk| {
        d.history
            .iter()
            .filter_map(|e| if let Event2::User { text } = e { Some(message::split(text).0.to_string()) } else { None })
            .collect::<Vec<_>>()
    };
    let all = f(&d);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    assert_eq!(d.entry().unwrap().seen.agrees(&refs), Some(n));
    d.lands(&t);
    d.settles();
    assert_eq!(d.state(), Some(State::InChat));

    // A rollback within the kept digests: placed; a covering edit past it
    // retires ours.
    let mut w = base();
    w.queued(false);
    w.history.truncate(w.history.len() - 2);
    let kept = f(&w);
    let refs: Vec<&str> = kept.iter().map(String::as_str).collect();
    assert_eq!(w.entry().unwrap().seen.agrees(&refs), Some(n - 2), "placed");
    let covering = theirs(&w, skip("bandaid"));
    w.lands(&covering);
    w.on(Event::Idle);
    w.settles();
    assert_eq!(w.retired(), Some(Retired::Elsewhere));

    // A compaction: the successor holds only ours, and can't be placed:
    // not confirmed by it. Unconfirmed, with its Resend, once checked.
    let mut c = base();
    let t = c.queued(false);
    c.history = vec![Event2::User { text: t.clone() }];
    c.settles();
    assert_eq!(c.state(), Some(State::Queued), "not confirmed by a history it can't place");
    c.on(Event::Idle);
    c.settles();
    assert_eq!((c.state(), c.retired()), (Some(State::Unconfirmed), None));

    // A rollback below the kept digests, to an edit it had seen that covers
    // ours: never retired by it.
    let mut b = Desk::new();
    let first = theirs(&b, skip("bandaid"));
    b.lands(&first);
    for i in 0..n {
        let t = other(&b, i);
        b.lands(&t);
    }
    b.queued(false);
    let cut = b.history.iter().position(|e| matches!(e, Event2::User { .. })).unwrap() + 1;
    b.history.truncate(cut);
    let left = f(&b);
    let refs: Vec<&str> = left.iter().map(String::as_str).collect();
    assert_eq!(b.entry().unwrap().seen.agrees(&refs), None, "not placed");
    b.on(Event::Idle);
    b.settles();
    assert_eq!((b.state(), b.retired()), (Some(State::Unconfirmed), None), "the edit it had seen doesn't retire it");
}

/// The bound: an entry is its body and a few hundred bytes, however many
/// edits its incarnation had.
#[test]
fn an_entry_keeps_at_most_seen_max_digests() {
    let mut d = Desk::new();
    for i in 0..3 * SEEN_MAX {
        let t = theirs(&d, set("steps.test.label", json!(format!("Test {i}"))));
        d.lands(&t);
    }
    d.hand_back(skip("bandaid")).unwrap();
    let seen = &d.entry().unwrap().seen;
    assert_eq!((SEEN_MAX, seen.n, seen.last.len()), (8, 3 * SEEN_MAX, SEEN_MAX));
}

/// The history differs from what was seen below the kept digests, yet is
/// as long: the chain says so, and that history is no evidence, though it
/// holds ours: not confirmed, and recoverable.
#[test]
fn the_seen_chain_catches_a_history_that_differs_below_the_kept_digests() {
    let n = SEEN_MAX + 2;
    let mut d = Desk::new();
    for i in 0..n {
        let t = theirs(&d, set("steps.test.label", json!(format!("Test {i}"))));
        d.lands(&t);
    }
    let t = d.queued(false);
    // A rollback to before them all; then another edit, ours, and another:
    // three, past the two the chain covers, ours below the kept digests.
    let first = d.history.iter().position(|e| matches!(e, Event2::User { .. })).unwrap();
    d.history.truncate(first);
    let o = theirs(&d, set("steps.fix.label", json!("Fix 0")));
    d.lands(&o);
    d.lands(&t);
    let o = theirs(&d, set("steps.fix.label", json!("Fix 1")));
    d.lands(&o);
    let skip = n - SEEN_MAX;
    assert_eq!((d.history.iter().filter(|e| matches!(e, Event2::User { .. })).count(), skip), (3, 2));
    d.settles();
    assert_eq!(d.state(), Some(State::Queued), "not confirmed by a history it can't place");
    d.on(Event::Idle);
    d.settles();
    assert_eq!((d.state(), d.retired()), (Some(State::Unconfirmed), None));
    assert!(d.may_resend(), "recoverable");
}

/// Within the kept digests, an edit in place of one it had seen is one it
/// never saw: covering ours, it retires it.
#[test]
fn an_edit_in_place_of_one_seen_is_unknown() {
    let mut d = Desk::new();
    for i in 0..3 {
        let t = theirs(&d, set("steps.test.label", json!(format!("Test {i}"))));
        d.lands(&t);
    }
    d.queued(false);
    d.history.pop();
    let covering = theirs(&d, skip("bandaid"));
    d.lands(&covering);
    d.on(Event::Idle);
    d.settles();
    assert_eq!(d.retired(), Some(Retired::Elsewhere), "as long as what was seen, and not it");
}

/// The card memo keeps its own rule (§3.7, "Bounds"): a card's answer is
/// its whole text, so a notice line after it makes another message.
#[test]
fn a_cards_answer_is_still_matched_by_its_whole_text() {
    use crate::fold::{self, Event as F};
    use crate::{CardId, Envelope, Input, Reply, parse_answer, parse_input};
    let ask = json!({"question": "Which approach?", "options": ["Refactor", "Rewrite"]});
    let Input::Create { doc, wait } = parse_input(&ask).unwrap() else { unreachable!() };
    let id = CardId::parse("c4f9x2a").unwrap();
    let line = Envelope::showing(id.clone(), &doc, wait).to_string();
    let f = fold::fold(&[
        F::Call { tool_id: "t1".into(), name: "cards_ui".into(), input: ask },
        F::Result { tool_id: "t1".into(), text: line },
    ]);
    let mut m = super::super::Memo::default();
    let now = Instant::now();
    let reply = Reply::Answer(parse_answer(&doc, &json!({"choice": "Rewrite"})).unwrap());
    let t = m.press(f.bound(&id).unwrap(), reply, now).unwrap();
    m.on(Event::Echo { text: &format!("{t}{}", trailer(1).text()), driver: false }, now);
    assert_eq!(m.get(&id).unwrap().state, State::Sending, "with a notice line, another message");
    m.on(Event::Echo { text: &t, driver: false }, now);
    assert_eq!(m.get(&id).unwrap().state, State::Queued);
}

// ── Shady's review of B4 (P2-1 to P2-4, P3-1) ─────────────────────────

impl Desk {
    /// Hand back `parts` on the plan, then make it not sent (`retry`) or
    /// unconfirmed, moving no other entry. Its body.
    fn recoverable(&mut self, parts: Value, retry: bool) -> String {
        let t = self.hand_back(parts).unwrap();
        if retry {
            self.on(Event::Refused { reason: "daemon busy" });
            assert_eq!(self.state(), Some(State::NotSent));
        } else {
            self.on(Event::Echo { text: &t, driver: false });
            self.on(Event::Dequeued { text: &t });
            self.settles();
            assert_eq!(self.state(), Some(State::Unconfirmed));
        }
        self.sent.clone()
    }

    /// Drive the followed hand-back, just sent as `t`, to `state`: one of
    /// those in flight.
    fn drive(&mut self, t: &str, state: State) {
        if state != State::Sending {
            self.on(Event::Echo { text: t, driver: false });
        }
        if matches!(state, State::Held | State::Checking) {
            self.on(Event::Idle);
        }
        if state == State::Held {
            self.view(true, Some(t), false);
        }
        assert_eq!(self.state(), Some(state));
    }

    /// Resend and Retry on hand-back `body`: whether each shows.
    fn may_of(&self, body: &str) -> (bool, bool) {
        let s = self.board();
        let h = History::new(&self.history, &s);
        (self.memo.may_resend(body, &h), self.memo.may_retry(body, &h))
    }

    /// A Resend click on hand-back `body`.
    fn resend_of(&mut self, body: &str, settled: bool) -> Resend {
        let s = self.board();
        let h = History::new(&self.history, &s);
        let t = trailer(self.desires);
        self.memo.resend(body, &t, &View { edits: &h, settled, queued: None, streaming: false }, self.now)
    }

    /// A Retry click on hand-back `body`.
    fn retry_of(&mut self, body: &str) -> Option<String> {
        let s = self.board();
        let h = History::new(&self.history, &s);
        let t = trailer(self.desires);
        self.memo.retry(body, &t, &h, self.now)
    }
}

/// P2-1: the incarnation's one flight holds at every send. With another
/// edit of it sending, queued, held or checking, Retry and Resend are off,
/// and a click sends nothing and checks nothing; once that edit lands,
/// they are back.
#[test]
fn retry_and_resend_wait_while_another_edit_of_the_incarnation_is_in_flight() {
    for sibling in [State::Sending, State::Queued, State::Held, State::Checking] {
        for retry in [false, true] {
            let label = format!("{sibling:?}, {}", if retry { "Retry" } else { "Resend" });
            let mut d = Desk::new();
            let a = d.recoverable(set("steps.fix.label", json!("A")), retry);
            // Disjoint: A stays, recoverable, and B is accepted.
            let b = d.hand_back(set("steps.test.label", json!("B"))).unwrap();
            d.drive(&b, sibling);
            let before = d.memo.entries().to_vec();
            assert_eq!(d.may_of(&a), (false, false), "{label}: off");
            if retry {
                assert_eq!(d.retry_of(&a), None, "{label}: sends nothing");
            } else {
                assert_eq!(d.resend_of(&a, true), Resend::No, "{label}: sends nothing");
                assert_eq!(d.resend_of(&a, false), Resend::No, "{label}: checks nothing");
            }
            assert_eq!(d.memo.entries(), before.as_slice(), "{label}: nothing moved");
            d.lands(&b);
            d.settles();
            assert_eq!(d.state(), Some(State::InChat), "{label}");
            assert_eq!(d.may_of(&a), (!retry, retry), "{label}: back");
            if retry {
                assert!(d.retry_of(&a).is_some(), "{label}");
            } else {
                assert!(matches!(d.resend_of(&a, true), Resend::Send(_)), "{label}");
            }
        }
    }
}

/// P2-1: the flight is per incarnation. An edit in flight to another
/// block, or to the incarnation a replace ended, leaves Retry and Resend
/// on.
#[test]
fn another_incarnations_edit_in_flight_leaves_retry_and_resend_on() {
    for retry in [false, true] {
        // Another block: the note's edit, queued.
        let mut d = Desk::new();
        let note = d.edit("cause", set("text", json!("Not a race")));
        let n = d.send(&note).unwrap();
        d.on(Event::Echo { text: &n, driver: false });
        let a = d.recoverable(set("steps.fix.label", json!("A")), retry);
        assert_eq!(d.memo.in_flight(&r("cause"), &note.of).map(|e| e.state), Some(State::Queued));
        assert_eq!(d.may_of(&a), (!retry, retry), "another block");
        assert!(if retry { d.retry_of(&a).is_some() } else { matches!(d.resend_of(&a, true), Resend::Send(_)) });

        // The same block, replaced: its predecessor's edit is queued.
        let mut d = Desk::new();
        let old = d.queued(false);
        d.agent(json!([{"op":"replace","id":PLAN,"kind":"plan","title":"Again","body":plan(&["fix","test","deploy","bandaid"])}]));
        let a = d.recoverable(set("steps.fix.label", json!("A")), retry);
        assert_eq!(d.memo.get(&old).map(|e| e.state), Some(State::Queued));
        assert_eq!(d.may_of(&a), (!retry, retry), "another incarnation");
        assert!(if retry { d.retry_of(&a).is_some() } else { matches!(d.resend_of(&a, true), Resend::Send(_)) });
    }
}

/// P2-1: evidence isn't gated. A view showing an unconfirmed edit still in
/// the queued slot moves it to held, beside the edit of its incarnation in
/// flight: two in flight, because that is where they are.
#[test]
fn a_view_showing_an_earlier_edit_queued_moves_it_beside_the_one_in_flight() {
    let mut d = Desk::new();
    let a = d.recoverable(set("steps.fix.label", json!("A")), false);
    d.hand_back(set("steps.test.label", json!("B"))).unwrap();
    let slot = format!("{a}{}", trailer(1).text());
    d.view(true, Some(&slot), false);
    assert_eq!(d.memo.get(&a).map(|e| e.state), Some(State::Held));
    assert_eq!(d.state(), Some(State::Sending));
    assert_eq!(d.memo.entries().iter().filter(|e| e.in_flight()).count(), 2);
}

/// P2-2, Shady's counterexample: an edit identical to ours (another
/// desk's) and, with `cover`, an edit covering it, both seen when ours was
/// sent among 12; then a rollback to those two, before ours is in the
/// chat. That history can't be placed: it neither confirms ours nor
/// retires it, and ours ends unconfirmed, with its Resend.
#[test]
fn a_deep_rollback_to_an_old_identical_edit_and_its_seen_cover_settles_nothing() {
    for cover in [true, false] {
        let mut d = Desk::new();
        let wanted = d.edit(PLAN, set("steps.fix.label", json!("Wanted")));
        let first = format!("{}{}", Desk::body(&wanted), trailer(900).text());
        d.lands(&first);
        let second = theirs(&d, set(if cover { "steps.fix.label" } else { "steps.deploy.label" }, json!("Other")));
        d.lands(&second);
        for i in 0..10 {
            let t = theirs(&d, set("steps.test.label", json!(format!("Test {i}"))));
            d.lands(&t);
        }
        // A fresh hand-back of the same edit, from its base: the same body.
        let t = d.send(&wanted).unwrap();
        assert_eq!(message::split(&first).0, d.sent);
        assert_eq!(d.entry().unwrap().seen.n, 12);
        d.on(Event::Echo { text: &t, driver: false });
        let users: Vec<usize> = d.history.iter().enumerate().filter(|(_, e)| matches!(e, Event2::User { .. })).map(|(i, _)| i).collect();
        d.history.truncate(users[1] + 1);
        d.settles();
        assert_eq!((d.state(), d.retired()), (Some(State::Queued), None), "cover {cover}: neither confirmed nor retired");
        d.on(Event::Idle);
        d.settles();
        assert_eq!((d.state(), d.retired()), (Some(State::Unconfirmed), None), "cover {cover}");
        assert!(d.may_resend(), "cover {cover}: recoverable");
    }
}

/// An entry's owned bytes, on 64-bit targets (§1.8): inline, and what it
/// owns on the heap by capacity. Its id was parsed and cloned, so its
/// capacity is its length.
fn owned(e: &Entry) -> usize {
    let id = match &e.id {
        Ref::Block(b) => b.as_str().len(),
        Ref::Card(c) => c.as_str().len(),
    };
    let cause = match &e.cause {
        Some(Cause::NotSent(s)) => s.capacity(),
        _ => 0,
    };
    std::mem::size_of::<Entry>() + e.text.capacity() + id + e.seen.last.capacity() * 16 + cause
}

/// P2-3: §1.8's ceiling on an entry, derived from its inline size and the
/// caps (a body of 200 four-byte characters and 8 KiB, the longest id,
/// [`SEEN_MAX`] digests, a reason of [`REASON_MAX`]), then checked on a
/// near-maximal entry refused with a reason of 1.2 MB, which is cut, `…`
/// included. A card's answer keeps its reason whole, as before B4.
#[test]
fn an_entry_stays_under_its_ceiling_with_a_huge_refusal_reason() {
    let body_max = message::EDIT_LINE_MAX * 4 + 1 + message::EDIT_PATCH_MAX_BYTES;
    let inline = std::mem::size_of::<Entry>();
    let derived = inline + body_max + crate::block::BLOCK_ID_MAX + SEEN_MAX * 16 + REASON_MAX;
    assert!(derived <= ENTRY_MAX_BYTES, "{inline} inline + {body_max} body + id, digests and reason = {derived}");

    let mut d = Desk::new();
    for i in 0..3 * SEEN_MAX {
        let e = d.edit("cause", set("text", json!(format!("Guess {i}"))));
        let m = format!("{}{}", message::build(&e, &summary()).unwrap(), trailer(900).text());
        d.lands(&m);
    }
    let big = d.edit("cause", set("text", json!("𝕏".repeat(crate::block::NOTE_TEXT_MAX))));
    let title = "𝕏".repeat(message::EDIT_LINE_MAX);
    let wide = Summary { title: Some(&title), back_to: None, pieces: vec!["an edit"] };
    let s = d.board();
    let h = History::new(&d.history, &s);
    d.memo.hand_back(&big, &wide, &trailer(1), &h, d.now).unwrap();
    let reason = "é".repeat(600_000);
    d.on(Event::Refused { reason: &reason });
    let e = &d.memo.entries()[0];
    assert_eq!(e.state, State::NotSent);
    let (line1, line2) = e.text.split_once('\n').unwrap();
    assert_eq!(line1.chars().count(), message::EDIT_LINE_MAX);
    assert!(line1.len() > 700 && line2.len() > 6000, "near-maximal: {} and {} bytes", line1.len(), line2.len());
    let Some(Cause::NotSent(kept)) = &e.cause else { panic!("{:?}", e.cause) };
    assert!(kept.len() <= REASON_MAX && kept.capacity() <= REASON_MAX, "{} bytes", kept.len());
    assert!(kept.ends_with('…') && reason.starts_with(kept.trim_end_matches('…')), "cut, marked");
    assert_eq!(e.seen.last.capacity(), SEEN_MAX);
    assert!(owned(e) <= derived, "{} owned", owned(e));

    // The card memo, unchanged: its reason whole.
    use crate::fold::{self, Event as F};
    use crate::{CardId, Envelope, Input, Reply, parse_answer, parse_input};
    let ask = json!({"question": "Which approach?", "options": ["Refactor", "Rewrite"]});
    let Input::Create { doc, wait } = parse_input(&ask).unwrap() else { unreachable!() };
    let id = CardId::parse("c4f9x2a").unwrap();
    let line = Envelope::showing(id.clone(), &doc, wait).to_string();
    let f = fold::fold(&[
        F::Call { tool_id: "t1".into(), name: "cards_ui".into(), input: ask },
        F::Result { tool_id: "t1".into(), text: line },
    ]);
    let mut m = super::super::Memo::default();
    let reply = Reply::Answer(parse_answer(&doc, &json!({"choice": "Rewrite"})).unwrap());
    m.press(f.bound(&id).unwrap(), reply, d.now).unwrap();
    let long = "é".repeat(5_000);
    m.on(Event::Refused { reason: &long }, d.now);
    assert_eq!(m.get(&id).unwrap().cause, Some(Cause::NotSent(long)));
}

/// The test binary's allocator: the system's, counting on each thread the
/// bytes live since [`counting::measure`] began, and their peak.
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Counting;

    thread_local! {
        static LIVE: Cell<isize> = const { Cell::new(0) };
        static PEAK: Cell<isize> = const { Cell::new(0) };
    }

    fn moved(by: isize) {
        let _ = LIVE.try_with(|l| {
            l.set(l.get() + by);
            let _ = PEAK.try_with(|p| p.set(p.get().max(l.get())));
        });
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(l) };
            if !p.is_null() {
                moved(l.size() as isize);
            }
            p
        }

        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(l) };
            if !p.is_null() {
                moved(l.size() as isize);
            }
            p
        }

        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) };
            moved(-(l.size() as isize));
        }

        unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
            let q = unsafe { System.realloc(p, l, size) };
            if !q.is_null() {
                moved(size as isize - l.size() as isize);
            }
            q
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    /// Run `f` on this thread: what it returns, the bytes it left
    /// allocated, and their peak.
    pub fn measure<T>(f: impl FnOnce() -> T) -> (T, isize, isize) {
        LIVE.with(|l| l.set(0));
        PEAK.with(|p| p.set(0));
        let out = f();
        (out, LIVE.with(Cell::get), PEAK.with(Cell::get))
    }
}

/// P2-4: a long history, its edits of one incarnation many and alike
/// (no-change ones among them), and many edits of ended incarnations. The
/// adapter allocates nothing; an observation's scratch doesn't grow with
/// the history (one parsed edit at a time); a hand-back keeps only its
/// entry.
#[test]
fn a_long_history_costs_the_memo_no_owned_index() {
    let history = |edits: usize, ended: usize| {
        let mut d = Desk::new();
        d.queued(false);
        let (tag, rev) = d.live(PLAN);
        let line = Summary { title: None, back_to: None, pieces: vec!["their edit"] };
        for i in 0..edits {
            let label = if i % 2 == 0 { "Do test".to_string() } else { format!("Test {i}") };
            let e = Edit::new(r(PLAN), rev, tag, &set("steps.test.label", json!(label))).unwrap();
            d.lands(&format!("{}{}", message::build(&e, &line).unwrap(), trailer(900).text()));
        }
        for j in 0..ended {
            let gone = Tag::parse(&format!("{:032x}", j + 1)).unwrap();
            let e = Edit::new(r(PLAN), 1, gone, &skip("bandaid")).unwrap();
            d.lands(&format!("{}{}", message::build(&e, &line).unwrap(), trailer(900).text()));
        }
        d
    };
    let observe = |d: &mut Desk| {
        let s = d.board();
        let (h, left, peak) = counting::measure(|| History::new(&d.history, &s));
        assert_eq!((left, peak), (0, 0), "the adapter allocates nothing");
        let view = View { edits: &h, settled: true, queued: None, streaming: false };
        let ((), _, peak) = counting::measure(|| d.memo.observe(&view, d.now));
        peak
    };
    let mut short = history(10, 0);
    let mut long = history(2_000, 1_000);
    let (a, b) = (observe(&mut short), observe(&mut long));
    assert_eq!((short.state(), long.state()), (Some(State::Queued), Some(State::Queued)));
    assert!(b <= a + 1024 && b <= 64 * 1024, "an observation's peak: {a} bytes over 10 edits, {b} over 3,000");

    // A hand-back that sees the 3,000 (the queued one discarded, so none
    // is in flight): its entry and a few hundred bytes.
    let queued = long.sent.clone();
    long.memo.forget(&queued).unwrap();
    let s = long.board();
    let h = History::new(&long.history, &s);
    let e = long.edit(PLAN, skip("deploy"));
    let t = trailer(2);
    let (sent, left, peak) = counting::measure(|| long.memo.hand_back(&e, &summary(), &t, &h, long.now).map(drop));
    sent.unwrap();
    let entry = long.memo.entries().last().unwrap();
    assert_eq!(entry.seen.n, 2_000);
    assert!(left as usize <= owned(entry) && peak <= 64 * 1024, "kept {left} bytes (the entry owns {}), peak {peak}", owned(entry));
}

/// P3-1: the query and the hand-back agree. With [`CAP`] recoverable
/// entries, a hand-back that makes its own room (covering one, or the same
/// edit as one) is admitted, and one that doesn't is refused, by both;
/// with an edit in flight, or the block gone, both refuse alike.
#[test]
fn the_hand_back_query_and_the_hand_back_agree() {
    let steps: Vec<String> = (0..20).map(|i| format!("s{i}")).collect();
    let names: Vec<&str> = steps.iter().map(String::as_str).collect();
    let key = |i: usize| format!("steps.s{}.{}", i % 20, if i < 20 { "label" } else { "stop" });
    let value = |i: usize| if i < 20 { json!(format!("Step {i}")) } else { json!(true) };
    let mut c = Desk::with(json!([put_plan(PLAN, &names)]));
    for i in 0..CAP {
        c.unconfirmed_with(set(&key(i), value(i)));
    }
    let candidates = [
        (json!({"set": {key(0): "Again", key(1): "Again too"}}), Ok(())),
        (set(&key(5), json!("Covers five")), Ok(())),
        (set(&key(5), value(5)), Ok(())),
        (set(&key(CAP), value(CAP)), Err(Refusal::Full)),
    ];
    for (parts, want) in &candidates {
        let asked = c.may_hand_back(parts.clone());
        let before = (c.memo.clone(), c.desires, c.sent.clone());
        let got = c.hand_back(parts.clone()).map(drop);
        assert_eq!((&asked, &got), (want, want), "{parts}");
        (c.memo, c.desires, c.sent) = before;
    }
    // One in flight: both refuse every candidate alike.
    c.hand_back(candidates[0].0.clone()).unwrap();
    for (parts, _) in &candidates {
        let asked = c.may_hand_back(parts.clone());
        let got = c.hand_back(parts.clone()).map(drop);
        assert_eq!((&asked, &got), (&Err(Refusal::InFlight), &Err(Refusal::InFlight)), "{parts}");
    }
    // The block gone: both say so.
    c.agent(json!([{"op":"remove","id":PLAN}]));
    let (tag, _) = Desk::with(json!([put_plan(PLAN, &names)])).live(PLAN);
    let e = Edit::new(r(PLAN), 1, tag, &set(&key(CAP), value(CAP))).unwrap();
    let s = c.board();
    let h = History::new(&c.history, &s);
    assert_eq!(c.memo.may_hand_back(&e, &summary(), &h), Err(Refusal::Gone));
    assert_eq!(c.memo.hand_back(&e, &summary(), &trailer(9), &h, c.now).map(drop), Err(Refusal::Gone));
}
