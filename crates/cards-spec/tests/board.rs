//! `cards_spec::board`'s properties (docs/blocks.md §8.1, B2), over seeded
//! streams (SplitMix64, no new dependency):
//! - the working set stays under §1.8's caps, counts and owned bytes, for
//!   random histories and adversarial streams (put/patch/remove loops,
//!   maximal bodies, item churn, merges);
//! - any split of a history folds to the whole's board;
//! - the live overlay at a turn's end, with results in any order, is the
//!   history's fold once the turn lands, and a reattach's;
//! - call, result and user interleavings, cancelled and unbound creates,
//!   caps reached by pending calls, unknown catalogs, overlapping paths.

use cards_spec::block::message::{self, Edit, Summary};
use cards_spec::block::{self, Body, Generation, ItemId, Kind, Ref};
use cards_spec::board::*;
use cards_spec::{CardId, Envelope, Input, parse_input};
use serde_json::{Value, json};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

fn generation() -> Generation {
    Generation::of(&json!({"role":"user","content":"start"}))
}

const IDS: [&str; 10] = ["a", "b", "c", "d", "g", "h", "plan", "tests", "c4f9x2a", "card:c4f9x2a"];
const ITEMS: [&str; 6] = ["fix", "test", "deploy", "s1", "s2", "s3"];
const CARD: &str = "c4f9x2a";

fn ask() -> Value {
    json!({ "question": "Which lock wait?", "options": ["900 ms", "1200 ms"] })
}

fn text(r: &mut Rng, max: usize) -> String {
    let n = 1 + r.below(max);
    let c = *r.pick(&["a", "é", "😀", "x"]);
    c.repeat(n)
}

fn steps(r: &mut Rng, n: usize, label: usize) -> Vec<Value> {
    let mut ids: Vec<&str> = ITEMS.to_vec();
    (0..n.min(ids.len()))
        .map(|_| {
            let id = ids.remove(r.below(ids.len()));
            json!({"id": id, "label": text(r, label)})
        })
        .collect()
}

fn body(r: &mut Rng, kind: &str) -> Value {
    match kind {
        "plan" => {
            let n = 1 + r.below(4);
            json!({"steps": steps(r, n, 30)})
        }
        "note" => json!({"text": text(r, 200)}),
        "status" => json!({"text": text(r, 40), "state": *r.pick(&["working", "done"])}),
        "metric" => json!({"label": "L", "value": "1", "series": (0..r.below(5)).collect::<Vec<_>>()}),
        "diff" => json!({"file": "f.rs", "state": "proposed", "hunks": [{"id": "h1", "lines": ["+x"]}, {"id": "h2", "lines": ["-y"]}]}),
        _ => json!({"path": "f.rs", "state": "reading"}),
    }
}

fn id(r: &mut Rng) -> String {
    r.pick(&IDS).to_string()
}

fn block_id(r: &mut Rng) -> String {
    r.pick(&IDS[..9]).to_string()
}

fn at(r: &mut Rng, v: &mut Value) {
    if r.chance(25) {
        v["at"] = json!({"after": id(r)});
    }
}

/// One op, as an agent might write it, valid or not.
fn op(r: &mut Rng) -> Value {
    let kinds = ["plan", "note", "status", "metric", "diff", "file"];
    match r.below(10) {
        0..=2 => {
            let kind = *r.pick(&kinds);
            let mut v = json!({"op":"put","id":block_id(r),"kind":kind,"body":body(r, kind)});
            match r.below(4) {
                0 => v["target"] = json!("rail"),
                1 => v["slot"] = json!(*r.pick(&["upper", "end"])),
                _ => {}
            }
            at(r, &mut v);
            v
        }
        3..=5 => {
            let mut v = json!({"op":"patch","id":id(r)});
            match r.below(8) {
                0 => v["set"] = json!({"text": text(r, 300)}),
                1 => v["set"] = json!({"title": text(r, 60)}),
                2 => v["state"] = json!({ *r.pick(&ITEMS): *r.pick(&["doing", "done", "todo", "failed"]) }),
                3 => {
                    let n = 1 + r.below(3);
                    v["append"] = json!({"steps": steps(r, n, 60)});
                }
                4 => v["remove"] = json!([format!("steps.{}", r.pick(&ITEMS))]),
                5 => v["order"] = json!({"steps": steps(r, 3, 1).iter().map(|s| s["id"].clone()).collect::<Vec<_>>()}),
                6 => v["append"] = json!({"series": (0..1 + r.below(40)).collect::<Vec<_>>()}),
                _ => v["set"] = json!({ format!("steps.{}.label", r.pick(&ITEMS)): text(r, 90), "state": "done" }),
            }
            v
        }
        6 => {
            let kind = *r.pick(&kinds);
            json!({"op":"replace","id":id(r),"kind":kind,"body":body(r, kind),"why":"why"})
        }
        7 => {
            let mut ids: Vec<String> = (0..1 + r.below(4)).map(|_| id(r)).collect();
            ids.sort();
            ids.dedup();
            let into = block_id(r);
            ids.retain(|i| *i != into);
            if ids.is_empty() {
                ids.push("zz".into());
            }
            json!({"op":"merge","into":into,"ids":ids})
        }
        8 => {
            let mut v = json!({"op":"move","id":id(r)});
            match r.below(4) {
                0 => v["target"] = json!(*r.pick(&["rail", "transcript"])),
                1 => v["slot"] = json!(*r.pick(&["upper", "end"])),
                _ => {}
            }
            at(r, &mut v);
            v
        }
        _ => match r.below(3) {
            0 => json!({"op":"remove","id":id(r)}),
            1 => json!({"op":"collapse","id":id(r),"open":r.chance(30)}),
            _ => json!({"op":"collapse","run":true,"summary":text(r, 80)}),
        },
    }
}

/// A valid call's ops (the extension refuses the rest; a refused call's
/// result never binds).
fn call_ops(r: &mut Rng) -> Value {
    loop {
        let ops: Vec<Value> = (0..1 + r.below(4)).map(|_| op(r)).collect();
        if block::parse_call(&json!({ "ops": ops })).is_ok() || r.chance(10) {
            return json!(ops);
        }
    }
}

/// A user message built from the bound board: an edit (maybe stale), its
/// notices, or plain text with notices.
fn user_text(r: &mut Rng, s: &State) -> String {
    let mut body = String::from("ok");
    let live: Vec<&cards_spec::board::Block> = s.blocks().filter(|b| matches!(b.kind, Kind::Plan | Kind::Note | Kind::Diff)).collect();
    if !live.is_empty() && r.chance(60) {
        let b = live[r.below(live.len())];
        let item = *r.pick(&ITEMS);
        let parts = match b.kind {
            Kind::Plan => match r.below(5) {
                0 => json!({"set":{ format!("steps.{item}.label"): text(r, 50) }}),
                1 => json!({"set":{ format!("steps.{item}.state"): *r.pick(&["skipped", "todo"]) }}),
                2 => json!({"set":{ format!("steps.{item}.stop"): r.chance(50) }}),
                3 => json!({"append":{"steps":[{"id": ItemId::mint(r.below(4) as u32).as_str(), "label": text(r, 40)}]}}),
                _ => json!({"order":{"steps": ITEMS[..3].to_vec()}}),
            },
            Kind::Note => json!({"set":{ *r.pick(&["text", "title"]): text(r, 70) }}),
            _ => json!({"set":{ format!("hunks.{}.state", r.pick(&["h1", "h2"])): *r.pick(&["kept", "dropped"]) }}),
        };
        let base = match r.below(10) {
            0 => b.rev + 1,
            1 => 1,
            _ => b.rev,
        };
        let tag = if r.chance(10) { s.tombstones().next().map_or(b.tag, |t| t.tag) } else { b.tag };
        if let Ok(e) = Edit::new(b.id.clone(), base, tag, &parts)
            && let Ok(t) = message::build(&e, &Summary { title: None, back_to: None, pieces: vec!["edit"] })
        {
            body = t;
        }
    }
    let high = s.visibility().highest();
    for what in ["dismissed", "shown"] {
        if r.chance(30) {
            let ids: Vec<String> =
                (0..1 + r.below(3)).map(|_| format!("{} v{}", r.pick(&IDS[..9]), 1 + r.below(high as usize + 3))).collect();
            body.push_str(&format!("\n[block {} {what}]", ids.join(", ")));
        }
    }
    body
}

/// A random history, the board's input: turns of calls, their results (in
/// history order) and a user message. Returns the events and, per turn,
/// the range of its results block.
fn history(r: &mut Rng, turns: usize) -> (Vec<Event>, Vec<std::ops::Range<usize>>) {
    history_of(r, turns, false)
}

/// A history where every call has its result (`complete`), as the engine
/// pairs them even through a cancel, or where some have none.
fn history_of(r: &mut Rng, turns: usize, complete: bool) -> (Vec<Event>, Vec<std::ops::Range<usize>>) {
    let mut events: Vec<Event> = Vec::new();
    let mut blocks = Vec::new();
    let mut board = Board::new(generation());
    let mut tool = 0;
    for _ in 0..turns {
        let mut calls: Vec<(String, Value, &str)> = Vec::new();
        for _ in 0..1 + r.below(3) {
            tool += 1;
            let t = format!("toolu_{tool:04}");
            let (name, input) = match r.below(12) {
                0 => ("cards_ui", ask()),
                1 => ("bash", Value::Null),
                _ => ("cards_block", json!({ "ops": call_ops(r) })),
            };
            calls.push((t, input, name));
        }
        let start = events.len();
        for (t, input, name) in &calls {
            events.push(Event::Call { tool_id: t.clone(), name: (*name).into(), input: input.clone() });
        }
        let from = events.len();
        for (t, input, name) in &calls {
            let text = match *name {
                "cards_ui" => {
                    let Input::Create { doc, wait } = parse_input(input).unwrap() else { unreachable!() };
                    Envelope::showing(CardId::parse(CARD).unwrap(), &doc, wait).to_string()
                }
                "bash" => continue,
                _ => match (block::parse_call(input), r.below(20)) {
                    (Err(e), _) => format!("cards_block: {e}"),
                    (Ok(_), 0) if !complete => continue,
                    (Ok(_), 1) => "Tool execution was cancelled.".into(),
                    (Ok(b), 2) => b.receipt().to_string().replace("blocks/1", "blocks/2"),
                    (Ok(b), 3) => {
                        let mut rc = b.receipt();
                        rc.ops += 1;
                        rc.to_string()
                    }
                    (Ok(b), _) => b.receipt().to_string(),
                },
            };
            events.push(Event::Result { tool_id: t.clone(), text });
        }
        blocks.push(from..events.len());
        for e in &events[start..] {
            board.apply(e);
        }
        if r.chance(75) {
            let text = if r.chance(10) {
                // A card answer.
                let Input::Create { doc, .. } = parse_input(&ask()).unwrap() else { unreachable!() };
                let a = cards_spec::parse_answer(&doc, &json!({"choice": "900 ms"})).unwrap();
                cards_spec::message::build(&CardId::parse(CARD).unwrap(), &doc, &cards_spec::Reply::Answer(a))
            } else {
                user_text(r, &board.bound())
            };
            let e = Event::User { text };
            board.apply(&e);
            events.push(e);
        }
    }
    (events, blocks)
}

/// §1.8's caps and the board's own invariants.
fn check(s: &State, what: &str) {
    assert!(s.live() <= block::LIVE_MAX, "{what}");
    assert!(s.owned_bytes() <= WORKING_SET_MAX_BYTES, "{what}: {} bytes", s.owned_bytes());
    assert!(s.tombstones().count() <= TOMBSTONES_MAX, "{what}");
    assert!(s.records().count() <= RECORDS_MAX, "{what}");
    assert!(s.visibility().len() <= VISIBILITY_MAX, "{what}");
    assert!(s.runs().count() <= RUNS_MAX, "{what}");
    assert!(s.outcomes().count() <= OUTCOMES_MAX, "{what}");
    assert!(s.log().count() <= LOG_MAX, "{what}");
    let (record, line) = s.largest_entries();
    assert!(record <= RECORD_MAX_BYTES && line <= RECORD_MAX_BYTES, "{what}: {record} {line}");
    let listed: Vec<&Ref> = s.lists().flat_map(|(_, l)| l.iter()).collect();
    for b in s.blocks() {
        assert!(b.body.bytes() <= b.kind.max_bytes(), "{what}: {} over its cap", b.id);
        assert!(b.item_ids() <= block::ITEM_IDS_MAX && (1..=block::REVISIONS_MAX).contains(&b.rev), "{what}");
        assert!(s.records_of(b.tag).count() <= RECORDS_PER_BLOCK, "{what}");
        // Listed once, unless a member; a member's group lists it; one level.
        let n = listed.iter().filter(|r| **r == &b.id).count();
        match &b.group {
            None => assert_eq!(n, 1, "{what}: {} listed {n} times", b.id),
            Some(g) => {
                assert_eq!(n, 0, "{what}");
                let g = s.block(&Ref::Block(g.clone())).expect("a member's group is live");
                assert!(g.members().iter().any(|m| Some(m) == b.id.block()), "{what}");
                assert_eq!(g.place, b.place, "{what}");
            }
        }
        if b.kind == Kind::Group {
            assert!((2..=block::GROUP_MAX).contains(&b.members().len()), "{what}: {} has {:?}", b.id, b.members());
            assert!(b.group.is_none(), "{what}");
            for m in b.members() {
                let m = s.block(&Ref::Block(m.clone())).expect("a member is live");
                assert!(m.kind != Kind::Group && m.group.as_ref() == b.id.block(), "{what}");
            }
        }
        if let Ref::Card(_) = b.id {
            assert!(matches!(b.place, cards_spec::board::Place::Transcript(_)) && b.group.is_none(), "{what}");
        }
    }
    assert_eq!(listed.len(), s.blocks().filter(|b| b.group.is_none()).count(), "{what}");
}

#[test]
fn random_histories_keep_the_caps_at_every_event() {
    let mut r = Rng(0xb0a2_d001);
    let mut ops = 0;
    for case in 0..40 {
        let (events, _) = history(&mut r, 60);
        let mut b = Board::new(generation());
        for (i, e) in events.iter().enumerate() {
            b.apply(e);
            let v = b.view();
            check(&v, &format!("case {case} event {i}"));
            check(&b.bound(), &format!("case {case} event {i} (bound)"));
            ops = ops.max(v.log().count());
        }
    }
    assert_eq!(ops, LOG_MAX, "the streams reach the log's cap");
}

#[test]
fn any_split_of_a_history_folds_to_the_whole() {
    let mut r = Rng(0x5911_7000);
    for case in 0..30 {
        let (events, _) = history(&mut r, 40);
        let whole = Board::fold(generation(), &events);
        for _ in 0..6 {
            let k = r.below(events.len() + 1);
            // A reattach: the history up to k, then the rest as it comes.
            let mut b = Board::fold(generation(), &events[..k]);
            let again = b.clone();
            for e in &events[k..] {
                b.apply(e);
            }
            assert_eq!(b.view(), whole.view(), "case {case} split at {k}");
            assert_eq!(b.bound(), whole.bound(), "case {case} split at {k}");
            assert_eq!(again.view(), Board::fold(generation(), &events[..k]).view(), "case {case}");
        }
    }
}

#[test]
fn the_overlay_at_a_turns_end_is_the_history_fold_and_the_reattach_fold() {
    let mut r = Rng(0x0e7a_11a5);
    let mut checked = 0;
    for case in 0..40 {
        let (events, blocks) = history_of(&mut r, 30, true);
        // The live stream: each results block in another order.
        let mut live = events.clone();
        for b in &blocks {
            let slice = &mut live[b.clone()];
            for i in (1..slice.len()).rev() {
                slice.swap(i, r.below(i + 1));
            }
        }
        let mut hist = Board::new(generation());
        let mut stream = Board::new(generation());
        let mut ends = blocks.iter().map(|b| b.end).peekable();
        for (i, (h, l)) in events.iter().zip(&live).enumerate() {
            hist.apply(h);
            stream.apply(l);
            if ends.peek() == Some(&(i + 1)) {
                ends.next();
                // The turn's end: every result in. The overlay is what the
                // history holds once the turn lands (an empty message
                // commits and changes nothing), and what a reattach folds.
                let overlay = stream.view();
                assert_eq!(overlay, hist.view(), "case {case} event {i}");
                assert_eq!(overlay, stream.bound(), "case {case} event {i}: nothing pending");
                let mut landed = hist.clone();
                landed.apply(&Event::User { text: String::new() });
                assert_eq!(overlay, landed.view(), "case {case} event {i}");
                assert_eq!(overlay, Board::fold(generation(), &events[..=i]).view(), "case {case} event {i}");
                assert!(overlay.log().all(|l| !l.pending));
                checked += 1;
            }
        }
    }
    assert!(checked >= 1000, "{checked}");
}

#[test]
fn a_pending_call_draws_what_its_binding_will_and_none_if_it_doesnt() {
    let mut r = Rng(0x9e4d_1a6c);
    for case in 0..300 {
        let (events, _) = history_of(&mut r, 12, true);
        let before = Board::fold(generation(), &events);
        let ops = call_ops(&mut r);
        let Ok(batch) = block::parse_call(&json!({ "ops": ops })) else { continue };
        let call = Event::Call { tool_id: "toolu_live".into(), name: "cards_block".into(), input: json!({ "ops": ops }) };
        let mut pending = before.clone();
        pending.apply(&call);
        let v = pending.view();
        check(&v, &format!("case {case} pending"));
        // Bound, it draws the same, unmarked; unbound, nothing of it.
        let mut bound = pending.clone();
        bound.apply(&Event::Result { tool_id: "toolu_live".into(), text: batch.receipt().to_string() });
        let b = bound.view();
        let unmark = |s: &State| {
            format!("{:?}", s.blocks().map(|b| (&b.id, &b.body, &b.title, b.rev, b.tag, b.place, &b.group)).collect::<Vec<_>>())
        };
        assert_eq!(unmark(&v), unmark(&b), "case {case}");
        assert_eq!(pending.bound(), before.view(), "case {case}: a take never starts from pending state");
        let mut unbound = pending.clone();
        unbound.apply(&Event::Result { tool_id: "toolu_live".into(), text: "Tool execution was cancelled.".into() });
        assert_eq!(unmark(&unbound.view()), unmark(&before.view()), "case {case}");
    }
}

#[test]
fn unbound_and_cancelled_creates_leave_their_patches_nothing() {
    let mut r = Rng(0xca9c_e11d);
    for case in 0..200 {
        let id = format!("n{case}");
        let put = json!([{"op":"put","id":id,"kind":"note","body":{"text":"x"}}]);
        let patch = json!([{"op":"patch","id":id,"set":{"text":"y"}},{"op":"remove","id":id}]);
        let batch = block::parse_call(&json!({ "ops": put })).unwrap();
        let result = match r.below(5) {
            0 => "Tool execution was cancelled.".to_string(),
            1 => batch.receipt().to_string().replace("blocks/1", "blocks/7"),
            2 => format!("{}…", &batch.receipt().to_string()[..20]),
            3 => block::parse_call(&json!({"ops":[{"op":"put","id":id,"kind":"note","body":{"text":"hooked"}}]}))
                .unwrap()
                .receipt()
                .to_string(),
            _ => "cards_block: 31 calls in the last minute; batch your ops or patch less".into(),
        };
        let events = vec![
            Event::Call { tool_id: "t1".into(), name: "cards_block".into(), input: json!({ "ops": put }) },
            Event::Result { tool_id: "t1".into(), text: result.clone() },
            Event::Call { tool_id: "t2".into(), name: "cards:block".into(), input: json!({ "ops": patch }) },
            Event::Result { tool_id: "t2".into(), text: block::parse_call(&json!({ "ops": patch })).unwrap().receipt().to_string() },
        ];
        let s = Board::fold(generation(), &events).view();
        assert_eq!((s.live(), s.creations()), (0, 0), "{result}");
        let lines: Vec<Outcome> = s.log().map(|l| l.outcome.clone()).collect();
        assert_eq!(lines, [Outcome::Unbound, Outcome::Skipped(Why::NotThere), Outcome::NoChange], "{result}");
    }
}

#[test]
fn overlapping_paths_never_apply() {
    let plan = json!([{"op":"put","id":"plan","kind":"plan","body":{"steps":[{"id":"fix","label":"Fix"}]}}]);
    let base = vec![
        Event::Call { tool_id: "t1".into(), name: "cards_block".into(), input: json!({ "ops": plan }) },
        Event::Result { tool_id: "t1".into(), text: block::parse_call(&json!({ "ops": plan })).unwrap().receipt().to_string() },
    ];
    let before = Board::fold(generation(), &base).view();
    let tag = before.block(&Ref::parse("plan").unwrap()).unwrap().tag;
    // The agent's: the extension refuses, so the call never binds.
    for patch in [
        json!({"op":"patch","id":"plan","set":{"steps.fix.label":"x"},"remove":["steps.fix"]}),
        json!({"op":"patch","id":"plan","set":{"steps.fix.state":"done"},"state":{"fix":"done"}}),
        json!({"op":"patch","id":"plan","append":{"steps":[{"id":"new","label":"x"}]},"set":{"steps.new.label":"y"}}),
    ] {
        assert!(block::parse_call(&json!({"ops":[patch]})).is_err());
    }
    // The user's, typed: ordinary text, so the board is as it was.
    for line2 in [
        format!(
            "{{\"of\":\"{tag}\",\"append\":{{\"steps\":[{{\"id\":\"u-aaaaaa\",\"label\":\"x\"}}]}},\"set\":{{\"steps.u-aaaaaa.label\":\"y\"}}}}"
        ),
        format!("{{\"of\":\"{tag}\",\"order\":{{\"steps\":[\"fix\",\"fix\"]}}}}"),
        format!("{{\"of\":\"{tag}\",\"set\":{{\"steps.fix.label\":\"a\",\"steps.fix.label\":\"b\"}}}}"),
    ] {
        let mut events = base.clone();
        events.push(Event::User { text: format!("[block plan edited r1] plan → x\n{line2}") });
        let s = Board::fold(generation(), &events).view();
        assert_eq!(s.blocks().collect::<Vec<_>>(), before.blocks().collect::<Vec<_>>(), "{line2}");
        assert_eq!(s.outcomes().count(), 0, "{line2}");
    }
}

/// Adversarial streams at every cap together: 32 maximal blocks churning
/// their 64 item ids, the user's writes on every path, merges, 1,024
/// records, a full log of skipped paths, 64 records, runs and outcomes.
#[test]
fn adversarial_streams_stay_under_one_mib() {
    let mut events: Vec<Event> = Vec::new();
    let mut tool = 0;
    let mut call = |events: &mut Vec<Event>, ops: Value| {
        tool += 1;
        let t = format!("toolu_{tool:05}");
        let receipt = block::parse_call(&json!({ "ops": ops })).map(|b| b.receipt().to_string()).unwrap_or_default();
        events.push(Event::Call { tool_id: t.clone(), name: "cards_block".into(), input: json!({ "ops": ops }) });
        events.push(Event::Result { tool_id: t, text: receipt });
    };
    let long = |i: usize| format!("{}-{i:04}", "x".repeat(18));
    // 16 plans and 16 diffs at their byte caps.
    for k in 0..32 {
        let ops = if k < 16 {
            let steps: Vec<Value> = (0..20).map(|i| json!({"id": long(i), "label": "😀".repeat(25), "note": "n".repeat(20)})).collect();
            json!([{"op":"put","id":format!("p{k}"),"kind":"plan","title":"😀".repeat(80),"body":{"steps":steps}}])
        } else {
            let hunks: Vec<Value> =
                (0..6).map(|i| json!({"id": long(i), "at": "a".repeat(80), "lines": vec!["😀".repeat(50); 4]})).collect();
            json!([{"op":"put","id":format!("d{k}"),"kind":"diff","title":"😀".repeat(80),"body":{"file":"f".repeat(200),"state":"proposed","hunks":hunks}}])
        };
        call(&mut events, ops);
    }
    let state = |events: &[Event]| Board::fold(generation(), events).view();
    let s = state(&events);
    assert_eq!(s.live(), 32);
    assert!(s.blocks().all(|b| b.body.bytes() > b.kind.max_bytes() * 3 / 4), "near their caps");
    // Item churn on the plans: 44 more ids each, 20 live, 64 named.
    for k in 0..16 {
        let mut live: Vec<usize> = (0..20).collect();
        for new in [20..35, 35..50, 50..64] {
            let gone: Vec<String> = live.drain(..new.len()).map(|i| format!("steps.{}", long(i))).collect();
            live.extend(new.clone());
            let new: Vec<Value> = new.map(|i| json!({"id": long(i), "label": "😀".repeat(25)})).collect();
            call(&mut events, json!([{"op":"patch","id":format!("p{k}"),"remove":gone,"append":{"steps":new}}]));
        }
    }
    // Merges: four diffs go, four groups of the rest come, one dissolves.
    call(&mut events, json!([{"op":"remove","ids":["d28","d29","d30","d31"]}]));
    for k in 0..4 {
        let ids: Vec<String> = (16 + k * 3..16 + k * 3 + 3).map(|i| format!("d{i}")).collect();
        call(&mut events, json!([{"op":"merge","into":format!("g{k}"),"title":"😀".repeat(80),"ids":ids}]));
    }
    call(&mut events, json!([{"op":"move","id":"d16","target":"rail"},{"op":"move","id":"d17","slot":"end"}]));
    // A full log of skipped paths with long ids, and 1,024 records of four
    // long paths each.
    for round in 0..80 {
        let ops: Vec<Value> = (0..16)
            .map(|k| {
                let mut set = serde_json::Map::new();
                set.insert(format!("steps.{}.note", long(1000 + round)), json!("n"));
                for i in 0..4 {
                    set.insert(format!("steps.{}.label", long(44 + (round + i) % 20)), json!(format!("{round}-{i}")));
                }
                json!({"op":"patch","id":format!("p{k}"),"set":set})
            })
            .collect();
        call(&mut events, json!(ops));
    }
    // The user writes every path of every live step, in big hand-backs.
    let s = state(&events);
    for k in 0..16 {
        let b = s.block(&Ref::parse(&format!("p{k}")).unwrap()).unwrap();
        let Body::Plan(p) = &b.body else { panic!() };
        let mut set = serde_json::Map::new();
        for st in &p.steps {
            set.insert(format!("steps.{}.label", st.id.as_str()), json!("😀".repeat(30)));
            set.insert(format!("steps.{}.stop", st.id.as_str()), json!(true));
            set.insert(format!("steps.{}.state", st.id.as_str()), json!("skipped"));
        }
        let order: Vec<&str> = p.steps.iter().rev().map(|s| s.id.as_str()).collect();
        let e = Edit::new(b.id.clone(), b.rev, b.tag, &json!({"set": set, "order": {"steps": order}})).unwrap();
        events.push(Event::User { text: message::build(&e, &Summary::default()).unwrap() });
    }
    // 64 tombstones of long ids.
    for i in 0..70 {
        let id = format!("{}{i:03}", "t".repeat(21));
        call(&mut events, json!([{"op":"put","id":id,"kind":"note","body":{"text":"x"}},{"op":"remove","id":id,"why":"😀".repeat(80)}]));
    }
    // 64 visibility records, 64 runs, 32 outcomes with many codes.
    for v in 1..=80u32 {
        events.push(Event::User { text: format!("x\n[block {}-{v:03} v{v} dismissed]", "z".repeat(18)) });
    }
    for _ in 0..70 {
        call(&mut events, json!([{"op":"collapse","run":true,"summary":"😀".repeat(80)}]));
    }
    let s = state(&events);
    for _ in 0..40 {
        let b = s.block(&Ref::parse("p0").unwrap()).unwrap();
        let Body::Plan(p) = &b.body else { panic!() };
        let mut set = serde_json::Map::new();
        for st in &p.steps {
            set.insert(format!("steps.{}.label", st.id.as_str()), json!("y"));
            set.insert(format!("steps.{}.stop", st.id.as_str()), json!(false));
        }
        let e = Edit::new(b.id.clone(), b.rev, b.tag, &json!({ "set": set })).unwrap();
        events.push(Event::User { text: message::build(&e, &Summary::default()).unwrap() });
    }
    let s = state(&events);
    check(&s, "adversarial");
    assert_eq!(
        (s.live(), s.records().count(), s.log().count(), s.visibility().len(), s.runs().count(), s.outcomes().count()),
        (31, RECORDS_MAX, LOG_MAX, VISIBILITY_MAX, RUNS_MAX, OUTCOMES_MAX)
    );
    assert!(s.blocks().filter(|b| b.item_ids() == block::ITEM_IDS_MAX).count() >= 16);
    let total = s.owned_bytes();
    eprintln!("adversarial working set: {total} bytes; by owner {:?}", s.owned_by_owner());
    assert!(total <= WORKING_SET_MAX_BYTES, "{total}");
}
