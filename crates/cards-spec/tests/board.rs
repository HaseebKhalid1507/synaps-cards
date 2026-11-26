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
    if r.chance(20) {
        return big_body(r, kind);
    }
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

/// A body at or near its kind's caps: 20 steps of long ids and labels, a
/// diff of 6 hunks and 120 lines, or from the edits, a note of 1,500
/// characters over 30 lines, a full constant series. Some pass the byte
/// cap, and the extension refuses those.
fn big_body(r: &mut Rng, kind: &str) -> Value {
    let long = |p: &str, i: usize| format!("{p}{i:0>w$}", w = block::ITEM_ID_MAX - p.len());
    let c = *r.pick(&["a", "é", "😀"]);
    match kind {
        "plan" => {
            let n = 15 + r.below(6);
            let steps: Vec<Value> = (0..n).map(|i| json!({"id": long("s", i), "label": c.repeat(10 + r.below(40))})).collect();
            json!({ "steps": steps })
        }
        "note" => json!({"text": (0..30).map(|_| c.repeat(1 + r.below(49))).collect::<Vec<_>>().join("\n")}),
        "status" => json!({"text": c.repeat(80), "detail": c.repeat(120), "progress": {"n": 3, "of": 7}}),
        "metric" => json!({"label": "L", "value": "0", "series": vec![0; block::SERIES_MAX]}),
        "diff" if r.chance(40) => json!({"file": c.repeat(200), "state": "editing", "from": "edits"}),
        "diff" => {
            let hunks: Vec<Value> = (0..6).map(|i| json!({"id": long("h", i), "lines": vec![c.repeat(1 + r.below(12)); 20]})).collect();
            json!({"file": "f.rs", "state": "proposed", "hunks": hunks})
        }
        _ => json!({"path": c.repeat(200), "line": 1, "note": c.repeat(80), "state": "reading"}),
    }
}

fn id(r: &mut Rng) -> String {
    if r.chance(40) { format!("n{}", r.below(40)) } else { r.pick(&IDS).to_string() }
}

/// A block id: the few the edits name, or one of 40 more, so 32 live blocks
/// are reachable.
fn block_id(r: &mut Rng) -> String {
    if r.chance(40) { format!("n{}", r.below(40)) } else { r.pick(&IDS[..9]).to_string() }
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
            match r.below(11) {
                8 => v["set"] = json!({"text": format!("{}\n{}", text(r, 30), text(r, 30))}),
                9 => v["append"] = json!({"hunks": [{"id": format!("h{}", 3 + r.below(4)), "lines": ["+z"]}]}),
                10 => v["append"] = json!({"series": [0]}),
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
            let into = if r.chance(70) { format!("grp{}", r.below(3)) } else { block_id(r) };
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
        // Now and then a burst of calls in one message, and a provider's
        // long tool id.
        let n = if r.chance(4) { 20 + r.below(20) } else { 1 + r.below(3) };
        for _ in 0..n {
            tool += 1;
            let t = if r.chance(5) { format!("toolu_{tool:04}_{}", "x".repeat(2048)) } else { format!("toolu_{tool:04}") };
            let (name, input) = match r.below(12) {
                0 => ("cards_ui", ask()),
                1 => ("bash", Value::Null),
                _ => ("cards_block", json!({ "ops": call_ops(r) })),
            };
            calls.push((t, input, name));
        }
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
        // The next message's results end, as `fold::events` marks it.
        if r.chance(90) {
            events.push(Event::ResultsEnd);
        }
        board.read(&events);
        if r.chance(75) {
            let text = if r.chance(10) {
                // A card answer.
                let Input::Create { doc, .. } = parse_input(&ask()).unwrap() else { unreachable!() };
                let a = cards_spec::parse_answer(&doc, &json!({"choice": "900 ms"})).unwrap();
                cards_spec::message::build(&CardId::parse(CARD).unwrap(), &doc, &cards_spec::Reply::Answer(a))
            } else {
                user_text(r, &board.bound(&events))
            };
            events.push(Event::User { text });
            board.read(&events);
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
    // Each owner's entries under its owned-byte ceiling (§1.8, D8).
    let l = s.largest_entries();
    let ceilings = [
        (l.block, BLOCK_MAX_OWNED),
        (l.tombstone, TOMBSTONE_MAX_OWNED),
        (l.record, RECORD_MAX_BYTES),
        (l.visibility, VISIBILITY_MAX_OWNED),
        (l.run, RUN_MAX_OWNED),
        (l.outcome, OUTCOME_MAX_OWNED),
        (l.log, RECORD_MAX_BYTES),
    ];
    assert!(ceilings.iter().all(|(got, max)| got <= max), "{what}: {l:?}");
    assert!(s.blocks().all(|b| b.writer_count() <= WRITERS_MAX && b.item_ids() <= block::ITEM_IDS_MAX), "{what}");
    let listed: Vec<&Ref> = s.lists().flat_map(|(_, l)| l.iter()).collect();
    for b in s.blocks() {
        // B1's stored-body contract, whoever wrote the body last.
        assert_eq!(check_stored(&b.body), Ok(()), "{what}: {:?}", b.body);
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
    // What the streams did, not only tried: each transition must occur.
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for case in 0..40 {
        let (events, _) = history(&mut r, 60);
        let mut b = Board::new(generation());
        for i in 0..events.len() {
            b.read(&events[..=i]);
            let v = b.view(&events);
            check(&v, &format!("case {case} event {i}"));
            check(&b.bound(&events), &format!("case {case} event {i} (bound)"));
            ops = ops.max(v.log().count());
            *seen.entry("max live".into()).or_default() = seen.get("max live").copied().unwrap_or(0).max(v.live());
            if v.blocks().any(|x| x.pending) {
                *seen.entry("pending".into()).or_default() += 1;
            }
        }
        let s = b.view(&events);
        assert!(b.owned_bytes() <= WORKING_SET_MAX_BYTES, "case {case}");
        for l in s.log() {
            let what = match &l.outcome {
                Outcome::SkippedPath(_, why) => format!("{:?} path {why:?}", l.did),
                Outcome::Skipped(why) => format!("{:?} skipped {why:?}", l.did),
                Outcome::Stale { .. } => format!("{:?} stale", l.did),
                o => format!("{:?} {o:?}", l.did),
            };
            *seen.entry(what).or_default() += 1;
        }
        if s.tombstones().count() > 0 {
            *seen.entry("tombstones".into()).or_default() += 1;
        }
        if s.blocks().any(|x| x.kind == Kind::Group) {
            *seen.entry("groups".into()).or_default() += 1;
        }
    }
    assert_eq!(ops, LOG_MAX, "the streams reach the log's cap");
    let count = |want: &str| seen.iter().filter(|(k, _)| k.starts_with(want)).map(|(_, n)| n).sum::<usize>();
    for want in [
        "Put Applied",
        "Patch Applied",
        "Patch NoChange",
        "Replace Applied",
        "Collapse Applied",
        "Run Applied",
        "Merge Applied",
        "Move Applied",
        "Remove Applied",
        "Call Unbound",
        "Edit Applied",
        "Edit skipped Rewritten",
        "Notice Applied",
        "Notice stale",
        "Patch path FromEdits",
        "Patch path Removed",
        "Patch path NotKind",
        "Put skipped Full",
        "Merge skipped InGroup",
        "pending",
        "tombstones",
        "groups",
    ] {
        assert!(count(want) > 0, "never: {want}; seen {seen:?}");
    }
    assert_eq!(seen["max live"], block::LIVE_MAX, "{seen:?}");
}

#[test]
fn any_split_of_a_history_folds_to_the_whole() {
    let mut r = Rng(0x5911_7000);
    for case in 0..30 {
        let (events, _) = history(&mut r, 40);
        let whole = Board::fold(generation(), &events);
        // Every split of the first three histories, six of the rest.
        let splits: Vec<usize> = if case < 3 { (0..=events.len()).collect() } else { (0..6).map(|_| r.below(events.len() + 1)).collect() };
        for k in splits {
            // A reattach: the history up to k, then the rest as it comes.
            let mut b = Board::fold(generation(), &events[..k]);
            let again = b.clone();
            b.read(&events);
            assert_eq!(b.view(&events), whole.view(&events), "case {case} split at {k}");
            assert_eq!(b.bound(&events), whole.bound(&events), "case {case} split at {k}");
            assert_eq!(again.view(&events), Board::fold(generation(), &events[..k]).view(&events[..k]), "case {case}");
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
        for i in 0..events.len() {
            hist.read(&events[..=i]);
            stream.read(&live[..=i]);
            if ends.peek() == Some(&(i + 1)) {
                ends.next();
                // The turn's end: every result in. The overlay is what the
                // history holds once the turn lands (an empty message
                // commits and changes nothing), and what a reattach folds.
                let overlay = stream.view(&live);
                assert_eq!(overlay, hist.view(&events), "case {case} event {i}");
                assert_eq!(overlay, stream.bound(&live), "case {case} event {i}: nothing pending");
                let mut landed = hist.clone();
                let mut with = events[..=i].to_vec();
                with.push(Event::User { text: String::new() });
                landed.read(&with);
                assert_eq!(overlay, landed.view(&with), "case {case} event {i}");
                assert_eq!(overlay, Board::fold(generation(), &events[..=i]).view(&events[..=i]), "case {case} event {i}");
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
        let mut with = events.clone();
        with.push(call);
        pending.read(&with);
        let v = pending.view(&with);
        check(&v, &format!("case {case} pending"));
        // Bound, it draws the same, unmarked; unbound, nothing of it.
        let mut bound = pending.clone();
        let mut bound_events = with.clone();
        bound_events.push(Event::Result { tool_id: "toolu_live".into(), text: batch.receipt().to_string() });
        bound.read(&bound_events);
        let b = bound.view(&bound_events);
        let unmark = |s: &State| {
            format!("{:?}", s.blocks().map(|b| (&b.id, &b.body, &b.title, b.rev, b.tag, b.place, &b.group)).collect::<Vec<_>>())
        };
        assert_eq!(unmark(&v), unmark(&b), "case {case}");
        assert_eq!(pending.bound(&with), before.view(&events), "case {case}: a take never starts from pending state");
        let mut unbound = pending.clone();
        with.push(Event::Result { tool_id: "toolu_live".into(), text: "Tool execution was cancelled.".into() });
        unbound.read(&with);
        assert_eq!(unmark(&unbound.view(&with)), unmark(&before.view(&events)), "case {case}");
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
        let s = Board::fold(generation(), &events).view(&events);
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
    let before = Board::fold(generation(), &base).view(&base);
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
        let s = Board::fold(generation(), &events).view(&events);
        assert_eq!(s.blocks().collect::<Vec<_>>(), before.blocks().collect::<Vec<_>>(), "{line2}");
        assert_eq!(s.outcomes().count(), 0, "{line2}");
    }
}

/// An adversarial stream at every cap together: 32 maximal blocks churning
/// their 64 item ids, the user's writes on every path, merges, 1,024
/// records, a full log of skipped paths, 64 records, runs and outcomes. Its
/// last call's batch is still open.
fn adversarial_history() -> Vec<Event> {
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
    let state = |events: &[Event]| Board::fold(generation(), events).view(events);
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
    // 40 hand-backs on p0, each rewriting every label as long as before,
    // so the plan stays at its size; then a 32nd block.
    let mut rb = Board::fold(generation(), &events);
    for round in 0..40 {
        rb.read(&events);
        let s = rb.view(&events);
        let b = s.block(&Ref::parse("p0").unwrap()).unwrap();
        let Body::Plan(p) = &b.body else { panic!() };
        let mut set = serde_json::Map::new();
        for st in &p.steps {
            set.insert(format!("steps.{}.label", st.id.as_str()), json!(if round % 2 == 0 { "😁" } else { "😀" }.repeat(30)));
            set.insert(format!("steps.{}.stop", st.id.as_str()), json!(round % 2 == 0));
        }
        let e = Edit::new(b.id.clone(), b.rev, b.tag, &json!({ "set": set })).unwrap();
        events.push(Event::User { text: message::build(&e, &Summary::default()).unwrap() });
    }
    call(&mut events, json!([{"op":"put","id":"last","kind":"note","title":"😀".repeat(80),"body":{"text":"😀".repeat(500)}}]));
    events
}

#[test]
fn adversarial_streams_stay_under_one_mib() {
    let events = adversarial_history();
    let s = Board::fold(generation(), &events).view(&events);
    assert_eq!(
        (s.live(), s.records().count(), s.log().count(), s.visibility().len(), s.runs().count(), s.outcomes().count()),
        (32, RECORDS_MAX, LOG_MAX, VISIBILITY_MAX, RUNS_MAX, OUTCOMES_MAX)
    );
    let p0 = s.block(&Ref::parse("p0").unwrap()).unwrap();
    assert!(p0.body.bytes() > 3_500 && p0.writer_count() >= 40, "p0 stays at its size: {} B", p0.body.bytes());
    // Every prefix, not only the end: the board read event by event.
    let mut b = Board::new(generation());
    for i in 0..events.len() {
        b.read(&events[..=i]);
        check(&b.view(&events), &format!("adversarial event {i}"));
        assert!(b.owned_bytes() <= WORKING_SET_MAX_BYTES, "event {i}");
    }
    assert!(s.blocks().filter(|b| b.item_ids() == block::ITEM_IDS_MAX).count() >= 16);
    let total = s.owned_bytes();
    eprintln!("adversarial working set: {total} bytes; by owner {:?}", s.owned_by_owner());
    assert!(total <= WORKING_SET_MAX_BYTES, "{total}");
}

/// §1.8 (R2-P2-1): the re-fold scratch is one working set, ≤ 1 MiB at
/// every step and once done, its card fold apart: a hand-back's outcome
/// through the whole adversarial history, and that hand-back's revision.
#[test]
fn a_refold_owns_one_working_set() {
    let mut events = adversarial_history();
    events.push(Event::ResultsEnd);
    let last = events.iter().rposition(|e| matches!(e, Event::User { .. })).unwrap();
    let whole = Board::fold(generation(), &events).view(&events);
    for target in [Target::Outcome(Pos::event(last)), Target::Revision(Pos::event(last))] {
        let mut rf = Refold::to(generation(), &events, target).unwrap();
        let mut most = rf.owned_bytes();
        while !rf.step(1000) {
            most = most.max(rf.owned_bytes());
        }
        most = most.max(rf.owned_bytes());
        let s = rf.state().unwrap();
        assert!(s.owned_bytes() > WORKING_SET_MAX_BYTES / 2, "{target:?}: a large working set, {} bytes", s.owned_bytes());
        if let Target::Outcome(_) = target {
            assert_eq!(s.blocks().collect::<Vec<_>>(), whole.blocks().collect::<Vec<_>>());
        }
        assert!(most <= WORKING_SET_MAX_BYTES, "{target:?}: the scratch owns {most} bytes");
    }
}

/// §1.8 (P1-1): the open batch holds no copy of its calls, neither while a
/// burst of calls waits for its results nor once they commit.
#[test]
fn a_burst_of_pending_calls_retains_no_copy_of_them() {
    let long = format!("toolu_{}", "x".repeat(250));
    let put = json!([{"op":"put","id":"n","kind":"note","body":{"text":"n"}}]);
    let patch = json!([{"op":"patch","id":"n","set":{"text": "y".repeat(1500)}}]);
    let receipt = |ops: &Value| block::parse_call(&json!({ "ops": ops })).unwrap().receipt().to_string();
    let mut events = vec![
        Event::Call { tool_id: "t0".into(), name: "cards_block".into(), input: json!({ "ops": put }) },
        Event::Result { tool_id: "t0".into(), text: receipt(&put) },
        Event::User { text: "go".into() },
    ];
    for k in 0..2_000 {
        events.push(Event::Call { tool_id: format!("{long}{k}"), name: "cards_block".into(), input: json!({ "ops": patch }) });
    }
    let mut b = Board::new(generation());
    b.read(&events);
    let pending = b.owned_bytes() - b.committed().owned_bytes();
    for k in 0..2_000 {
        events.push(Event::Result { tool_id: format!("{long}{k}"), text: receipt(&patch) });
    }
    events.push(Event::User { text: "next".into() });
    b.read(&events);
    let committed = b.owned_bytes() - b.committed().owned_bytes();
    assert!(pending <= 1024 && committed <= 1024, "beyond the committed state: {pending} bytes pending, {committed} once committed");
    assert!(b.owned_bytes() <= WORKING_SET_MAX_BYTES);
}

/// §1.8 (P1-1): a run title keeps no copy of its call's tool id, however
/// long the provider made it.
#[test]
fn a_run_title_keeps_no_copy_of_a_long_tool_id() {
    let long = "t".repeat(1 << 20);
    let ops = json!([{"op":"collapse","run":true,"summary":"Ran the tests"}]);
    let receipt = block::parse_call(&json!({ "ops": ops })).unwrap().receipt().to_string();
    let events = vec![
        Event::Call { tool_id: long.clone(), name: "cards_block".into(), input: json!({ "ops": ops }) },
        Event::Result { tool_id: long, text: receipt },
        Event::User { text: "ok".into() },
    ];
    let mut b = Board::new(generation());
    b.read(&events);
    assert!(b.owned_bytes() < 64 * 1024, "{} bytes", b.owned_bytes());
    assert_eq!(b.view(&events).run(0), Some("Ran the tests"));
}

/// §1.8 (P1-1): the board reports the card fold apart. Every card of the
/// history is the card fold's, as before blocks; none of it is the board's.
#[test]
fn the_card_fold_is_reported_apart_from_the_working_set() {
    let mut events = Vec::new();
    let Input::Create { doc, wait } = parse_input(&ask()).unwrap() else { unreachable!() };
    for k in 0..300 {
        let t = format!("toolu_card_{k:04}");
        events.push(Event::Call { tool_id: t.clone(), name: "cards_ui".into(), input: ask() });
        events.push(Event::Result { tool_id: t, text: Envelope::showing(CardId::parse(CARD).unwrap(), &doc, wait).to_string() });
        events.push(Event::User { text: "next".into() });
    }
    let small = Board::fold(generation(), &events[..3]);
    let b = Board::fold(generation(), &events);
    assert_eq!(b.owned_bytes(), small.owned_bytes() + (b.committed().owned_bytes() - small.committed().owned_bytes()));
    assert!(b.card_fold_bytes() > 100 * small.card_fold_bytes() / 2, "{} vs {}", b.card_fold_bytes(), small.card_fold_bytes());
}

/// §3.6, §1.8 (P2-2): every revision record a board keeps re-folds to its
/// revision: the incarnation it names, at that number.
#[test]
fn every_record_refolds_to_its_revision() {
    let mut r = Rng(0x2e40_1d00);
    let mut checked = 0;
    for case in 0..25 {
        let (events, _) = history_of(&mut r, 25, true);
        let s = Board::fold(generation(), &events).bound(&events);
        for rec in s.records() {
            let mut rf = Refold::to(generation(), &events, Target::Revision(rec.at)).unwrap();
            while !rf.step(REFOLD_SLICE) {}
            let at = rf.state().unwrap_or_else(|| panic!("case {case}: {rec:?} re-folds"));
            let b = at.blocks().find(|b| b.tag == rec.tag).unwrap_or_else(|| panic!("case {case}: {rec:?} is live there"));
            assert_eq!(b.rev, rec.rev, "case {case}: {rec:?}");
            checked += 1;
        }
    }
    assert!(checked >= 500, "{checked}");
}

fn assistant(content: Vec<Value>) -> Value {
    json!({ "role": "assistant", "content": content })
}

fn user_results(content: Vec<Value>) -> Value {
    json!({ "role": "user", "content": content })
}

fn tool_result(id: &str, content: Value) -> Value {
    json!({ "type": "tool_result", "tool_use_id": id, "content": content })
}

/// §1.7 (P2-1), from raw messages through `fold::events`: a block call
/// applies exactly when its own result, in the message right after its own,
/// binds it. Results go missing, come without text, as an image, cancelled,
/// late (in a later message), or the next message is the assistant's again;
/// other tools' calls and results ride along.
#[test]
fn raw_histories_bind_a_call_only_by_its_result_in_the_next_message() {
    let mut r = Rng(0x7a40_e5d1);
    let mut kinds = [0usize; 7];
    for case in 0..300 {
        let mut messages = vec![json!({"role":"user","content":"start"})];
        let mut expect: Vec<(String, bool)> = Vec::new();
        let mut withheld: Vec<Value> = Vec::new();
        for turn in 0..6 {
            // Earlier turns' receipts, held back: from here on they are late.
            let mut late = std::mem::take(&mut withheld);
            let mut uses = Vec::new();
            let mut results = Vec::new();
            let mut turn_expect = Vec::new();
            for k in 0..1 + r.below(3) {
                let t = format!("toolu_{case}_{turn}_{k}");
                if r.chance(25) {
                    uses.push(json!({"type":"tool_use","id":t,"name":"bash","input":{"command":"ls"}}));
                    if r.chance(80) {
                        results.push(tool_result(&t, json!("a\nb")));
                    }
                    continue;
                }
                let id = format!("b{turn}{k}");
                let ops = json!([{"op":"put","id":id,"kind":"note","body":{"text":"x"}}]);
                uses.push(json!({"type":"tool_use","id":t,"name":"cards_block","input":{"ops":ops}}));
                let receipt = block::parse_call(&json!({ "ops": ops })).unwrap().receipt().to_string();
                let way = r.below(7);
                kinds[way] += 1;
                let binds = match way {
                    0 => {
                        withheld.push(tool_result(&t, json!(receipt)));
                        false
                    }
                    1 => {
                        results.push(tool_result(&t, json!([])));
                        false
                    }
                    2 => {
                        results.push(tool_result(
                            &t,
                            json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]),
                        ));
                        false
                    }
                    3 => {
                        results.push(tool_result(&t, json!("Tool execution was cancelled.")));
                        false
                    }
                    4 => {
                        results.push(tool_result(&t, json!([{"type":"text","text":receipt}])));
                        true
                    }
                    _ => {
                        results.push(tool_result(&t, json!(receipt)));
                        true
                    }
                };
                turn_expect.push((id, binds));
            }
            messages.push(assistant(uses));
            // Late receipts of earlier turns ride in this message: too late.
            if r.chance(50) {
                results.append(&mut late);
            } else {
                withheld.append(&mut late);
            }
            if r.chance(85) || turn == 5 {
                messages.push(user_results(results));
                expect.extend(turn_expect);
                if r.chance(40) {
                    messages.push(json!({"role":"user","content":"go on"}));
                }
            } else {
                // The next message is the assistant's: this turn's calls
                // never get a result.
                kinds[6] += 1;
                expect.extend(turn_expect.into_iter().map(|(id, _)| (id, false)));
            }
        }
        let (board, events) = Board::of_history(&messages);
        let s = board.bound(&events);
        assert_eq!(s, board.view(&events), "case {case}: the last message's results are in, nothing pending");
        for (id, binds) in &expect {
            assert_eq!(s.block(&Ref::parse(id).unwrap()).is_some(), *binds, "case {case}: {id}");
        }
        check(&s, &format!("case {case}"));
    }
    assert!(kinds.iter().all(|&n| n > 20), "{kinds:?}");
}

/// §1.8: the board, the overlay and the scratch at their caps together, with
/// a burst of pending maximal calls on top: each ≤ 1 MiB.
#[test]
fn the_board_the_overlay_and_the_scratch_stay_under_their_caps_together() {
    let mut events = Vec::new();
    let long = |p: &str, i: usize| format!("{p}{i:0>w$}", w = block::ITEM_ID_MAX - p.len());
    let mut tool = 0;
    let mut call = |events: &mut Vec<Event>, ops: Value, bind: bool| {
        tool += 1;
        let t = format!("toolu_{tool:05}_{}", "x".repeat(1024));
        let receipt = block::parse_call(&json!({ "ops": ops })).unwrap().receipt().to_string();
        events.push(Event::Call { tool_id: t.clone(), name: "cards_block".into(), input: json!({ "ops": ops }) });
        if bind {
            events.push(Event::Result { tool_id: t, text: receipt });
            events.push(Event::ResultsEnd);
        }
    };
    for k in 0..32 {
        let steps: Vec<Value> = (0..20).map(|i| json!({"id": long("s", i), "label": "😀".repeat(30)})).collect();
        call(&mut events, json!([{"op":"put","id":long("p", k),"kind":"plan","title":"😀".repeat(80),"body":{"steps":steps}}]), true);
    }
    for round in 0..70 {
        let ops: Vec<Value> =
            (0..16).map(|k| json!({"op":"patch","id":long("p", (round * 16 + k) % 32),"set":{"title": format!("{round}-{k}")}})).collect();
        call(&mut events, json!(ops), true);
    }
    let mut b = Board::fold(generation(), &events);
    let committed = b.owned_bytes();
    // A burst of pending calls, each a maximal patch.
    for k in 0..200 {
        let labels: serde_json::Map<String, Value> =
            (0..20).map(|i| (format!("steps.{}.label", long("s", i)), json!("é".repeat(100)))).collect();
        call(&mut events, json!([{"op":"patch","id":long("p", k % 32),"set":labels}]), false);
    }
    b.read(&events);
    let overlay = b.view(&events);
    let rec = overlay.records().next().unwrap().clone();
    let mut rf = Refold::to(generation(), &events, Target::Revision(rec.at)).unwrap();
    let mut most = rf.owned_bytes();
    while !rf.step(1000) {
        most = most.max(rf.owned_bytes());
    }
    let scratch = rf.state().unwrap();
    // The whole scratch, not only the board it materialized.
    let all = [b.owned_bytes(), overlay.owned_bytes(), most.max(rf.owned_bytes())];
    eprintln!("board {}, overlay {}, scratch {} bytes", all[0], all[1], all[2]);
    assert_eq!(b.owned_bytes(), committed, "pending calls add nothing to the board");
    assert!(all.iter().all(|&n| n <= WORKING_SET_MAX_BYTES), "{all:?}");
    check(&overlay, "overlay");
    check(scratch, "scratch");
    assert_eq!(overlay.live(), 32);
}
