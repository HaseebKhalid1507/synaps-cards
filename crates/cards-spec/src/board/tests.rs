//! `board`: the design's traces (§3.4, §3.5, §1.8, the graph rules, cards,
//! §7's spoofing list), one history at a time. The property tests are in
//! `tests/board.rs`.

use serde_json::{Value, json};

use super::*;
use crate::block::message::Summary;
use crate::{CardId, Envelope, Input, parse_input};

fn generation() -> Generation {
    Generation::of(&json!({"role":"user","content":"start"}))
}

fn r(id: &str) -> Ref {
    Ref::parse(id).unwrap()
}

fn path(p: &str) -> Path {
    Path::parse(p).unwrap()
}

/// A history, as the fold's events.
#[derive(Default)]
struct H {
    events: Vec<Event>,
    tools: usize,
}

impl H {
    fn tool(&mut self) -> String {
        self.tools += 1;
        format!("toolu_{:02}", self.tools)
    }

    /// One block call, without its result: its tool id.
    fn call(&mut self, ops: Value) -> String {
        let t = self.tool();
        self.events.push(Event::Call { tool_id: t.clone(), name: "cards_block".into(), input: json!({ "ops": ops }) });
        t
    }

    /// The result that binds `ops`' call.
    fn bind(&mut self, t: &str, ops: &Value) {
        let receipt = block::parse_call(&json!({ "ops": ops })).unwrap().receipt();
        self.result(t, &receipt.to_string());
    }

    fn result(&mut self, t: &str, text: &str) {
        self.events.push(Event::Result { tool_id: t.into(), text: text.into() });
    }

    /// One assistant message with one bound block call: its position.
    fn ops(&mut self, ops: Value) -> usize {
        let at = self.events.len();
        let t = self.call(ops.clone());
        self.bind(&t, &ops);
        at
    }

    fn user(&mut self, text: &str) -> Pos {
        self.events.push(Event::User { text: text.into() });
        Pos::event(self.events.len() - 1)
    }

    fn board(&self) -> Board {
        Board::fold(generation(), &self.events)
    }

    fn state(&self) -> State {
        self.board().view()
    }

    /// The user's edit of `id` from revision `base` of its live
    /// incarnation, as the desk sends it; its position.
    fn edit(&mut self, id: &str, base: u32, parts: Value) -> Pos {
        let text = edit_text(&self.state(), id, base, parts);
        self.user(&text)
    }
}

fn edit_of(s: &State, id: &str, base: u32, parts: Value) -> Edit {
    let tag = s.block(&r(id)).expect("live").tag;
    Edit::new(r(id), base, tag, &parts).unwrap()
}

fn edit_text(s: &State, id: &str, base: u32, parts: Value) -> String {
    message::build(&edit_of(s, id, base, parts), &Summary { title: None, back_to: None, pieces: vec!["edit"] }).unwrap()
}

fn plan(steps: &[&str]) -> Value {
    json!({ "steps": steps.iter().map(|s| json!({"id": s, "label": format!("Do {s}")})).collect::<Vec<_>>() })
}

fn put_plan(id: &str, steps: &[&str]) -> Value {
    json!({"op":"put","id":id,"kind":"plan","title":"Close the refresh race","body":plan(steps)})
}

fn note(id: &str, text: &str) -> Value {
    json!({"op":"put","id":id,"kind":"note","body":{"text":text}})
}

fn steps(s: &State, id: &str) -> Vec<(String, String)> {
    match &s.block(&r(id)).unwrap().body {
        Body::Plan(p) => p.steps.iter().map(|s| (s.id.as_str().to_string(), s.state.as_str().to_string())).collect(),
        b => panic!("not a plan: {b:?}"),
    }
}

fn ids(s: &State, place: Place) -> Vec<String> {
    s.list(place).iter().map(|r| r.to_string()).collect()
}

/// The log's outcomes for `id`, oldest first.
fn outcomes(s: &State, id: &str) -> Vec<Outcome> {
    s.log().filter(|l| l.id == Some(r(id))).map(|l| l.outcome.clone()).collect()
}

fn last(s: &State, id: &str) -> Outcome {
    outcomes(s, id).pop().expect("logged")
}

// Cards.

fn ask() -> Value {
    json!({ "question": "Which lock wait?", "options": ["900 ms", "1200 ms"] })
}

fn card_id() -> CardId {
    CardId::parse("c4f9x2a").unwrap()
}

impl H {
    /// `cards_ui` asks, and its result binds it to `c4f9x2a`.
    fn ask(&mut self) -> usize {
        let at = self.events.len();
        let t = self.tool();
        self.events.push(Event::Call { tool_id: t.clone(), name: "cards_ui".into(), input: ask() });
        let Input::Create { doc, wait } = parse_input(&ask()).unwrap() else { unreachable!() };
        self.result(&t, &Envelope::showing(card_id(), &doc, wait).to_string());
        at
    }

    fn answer(&mut self) {
        let Input::Create { doc, .. } = parse_input(&ask()).unwrap() else { unreachable!() };
        let a = crate::parse_answer(&doc, &json!({"choice": "900 ms"})).unwrap();
        self.user(&crate::message::build(&card_id(), &doc, &crate::Reply::Answer(a)));
    }
}

// §1.7: commit at the result.

#[test]
fn a_call_commits_once_its_result_binds() {
    let mut h = H::default();
    let t = h.call(json!([note("n", "hello")]));
    // Pending: drawn provisionally, not on the bound board.
    let b = h.board();
    assert!(b.view().block(&r("n")).unwrap().pending);
    assert!(b.bound().block(&r("n")).is_none());
    h.bind(&t, &json!([note("n", "hello")]));
    let s = h.state();
    let n = s.block(&r("n")).unwrap();
    assert!(!n.pending);
    assert_eq!((n.rev, n.creation, n.kind), (1, 1, Kind::Note));
    assert_eq!(n.tag, Tag::new(&generation(), &r("n"), 1, "toolu_01", 0));
    assert_eq!(n.place, Place::Transcript(0));
    assert_eq!(h.board().bound(), s);
}

#[test]
fn a_call_that_doesnt_bind_applies_nothing() {
    for result in [
        "cards_block: ops[0].body: required".to_string(),
        // A hook rewrote the input: another digest.
        block::parse_call(&json!({"ops":[note("n", "other")]})).unwrap().receipt().to_string(),
        // An unknown catalog (blocks/2) reads as a plain row.
        block::parse_call(&json!({"ops":[note("n", "hello")]})).unwrap().receipt().to_string().replace("blocks/1", "blocks/2"),
        // A cut line.
        "ok 1 ops (blocks/1 #3f9a".to_string(),
    ] {
        let mut h = H::default();
        let t = h.call(json!([note("n", "hello")]));
        h.result(&t, &result);
        // A later patch finds nothing.
        h.ops(json!([{"op":"patch","id":"n","set":{"text":"x"}}]));
        let s = h.state();
        assert_eq!(s.live(), 0, "{result}");
        assert_eq!(s.creations(), 0, "a skipped creation takes no number");
        assert!(s.log().any(|l| l.did == Did::Call && l.outcome == Outcome::Unbound));
        assert_eq!(last(&s, "n"), Outcome::Skipped(Why::NotThere));
    }
}

#[test]
fn a_call_without_its_result_by_the_next_message_is_unbound_for_good() {
    let mut h = H::default();
    let t = h.call(json!([note("n", "hello")]));
    h.user("steer: carry on");
    // A result after the commit belongs to no open call.
    h.bind(&t, &json!([note("n", "hello")]));
    assert_eq!(h.state().live(), 0);
    // A cancelled call keeps its paired result: one that says so is no receipt.
    let mut h = H::default();
    let t = h.call(json!([note("n", "hello")]));
    h.result(&t, "Tool execution was cancelled.");
    h.ops(json!([{"op":"patch","id":"n","set":{"text":"x"}}]));
    assert_eq!(last(&h.state(), "n"), Outcome::Skipped(Why::NotThere));
}

#[test]
fn calls_commit_in_call_order_whatever_order_results_come_in() {
    let first = json!([note("a", "one"), {"op":"put","id":"b","kind":"note","body":{"text":"two"},"at":{"after":"a"}}]);
    let second = json!([{"op":"patch","id":"a","set":{"text":"uno"}}]);
    let mut h = H::default();
    let t1 = h.call(first.clone());
    let t2 = h.call(second.clone());
    h.bind(&t2, &second);
    // The second's result is in, the first's isn't: it is pending, and so is
    // everything after it, with no revision number yet.
    let b = h.board();
    assert!(b.bound().live() == 0);
    let v = b.view();
    assert!(v.block(&r("a")).unwrap().pending && v.log().all(|l| l.pending));
    h.bind(&t1, &first);
    let s = h.state();
    assert_eq!(ids(&s, Place::Transcript(0)), ["a", "b"]);
    let a = s.block(&r("a")).unwrap();
    assert_eq!((a.rev, a.pending), (2, false));
    // The same history with the results in call order: the same board.
    let mut h2 = H::default();
    let t1 = h2.call(first.clone());
    let t2 = h2.call(second.clone());
    h2.bind(&t1, &first);
    h2.bind(&t2, &second);
    assert_eq!(h2.state(), s);
}

#[test]
fn a_tool_id_called_again_belongs_to_its_latest_call() {
    let ops = json!([note("n", "hello")]);
    let mut h = H::default();
    let t = h.call(ops.clone());
    // Another tool's call takes the id over: the block call never gets its result.
    h.events.push(Event::Call { tool_id: t.clone(), name: "bash".into(), input: Value::Null });
    h.bind(&t, &ops);
    assert_eq!(h.state().live(), 0);
}

#[test]
fn caps_apply_to_pending_ops_too() {
    let mut h = H::default();
    let puts: Vec<Value> = (0..16).map(|i| note(&format!("n{i}"), "x")).collect();
    h.ops(json!(puts));
    let puts: Vec<Value> = (16..30).map(|i| note(&format!("n{i}"), "x")).collect();
    h.ops(json!(puts));
    h.user("go on");
    // 30 live; a pending call puts 5: two draw, three are skipped as the
    // confirmed fold would skip them.
    let puts: Vec<Value> = (30..35).map(|i| note(&format!("n{i}"), "x")).collect();
    let t = h.call(json!(puts.clone()));
    let v = h.board().view();
    assert_eq!(v.live(), 32);
    assert_eq!(last(&v, "n34"), Outcome::Skipped(Why::Full));
    assert!(v.log().filter(|l| l.pending).count() == 5);
    h.bind(&t, &json!(puts));
    let s = h.state();
    assert_eq!(s.live(), 32);
    assert_eq!(last(&s, "n32"), Outcome::Skipped(Why::Full));
    assert!(s.log().all(|l| !l.pending));
}

// §1.2: incarnations, places, at.after.

#[test]
fn put_is_create_only() {
    let mut h = H::default();
    h.ops(json!([note("n", "one")]));
    h.ops(json!([note("n", "two")]));
    let s = h.state();
    let Body::Note(n) = &s.block(&r("n")).unwrap().body else { panic!() };
    assert_eq!((n.text.as_str(), s.creations()), ("one", 1));
    assert_eq!(last(&s, "n"), Outcome::Exists);
}

#[test]
fn places_lists_and_at_after() {
    let mut h = H::default();
    let a0 = h.ops(json!([note("a", "1"), note("b", "2"), {"op":"put","id":"c","kind":"note","body":{"text":"3"},"at":{"after":"a"}}]));
    let a1 = h.ops(json!([
        {"op":"put","id":"r1","kind":"note","target":"rail","body":{"text":"r"}},
        {"op":"put","id":"r2","kind":"note","slot":"end","body":{"text":"r"}},
        {"op":"put","id":"r0","kind":"note","target":"rail","at":{"after":"r1"},"body":{"text":"r"}},
        // at.after a block in another target: the put goes here, marked.
        {"op":"put","id":"d","kind":"note","at":{"after":"r1"},"body":{"text":"d"}},
    ]));
    let s = h.state();
    assert_eq!(ids(&s, Place::Transcript(a0)), ["a", "c", "b"]);
    assert_eq!(ids(&s, Place::Rail(Slot::Upper)), ["r1", "r0"]);
    assert_eq!(ids(&s, Place::Rail(Slot::End)), ["r2"]);
    assert_eq!(ids(&s, Place::Transcript(a1)), ["d"]);
    assert_eq!(last(&s, "d"), Outcome::Here);
    assert!(s.block(&r("d")).unwrap().skipped.is_some());
    // It resolves once: no standing link. `c` stays put when `a` moves.
    let a2 = h.ops(json!([{"op":"move","id":"a"}]));
    let s = h.state();
    assert_eq!(ids(&s, Place::Transcript(a0)), ["c", "b"]);
    assert_eq!(ids(&s, Place::Transcript(a2)), ["a"]);
    // A move whose at.after doesn't resolve is skipped.
    h.ops(json!([{"op":"move","id":"b","at":{"after":"zz"}}]));
    assert_eq!(last(&h.state(), "b"), Outcome::Skipped(Why::After));
    // `at` alone: into its block's list (Reading).
    h.ops(json!([{"op":"move","id":"b","at":{"after":"r1"}}]));
    assert_eq!(ids(&h.state(), Place::Rail(Slot::Upper)), ["r1", "b", "r0"]);
    // Moving within a list, to where it already is: no change.
    h.ops(json!([{"op":"move","id":"b","at":{"after":"r1"}}]));
    assert_eq!(last(&h.state(), "b"), Outcome::NoChange);
}

#[test]
fn replace_keeps_the_slot_and_starts_the_content() {
    let mut h = H::default();
    let a0 = h.ops(json!([put_plan("plan", &["fix", "test"]), note("after", "x")]));
    h.ops(json!([{"op":"collapse","id":"plan"},{"op":"patch","id":"plan","state":{"fix":"done"}}]));
    let old = h.state().block(&r("plan")).unwrap().clone();
    assert_eq!(old.rev, 2);
    let a2 = h.ops(json!([{"op":"replace","id":"plan","kind":"plan","body":plan(&["fix"]),"why":"new approach"}]));
    let s = h.state();
    let b = s.block(&r("plan")).unwrap();
    assert_ne!(b.tag, old.tag);
    assert_eq!((b.rev, b.creation, b.replaced), (1, 3, Some(Pos { event: a2, op: Some(0) })));
    assert_eq!((b.place, b.collapsed), (Place::Transcript(a0), true));
    // Decision D2: no title given, the old one stays.
    assert_eq!(b.title.as_deref(), Some("Close the refresh race"));
    assert_eq!(steps(&s, "plan"), [("fix".to_string(), "todo".to_string())]);
    assert_eq!(ids(&s, Place::Transcript(a0)), ["plan", "after"]);
    let t: Vec<_> = s.tombstones().collect();
    assert_eq!((t.len(), t[0].tag, t[0].rev, t[0].ended), (1, old.tag, 2, Pos { event: a2, op: Some(0) }));
    // The old incarnation's records stay viewable (§3.6).
    assert_eq!(s.records_of(old.tag).count(), 2);
    assert!(op_at(&h.events, t[0].ended).is_some_and(|op| matches!(op, Op::Replace(r) if r.why.as_deref() == Some("new approach"))));
}

// §1.4: paths, phases, items.

#[test]
fn a_patch_applies_path_by_path_in_four_phases() {
    let mut h = H::default();
    h.ops(json!([put_plan("plan", &["fix", "test", "deploy"])]));
    h.ops(json!([{"op":"patch","id":"plan",
        "remove":["steps.test"],
        "append":{"steps":[{"id":"verify","label":"Verify"}]},
        "set":{"steps.fix.note":"token_store.rs:118","steps.gone.label":"x","title":"New"},
        "order":{"steps":["verify","deploy","nope"]}}]));
    let s = h.state();
    let names: Vec<String> = steps(&s, "plan").into_iter().map(|(i, _)| i).collect();
    assert_eq!(names, ["verify", "deploy", "fix"]);
    let b = s.block(&r("plan")).unwrap();
    assert_eq!((b.rev, b.title.as_deref()), (2, Some("New")));
    assert!(!b.has_item(&ItemId::parse("test").unwrap()));
    assert_eq!(b.removed_at(&ItemId::parse("test").unwrap()), Some(2));
    let log = outcomes(&s, "plan");
    assert!(log.contains(&Outcome::Applied));
    assert!(log.contains(&Outcome::SkippedPath(path("steps.gone.label"), PathWhy::NotThere)));
    let rec = s.records_of(b.tag).last().unwrap();
    assert_eq!((rec.rev, rec.changed, rec.did), (2, 5, Did::Patch));
    assert_eq!(rec.paths().collect::<Vec<_>>(), ["steps.test", "steps", "steps.fix.note", "title"]);
}

#[test]
fn an_item_id_names_one_item_for_the_incarnation() {
    let mut h = H::default();
    h.ops(json!([put_plan("plan", &["fix", "test"])]));
    h.ops(json!([{"op":"patch","id":"plan","remove":["steps.fix"]}]));
    // Appended again: skipped, `steps.fix was removed in r2`.
    h.ops(json!([{"op":"patch","id":"plan","append":{"steps":[{"id":"fix","label":"Again"}]}}]));
    let s = h.state();
    assert_eq!(steps(&s, "plan").len(), 1);
    assert!(outcomes(&s, "plan").contains(&Outcome::SkippedPath(path("steps"), PathWhy::Removed(2))));
    assert_eq!(last(&s, "plan"), Outcome::SkippedPath(path("steps"), PathWhy::Removed(2)));
    // Appending a live item again is a no-op: idempotent by its id.
    h.ops(json!([{"op":"patch","id":"plan","append":{"steps":[{"id":"test","label":"Twice"}]}}]));
    let s = h.state();
    assert_eq!(outcomes(&s, "plan").last(), Some(&Outcome::NoChange));
    assert_eq!(s.block(&r("plan")).unwrap().rev, 2);
}

#[test]
fn item_churn_stops_at_64_ids() {
    let mut h = H::default();
    h.ops(json!([put_plan("plan", &["s0"])]));
    for i in 1..64 {
        h.ops(json!([{"op":"patch","id":"plan","append":{"steps":[{"id":format!("s{i}"),"label":"x"}]},"remove":[format!("steps.s{}", i - 1)]}]));
    }
    let s = h.state();
    assert_eq!(s.block(&r("plan")).unwrap().item_ids(), 64);
    h.ops(json!([{"op":"patch","id":"plan","append":{"steps":[{"id":"s64","label":"x"}]}}]));
    let s = h.state();
    assert_eq!(last(&s, "plan"), Outcome::SkippedPath(path("steps"), PathWhy::Ids));
    assert_eq!(steps(&s, "plan"), [("s63".to_string(), "todo".to_string())]);
}

#[test]
fn caps_skip_the_path_that_would_pass_them() {
    let mut h = H::default();
    let twenty: Vec<String> = (0..20).map(|i| format!("s{i}")).collect();
    let refs: Vec<&str> = twenty.iter().map(String::as_str).collect();
    h.ops(json!([put_plan("plan", &refs), {"op":"put","id":"m","kind":"metric","body":{"label":"L","value":"1","series":[1,2,3]}}]));
    h.ops(json!([{"op":"patch","id":"plan","append":{"steps":[{"id":"s20","label":"x"}]},"set":{"steps.s0.label":"First"}}]));
    let s = h.state();
    assert!(outcomes(&s, "plan").contains(&Outcome::SkippedPath(path("steps"), PathWhy::Full)));
    assert_eq!(s.block(&r("plan")).unwrap().rev, 2, "the label still applies");
    // Decision D7: a patch may remove a plan's last step (and append in
    // the same patch: remove is its first phase).
    let all: Vec<String> = (0..20).map(|i| format!("steps.s{i}")).collect();
    h.ops(json!([{"op":"patch","id":"plan","remove":all}]));
    assert!(steps(&h.state(), "plan").is_empty());
    // The byte cap: a 4 KiB plan.
    h.ops(json!([{"op":"patch","id":"plan","append":{"steps":[{"id":"s99","label":"😀".repeat(100)}]}}]));
    h.ops(json!([{"op":"patch","id":"plan","append":{"steps":(20..39).map(|i| json!({"id":format!("t{i}"),"label":"😀".repeat(100)})).collect::<Vec<_>>()}}]));
    let s = h.state();
    assert_eq!(last(&s, "plan"), Outcome::SkippedPath(path("steps"), PathWhy::TooBig));
    // A series rolls off its oldest past 32.
    h.ops(json!([{"op":"patch","id":"m","append":{"series":(0..31).collect::<Vec<_>>()}}]));
    let s = h.state();
    let Body::Metric(m) = &s.block(&r("m")).unwrap().body else { panic!() };
    assert_eq!((m.series.len(), m.series[0].get()), (32, 3.0));
}

#[test]
fn a_path_of_another_kind_is_skipped() {
    let mut h = H::default();
    h.ops(json!([note("n", "hi"), {"op":"put","id":"s","kind":"status","body":{"text":"Working"}}]));
    // `text` fits a note and a status; a 200-character one only the note.
    let long = "x".repeat(200);
    h.ops(json!([{"op":"patch","id":"s","set":{"text":long}},{"op":"patch","id":"n","set":{"state":"done"}}]));
    let s = h.state();
    assert_eq!(last(&s, "s"), Outcome::SkippedPath(path("text"), PathWhy::NotKind));
    assert_eq!(outcomes(&s, "s")[1], Outcome::Skipped(Why::Paths));
    assert_eq!(last(&s, "n"), Outcome::SkippedPath(path("state"), PathWhy::NotKind));
}

// §3.4's traces.

/// `deploy` is todo at r4.
fn at_r4() -> H {
    let mut h = H::default();
    h.ops(json!([put_plan("plan", &["fix", "test", "deploy"])]));
    h.ops(json!([{"op":"patch","id":"plan","state":{"fix":"doing"}}]));
    h.ops(json!([{"op":"patch","id":"plan","state":{"fix":"done"}}]));
    h.ops(json!([{"op":"patch","id":"plan","state":{"test":"done"}}]));
    assert_eq!(h.state().block(&r("plan")).unwrap().rev, 4);
    h
}

#[test]
fn trace_a_skip_after_the_step_ran_is_a_conflict() {
    let mut h = at_r4();
    let s4 = h.state();
    let skip = edit_text(&s4, "plan", 4, json!({"set":{"steps.deploy.state":"skipped"}}));
    h.ops(json!([{"op":"patch","id":"plan","state":{"deploy":"doing"}}]));
    // Handed back after r5 is drawn: the pre-check drops the skip.
    let pre = h.state().precheck(&edit_of(&s4, "plan", 4, json!({"set":{"steps.deploy.state":"skipped"}}))).unwrap();
    assert_eq!((pre.result, pre.codes.as_slice()), (EditResult::Applied, &[Code::AlreadyRan][..]));
    // Sent before r5 landed: the fold skips it as a conflict where it lands.
    h.ops(json!([{"op":"patch","id":"plan","state":{"deploy":"done"}}]));
    let at = h.user(&skip);
    let s = h.state();
    assert_eq!(steps(&s, "plan")[2], ("deploy".to_string(), "done".to_string()));
    let o = s.outcome(at).unwrap();
    assert_eq!((o.result, o.codes.as_slice()), (EditResult::Applied, &[Code::AlreadyRan][..]));
    assert_eq!(s.block(&r("plan")).unwrap().rev, 6, "a conflict adds no revision");
    assert_eq!(last(&s, "plan"), Outcome::NoChange);
}

#[test]
fn trace_a_reorder_keeps_the_agents_execution_truth() {
    let mut h = at_r4();
    let order = edit_text(&h.state(), "plan", 4, json!({"order":{"steps":["deploy","fix","test"]}}));
    h.ops(json!([{"op":"patch","id":"plan","state":{"deploy":"doing"}}]));
    let at = h.user(&order);
    let s = h.state();
    assert_eq!(steps(&s, "plan"), [("deploy", "doing"), ("fix", "done"), ("test", "done")].map(|(a, b)| (a.to_string(), b.to_string())));
    assert_eq!(s.outcome(at).unwrap().codes, [Code::Applied]);
    let b = s.block(&r("plan")).unwrap();
    assert_eq!(b.rev, 6);
    assert_eq!(b.writer(&path("steps")).unwrap().author, Author::User);
    assert_eq!(s.records_of(b.tag).last().unwrap().author, Author::User);
}

#[test]
fn trace_a_rewritten_block_takes_no_stale_edit() {
    // The agent replaces `plan`, keeping item id `fix` for a new task.
    let mut h = at_r4();
    let stale = edit_text(&h.state(), "plan", 4, json!({"set":{"steps.fix.label":"Mine"}}));
    h.ops(json!([{"op":"replace","id":"plan","kind":"plan","body":plan(&["fix"])}]));
    let at = h.user(&stale);
    let s = h.state();
    assert_eq!(s.outcome(at).unwrap().result, EditResult::Rewritten);
    assert_eq!(s.block(&r("plan")).unwrap().rev, 1);
    // Removed and put again: the same.
    let mut h = at_r4();
    let stale = edit_text(&h.state(), "plan", 4, json!({"set":{"steps.fix.label":"Mine"}}));
    h.ops(json!([{"op":"remove","id":"plan"}]));
    let gone = h.user(&stale);
    h.ops(json!([put_plan("plan", &["fix"])]));
    let at = h.user(&stale);
    let s = h.state();
    assert_eq!(s.outcome(gone).unwrap().result, EditResult::Gone);
    assert_eq!(s.outcome(at).unwrap().result, EditResult::Rewritten);
    assert_eq!(s.block(&r("plan")).unwrap().rev, 1);
    // A base beyond the latest: rewritten too.
    let mut h = at_r4();
    let ahead = edit_text(&h.state(), "plan", 5, json!({"set":{"steps.fix.label":"Mine"}}));
    let at = h.user(&ahead);
    assert_eq!(h.state().outcome(at).unwrap().result, EditResult::Rewritten);
}

#[test]
fn trace_two_desks_add_steps_and_a_forced_collision_skips_one() {
    let mut h = at_r4();
    let s4 = h.state();
    let add = |id: &str| json!({"append":{"steps":[{"id":id,"label":"Mine"}]},"order":{"steps":["fix",id,"test","deploy"]}});
    let a = h.user(&edit_text(&s4, "plan", 4, add("u-aaaaaa")));
    let b = h.user(&edit_text(&s4, "plan", 4, add("u-bbbbbb")));
    let s = h.state();
    assert_eq!(s.outcome(a).unwrap().codes, [Code::Applied, Code::Applied]);
    assert_eq!(s.outcome(b).unwrap().codes, [Code::Applied, Code::Applied]);
    let names: Vec<String> = steps(&s, "plan").into_iter().map(|(i, _)| i).collect();
    assert_eq!(names, ["fix", "u-bbbbbb", "test", "deploy", "u-aaaaaa"]);
    // The same id again: that item and the order naming it are skipped, visibly.
    let c = h.user(&edit_text(&s4, "plan", 4, add("u-aaaaaa")));
    let s = h.state();
    assert_eq!(s.outcome(c).unwrap().codes, [Code::Collided, Code::Collided]);
    assert_eq!(
        keys(&edit_of(&s4, "plan", 4, add("u-aaaaaa"))).iter().map(|k| k.to_string()).collect::<Vec<_>>(),
        ["steps.u-aaaaaa", "order steps"]
    );
}

#[test]
fn trace_a_restore_is_an_edit_under_the_same_guards() {
    let mut h = H::default();
    h.ops(json!([note("cause", "First guess: the retry loop")]));
    h.ops(json!([{"op":"patch","id":"cause","set":{"text":"It's the lock"}}]));
    // Back to r1: the user path of r1 that differs, as a set, from r2.
    let at = h.edit("cause", 2, json!({"set":{"text":"First guess: the retry loop"}}));
    let s = h.state();
    let Body::Note(n) = &s.block(&r("cause")).unwrap().body else { panic!() };
    assert_eq!(n.text, "First guess: the retry loop");
    assert_eq!(s.outcome(at).unwrap().codes, [Code::Applied]);
    // A restore of a skip on a step that ran since: the guard holds.
    let mut h = at_r4();
    h.edit("plan", 4, json!({"set":{"steps.deploy.state":"skipped"}}));
    h.ops(json!([{"op":"patch","id":"plan","state":{"deploy":"done"}}]));
    let back = h.edit("plan", 6, json!({"set":{"steps.deploy.state":"todo"}}));
    assert_eq!(h.state().outcome(back).unwrap().codes, [Code::AlreadyRan]);
}

#[test]
fn guards_on_every_user_path() {
    let mut h = H::default();
    h.ops(json!([
        put_plan("plan", &["a", "b", "c"]),
        {"op":"put","id":"d","kind":"diff","body":{"file":"f.rs","state":"proposed","hunks":[{"id":"h1","lines":["+x"]},{"id":"h2","lines":["-y"]}]}},
        note("n", "text"),
    ]));
    h.ops(json!([{"op":"patch","id":"plan","state":{"b":"done"}}]));
    let at = h.edit(
        "plan",
        2,
        json!({"set":{"steps.a.state":"skipped","steps.a.stop":true,"steps.b.stop":true,"steps.c.state":"todo","steps.zz.label":"x"}}),
    );
    let s = h.state();
    let o = s.outcome(at).unwrap();
    // set paths in byte order: a.state, a.stop, b.stop, c.state, zz.label.
    assert_eq!(o.codes, [Code::Applied, Code::Applied, Code::AlreadyRan, Code::Unchanged, Code::NotThere]);
    let at = h.edit("d", 1, json!({"set":{"hunks.h1.state":"kept","hunks.h2.state":"dropped"}}));
    assert_eq!(h.state().outcome(at).unwrap().codes, [Code::Applied, Code::Applied]);
    h.ops(json!([{"op":"patch","id":"d","set":{"state":"written"}}]));
    let at = h.edit("d", 3, json!({"set":{"hunks.h1.state":"dropped"}}));
    assert_eq!(h.state().outcome(at).unwrap().codes, [Code::NotProposal]);
    // A note's text past its 2 KiB: too big.
    let at = h.edit("n", 1, json!({"set":{"text":"😀".repeat(600)}}));
    assert_eq!(h.state().outcome(at).unwrap().codes, [Code::TooBig]);
    // Paths no user writes on this kind: ordinary text, no outcome.
    let s = h.state();
    let tag = s.block(&r("n")).unwrap().tag;
    let e = Edit::new(r("n"), 1, tag, &json!({"set":{"steps.a.label":"x"}})).unwrap();
    let at = h.user(&message::build(&e, &Summary::default()).unwrap());
    assert!(h.state().outcome(at).is_none());
}

#[test]
fn the_agent_overwriting_the_users_edit_is_marked() {
    let mut h = at_r4();
    let at = h.edit("plan", 4, json!({"set":{"steps.deploy.label":"Ship it"},"order":{"steps":["deploy","fix","test"]}}));
    let s = h.state();
    assert_eq!(s.outcome(at).unwrap().codes, [Code::Applied, Code::Applied]);
    assert_eq!(s.block(&r("plan")).unwrap().changed_your_edit, None);
    // The agent ticks deploy: another path, nothing overwritten.
    h.ops(json!([{"op":"patch","id":"plan","state":{"deploy":"doing"}}]));
    assert_eq!(h.state().block(&r("plan")).unwrap().changed_your_edit, None);
    h.ops(json!([{"op":"patch","id":"plan","set":{"steps.deploy.label":"Deploy"}}]));
    let s = h.state();
    assert_eq!(s.block(&r("plan")).unwrap().changed_your_edit, Some(path("steps.deploy.label")));
    assert_eq!(s.outcome(at).unwrap().codes, [Code::Overwritten, Code::Applied]);
    h.ops(json!([{"op":"patch","id":"plan","order":{"steps":["fix"]}}]));
    assert_eq!(h.state().outcome(at).unwrap().codes, [Code::Overwritten, Code::Overwritten]);
}

// §1.3: groups (the graph rules).

#[test]
fn merge_groups_one_level_and_skips_members_one_by_one() {
    let mut h = H::default();
    let a0 = h.ops(json!([
        note("x", "1"), note("a", "2"), note("b", "3"),
        {"op":"put","id":"r","kind":"note","target":"rail","body":{"text":"r"}},
    ]));
    h.ask();
    h.answer();
    h.ops(json!([{"op":"merge","into":"g","title":"Research","ids":["a","card:c4f9x2a","zz","r","b"]}]));
    let s = h.state();
    let g = s.block(&r("g")).unwrap();
    assert_eq!((g.kind, g.collapsed, g.members().len()), (Kind::Group, true, 2));
    assert_eq!(ids(&s, Place::Transcript(a0)), ["x", "g"]);
    assert_eq!(s.block(&r("a")).unwrap().group.as_ref().map(BlockId::as_str), Some("g"));
    assert_eq!(outcomes(&s, "card:c4f9x2a"), [Outcome::Skipped(Why::Card)]);
    assert_eq!(outcomes(&s, "zz"), [Outcome::Skipped(Why::NotThere)]);
    assert_eq!(last(&s, "r"), Outcome::Skipped(Why::Elsewhere));
    assert_eq!(last(&s, "g"), Outcome::Applied);
    // A group can't be a member; a member can't join another; into a live non-group skips.
    h.ops(json!([{"op":"merge","into":"g2","ids":["g","a","x"]},{"op":"merge","into":"x","ids":["b"]}]));
    let s = h.state();
    assert_eq!(outcomes(&s, "g").last(), Some(&Outcome::Skipped(Why::Group)));
    assert_eq!(last(&s, "a"), Outcome::Skipped(Why::InGroup));
    assert_eq!(last(&s, "g2"), Outcome::Skipped(Why::TooFew));
    assert_eq!(last(&s, "x"), Outcome::Skipped(Why::NotGroup));
    // Joining a live group: at its end, a revision.
    h.ops(json!([{"op":"merge","into":"g","ids":["x"]}]));
    let s = h.state();
    let g = s.block(&r("g")).unwrap();
    assert_eq!((g.members().iter().map(BlockId::as_str).collect::<Vec<_>>(), g.rev), (vec!["a", "b", "x"], 2));
    assert_eq!(ids(&s, Place::Transcript(a0)), ["g"]);
}

#[test]
fn a_group_dissolves_at_one_member_and_releases_on_remove() {
    let mut h = H::default();
    let a0 = h.ops(json!([note("p", "0"), note("a", "1"), note("b", "2"), note("c", "3")]));
    h.ops(json!([{"op":"merge","into":"g","ids":["a","b"]}]));
    // A member that moves leaves; one member left, the group ends.
    let a2 = h.ops(json!([{"op":"move","id":"a"}]));
    let s = h.state();
    assert!(s.block(&r("g")).is_none());
    assert_eq!(ids(&s, Place::Transcript(a0)), ["p", "b", "c"]);
    assert_eq!(s.block(&r("b")).unwrap().group, None);
    assert_eq!(ids(&s, Place::Transcript(a2)), ["a"]);
    assert_eq!(s.tombstones().last().unwrap().id, r("g"));
    // Removing a group releases its members at its place, in member order.
    h.ops(json!([{"op":"merge","into":"g","ids":["c","b","p"]}]));
    assert_eq!(ids(&h.state(), Place::Transcript(a0)), ["g"]);
    h.ops(json!([{"op":"remove","id":"g","why":"done"}]));
    let s = h.state();
    assert_eq!(ids(&s, Place::Transcript(a0)), ["c", "b", "p"]);
    assert!(s.blocks().all(|b| b.group.is_none()));
    // A member's patch and replace keep it in its group; its remove ends membership.
    h.ops(json!([{"op":"merge","into":"g","ids":["c","b","p"]}]));
    h.ops(
        json!([{"op":"replace","id":"c","kind":"note","body":{"text":"new"}},{"op":"replace","id":"g","kind":"note","body":{"text":"x"}}]),
    );
    let s = h.state();
    assert_eq!(s.block(&r("c")).unwrap().group.as_ref().map(BlockId::as_str), Some("g"));
    assert_eq!(last(&s, "g"), Outcome::Skipped(Why::Group));
    h.ops(json!([{"op":"remove","id":"c"}]));
    let s = h.state();
    assert_eq!(s.block(&r("g")).unwrap().members().len(), 2);
    // A group moves with its members.
    h.ops(json!([{"op":"move","id":"g","target":"rail"}]));
    let s = h.state();
    assert_eq!(ids(&s, Place::Rail(Slot::Upper)), ["g"]);
    assert!(s.block(&r("b")).unwrap().place == Place::Rail(Slot::Upper));
    // at.after a member doesn't resolve.
    h.ops(json!([{"op":"put","id":"q","kind":"note","target":"rail","at":{"after":"b"},"body":{"text":"q"}}]));
    assert_eq!(last(&h.state(), "q"), Outcome::Here);
}

#[test]
fn a_group_holds_two_to_eight() {
    let mut h = H::default();
    let puts: Vec<Value> = (0..10).map(|i| note(&format!("n{i}"), "x")).collect();
    h.ops(json!(puts));
    let all: Vec<String> = (0..8).map(|i| format!("n{i}")).collect();
    h.ops(json!([{"op":"merge","into":"g","ids":all}]));
    h.ops(json!([{"op":"merge","into":"g","ids":["n8"]}]));
    let s = h.state();
    assert_eq!(s.block(&r("g")).unwrap().members().len(), 8);
    assert_eq!(last(&s, "n8"), Outcome::Skipped(Why::GroupFull));
    // 32 live, groups and members included.
    assert_eq!(s.live(), 11);
}

// Cards (§1.5): ops on a card apply once it is resolved.

#[test]
fn card_ops_wait_for_a_resolved_card_and_the_namespaces_stay_apart() {
    let mut h = H::default();
    let asked = h.ask();
    // A block may be named like the card's id: another namespace.
    h.ops(json!([note("c4f9x2a", "a block")]));
    let replace = json!([{"op":"replace","id":"card:c4f9x2a","kind":"note","body":{"text":"Lock wait set to 900 ms"}}]);
    h.ops(replace.clone());
    h.ops(json!([{"op":"collapse","id":"card:c4f9x2a"},{"op":"remove","id":"card:c4f9x2a"}]));
    let s = h.state();
    assert_eq!(outcomes(&s, "card:c4f9x2a"), vec![Outcome::Skipped(Why::CardOpen); 3]);
    // The receipt is the card fold's, untouched.
    assert_eq!(h.board().cards().bound(&card_id()).unwrap().state, crate::fold::State::Open);
    h.answer();
    // Resolved, unconverted (Decision D1): only replace acts on it.
    h.ops(json!([{"op":"patch","id":"card:c4f9x2a","set":{"text":"x"}}]));
    assert_eq!(last(&h.state(), "card:c4f9x2a"), Outcome::Skipped(Why::NoBlock));
    h.ops(replace);
    let s = h.state();
    let c = s.block(&r("card:c4f9x2a")).unwrap();
    assert_eq!((c.kind, c.place, c.rev), (Kind::Note, Place::Transcript(asked), 1));
    assert_eq!(h.board().cards().bound(&card_id()).unwrap().receipt().unwrap().rev, 1);
    assert!(s.block(&r("c4f9x2a")).is_some(), "the block named c4f9x2a is another");
    // Then it is a block: edited as card:<id>, kept out of the rail and groups.
    let at = h.edit("card:c4f9x2a", 1, json!({"set":{"text":"Lock wait set to 1200 ms"}}));
    let here = h.ops(json!([{"op":"move","id":"card:c4f9x2a"}]));
    let s = h.state();
    assert_eq!(s.outcome(at).unwrap().codes, [Code::Applied]);
    assert_eq!(s.block(&r("card:c4f9x2a")).unwrap().place, Place::Transcript(here));
}

#[test]
fn a_card_asked_in_the_same_batch_is_still_open_at_the_commit() {
    let mut h = H::default();
    let t = h.tool();
    h.events.push(Event::Call { tool_id: t.clone(), name: "cards_ui".into(), input: ask() });
    let ops = json!([{"op":"replace","id":"card:c4f9x2a","kind":"note","body":{"text":"x"}}]);
    let b = h.call(ops.clone());
    let Input::Create { doc, wait } = parse_input(&ask()).unwrap() else { unreachable!() };
    h.result(&t, &Envelope::showing(card_id(), &doc, wait).to_string());
    h.bind(&b, &ops);
    assert_eq!(last(&h.state(), "card:c4f9x2a"), Outcome::Skipped(Why::CardOpen));
}

#[test]
fn an_edit_of_an_unconverted_card_applies_nothing() {
    let mut h = H::default();
    h.ask();
    h.answer();
    let tag = Tag::new(&generation(), &r("card:c4f9x2a"), 1, "toolu_09", 0);
    let e = Edit::new(r("card:c4f9x2a"), 1, tag, &json!({"set":{"text":"x"}})).unwrap();
    let at = h.user(&message::build(&e, &Summary::default()).unwrap());
    let s = h.state();
    assert_eq!(s.outcome(at).unwrap().result, EditResult::Gone);
    assert_eq!(s.live(), 0);
    assert_eq!(h.board().cards().bound(&card_id()).unwrap().answers.len(), 1);
}

// §3.5: notices.

fn notice(id: &str, v: u32, what: &str) -> String {
    format!("[block {id} v{v} {what}]")
}

#[test]
fn notices_apply_above_effective_version() {
    let mut h = H::default();
    h.ops(json!([note("tests", "x")]));
    h.user(&format!("looks good\n{}", notice("tests", 3, "dismissed")));
    let s = h.state();
    assert!(s.visibility().dismissed(&BlockId::parse("tests").unwrap()));
    assert_eq!(s.visibility().highest(), 3);
    // An older carrier lands later: stale.
    h.user(&format!("old\n{}", notice("tests", 2, "shown")));
    // Two desks' opposite notices at one version: the first in history wins.
    h.user(&format!("a\n{}", notice("tests", 4, "shown")));
    h.user(&format!("b\n{}", notice("tests", 4, "dismissed")));
    let s = h.state();
    let id = BlockId::parse("tests").unwrap();
    assert_eq!(s.visibility().record(&id), Some(Vis { dismissed: false, version: 4 }));
    let log: Vec<Outcome> = outcomes(&s, "tests").into_iter().skip(2).collect();
    assert_eq!(log, [Outcome::Stale { version: 2, later: 3 }, Outcome::Applied, Outcome::Stale { version: 4, later: 4 }]);
    // One id in both trailers: neither applies.
    h.user(&format!("c\n{}\n{}", notice("tests", 9, "dismissed"), notice("tests", 10, "shown")));
    let s = h.state();
    assert_eq!(s.visibility().record(&id).unwrap().version, 4);
    assert_eq!(last(&s, "tests"), Outcome::Twice);
    assert_eq!(s.visibility().highest(), 10);
}

#[test]
fn a_dismissal_belongs_to_the_id() {
    let mut h = H::default();
    h.ops(json!([note("tests", "x"), note("other", "y")]));
    // The agent removes the block between the click and delivery, then puts it again.
    h.ops(json!([{"op":"remove","id":"tests"}]));
    h.user(&format!("hm\n{}", notice("tests", 1, "dismissed")));
    h.ops(json!([note("tests", "again")]));
    let s = h.state();
    assert!(s.visibility().dismissed(&BlockId::parse("tests").unwrap()));
    // Patch, replace and collapse apply unseen; move is skipped; it can't join a group.
    h.ops(json!([
        {"op":"patch","id":"tests","set":{"text":"z"}},
        {"op":"collapse","id":"tests"},
        {"op":"move","id":"tests","target":"rail"},
        {"op":"merge","into":"g","ids":["tests","other"]},
    ]));
    let s = h.state();
    let log = outcomes(&s, "tests");
    assert_eq!(
        log[log.len() - 4..],
        [Outcome::Applied, Outcome::Applied, Outcome::Skipped(Why::Dismissed), Outcome::Skipped(Why::Dismissed)]
    );
}
