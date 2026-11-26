//! Unit tests of `block`: the grammar's edges, the overlap rule, kind
//! inference, the receipt, the tag and the messages. The corpus
//! (`fixtures/blocks/`, `tests/blocks.rs`) pins the bytes.

use serde_json::json;

use super::message::{self, Edit, Notice, NoticeLine, Summary, Trailer, Visibility};
use super::*;
use crate::tests::safe_line;

fn call(ops: Value) -> Result<Batch, Error> {
    parse_call(&json!({ "ops": ops }))
}

#[track_caller]
fn ok(ops: Value) -> Value {
    let b = call(ops.clone()).unwrap_or_else(|e| panic!("{ops}: {e}"));
    let v = serde_json::to_value(&b).unwrap();
    // Normalizing is a fixed point.
    assert_eq!(parse_call(&v).as_ref(), Ok(&b), "{v}");
    v["ops"].clone()
}

#[track_caller]
fn refused(ops: Value) -> String {
    let e = call(ops.clone()).expect_err(&ops.to_string()).to_string();
    assert!(safe_line(&e), "{e}");
    e
}

#[test]
fn block_ids_item_ids_and_refs() {
    let b24 = format!("a{}", "b".repeat(23));
    assert!(BlockId::parse(&b24).is_some() && BlockId::parse(&format!("{b24}c")).is_none());
    for bad in ["", "1plan", "Plan", "pl an", "plan.x", "card:c4f9x2a", "pl:an", "é"] {
        assert_eq!(BlockId::parse(bad), None, "{bad}");
    }
    for good in ["p", "plan", "auth-diff", "t_1"] {
        assert!(BlockId::parse(good).is_some(), "{good}");
    }
    let i24 = "0".repeat(24);
    assert!(ItemId::parse(&i24).is_some() && ItemId::parse(&format!("{i24}0")).is_none());
    for good in ["0", "fix", "u-k3x7qa", "9_x-y"] {
        assert!(ItemId::parse(good).is_some(), "{good}");
    }
    for bad in ["", "-x", "_x", "Fix", "a.b"] {
        assert_eq!(ItemId::parse(bad), None, "{bad}");
    }
    assert_eq!(Ref::parse("card:c4f9x2a").map(|r| r.to_string()), Some("card:c4f9x2a".into()));
    assert_eq!(Ref::parse("card:plan"), None);
    assert_eq!(Ref::parse("plan").and_then(|r| r.block().cloned()), BlockId::parse("plan"));
}

/// §1.4: `u-` and 6 of a-z2-7, from 30 random bits.
#[test]
fn user_ids_are_minted_from_30_bits() {
    assert_eq!(ItemId::mint(0).as_str(), "u-aaaaaa");
    assert_eq!(ItemId::mint(0x3fff_ffff).as_str(), "u-777777");
    // u-k3x7qa: k=10, 3=27, x=23, 7=31, q=16, a=0.
    let bits = (10 << 25) | (27 << 20) | (23 << 15) | (31 << 10) | (16 << 5);
    assert_eq!(ItemId::mint(bits).as_str(), "u-k3x7qa");
    assert_eq!(ItemId::mint(bits | 0xc000_0000), ItemId::mint(bits), "the high 2 bits are not used");
    for id in ["u-k3x7qa", "u-aaaaaa", "u-777777"] {
        let id = ItemId::parse(id).unwrap();
        assert!(id.is_user() && id.is_minted(), "{id:?}");
    }
    // v1.1's u-k3x9qa (Shady round 2 L2): `9` isn't in the alphabet.
    for id in ["u-k3x9qa", "u-k3x7q", "u-k3x7qaa", "u-K3X7QA", "u-k3x1qa", "u-k3x8qa"] {
        let parsed = ItemId::parse(id);
        assert!(!parsed.as_ref().is_some_and(ItemId::is_minted), "{id}");
    }
    // `u-` alone is an item id, the user's (so never the agent's), and no minted one.
    let bare = ItemId::parse("u-").unwrap();
    assert!(bare.is_user() && !bare.is_minted() && !ItemId::parse("ux").unwrap().is_user());
}

/// §1.4's grammar: no indices, ids with `u-`, at most three segments.
#[test]
fn paths() {
    let f16 = "a".repeat(16);
    let i24 = "x".repeat(24);
    for good in ["title", "steps", "steps.0", "steps.0.label", "steps.u-k3x7qa.stop", "hunks.h1.lines", &f16, &format!("steps.{i24}.{f16}")]
    {
        assert!(Path::parse(good).is_some(), "{good}");
    }
    for bad in [
        "",
        "steps[0]",
        "steps.0.label.x",
        "steps..label",
        "steps.",
        ".steps",
        "Steps",
        "step2",
        "steps.Fix",
        "steps.-x",
        &format!("{f16}a"),
        &format!("steps.{i24}x"),
        &format!("steps.x.{f16}a"),
        "steps.x.la-bel",
    ] {
        assert_eq!(Path::parse(bad), None, "{bad}");
    }
    let p = Path::parse("steps.0.label").unwrap();
    assert_eq!((p.head(), p.item(), p.field()), ("steps", Some("0"), Some("label")));
    // "No indices": `steps.0` names the step whose id is `0`.
    let v = ok(json!([{"op":"patch","id":"plan","set":{"steps.0.label":"First"}}]));
    assert_eq!(v[0]["set"], json!({"steps.0.label":"First"}));
}

#[test]
fn a_call_is_1_to_16_ops_in_8_kib_and_nests_6_deep_per_op() {
    let one = json!({"op":"remove","id":"x"});
    assert_eq!(ok(Value::Array(vec![one.clone(); 16])).as_array().unwrap().len(), 16);
    assert_eq!(refused(Value::Array(vec![one.clone(); 17])), "ops: at most 16 (got 17); split them over calls");
    assert_eq!(refused(json!([])), "ops: at least 1 op");
    assert_eq!(parse_call(&json!({})).unwrap_err().to_string(), "ops: required: a list of 1 to 16 ops");
    assert_eq!(parse_call(&json!({"ops":[one], "x":1})).unwrap_err().to_string(), "x: unknown parameter; a call takes ops only");
    assert_eq!(parse_call(&json!([1])).unwrap_err().to_string(), "input: must be a JSON object {\"ops\":[…]}");
    // A proposed diff's lines sit 6 deep in their op: the deepest a kind goes.
    let deep = json!([{"op":"put","id":"d","kind":"diff","body":{"file":"f","state":"proposed","hunks":[{"id":"h","lines":["x"]}]}}]);
    ok(deep);
    let deeper = json!([{"op":"put","id":"d","kind":"diff","body":{"file":"f","state":"proposed","hunks":[{"id":"h","lines":[["x"]]}]}}]);
    assert_eq!(refused(deeper), "input: nested too deep: an op nests at most 6 levels");
}

#[test]
fn put_fills_its_place_and_its_defaults() {
    let v = ok(json!([{"op":"put","id":"s","kind":"status","body":{"text":"Bisecting"}}]));
    assert_eq!(v[0], json!({"op":"put","id":"s","kind":"status","target":"transcript","body":{"text":"Bisecting","state":"working"}}));
    let v = ok(json!([{"op":"put","id":"m","kind":"metric","slot":"end","title":"","body":{"label":"L","value":"1","unit":""}}]));
    assert_eq!(
        v[0],
        json!({"op":"put","id":"m","kind":"metric","target":"rail","slot":"end","body":{"label":"L","value":"1","state":"neutral"}})
    );
    let v = ok(json!([{"op":"put","id":"m","kind":"note","target":"rail","body":{"text":"a\r\nb\tc"}}]));
    assert_eq!(v[0]["slot"], "upper");
    assert_eq!(v[0]["body"]["text"], "a\nb c");
    assert_eq!(
        refused(json!([{"op":"put","id":"m","kind":"note","target":"transcript","slot":"end","body":{"text":"x"}}])),
        "ops[0].slot: a slot places a block in the rail; leave it out, or use target rail"
    );
    assert_eq!(
        refused(json!([{"op":"put","id":"m","kind":"note","at":{"after":"m"},"body":{"text":"x"}}])),
        "ops[0].at.after: names this block itself; name another, or leave at out"
    );
    assert_eq!(
        refused(json!([{"op":"put","id":"m","kind":"note","at":"top","body":{"text":"x"}}])),
        "ops[0].at: \"top\" is not in blocks/1; use {\"after\":\"<id>\"}, or leave at out to place it here"
    );
    assert_eq!(
        refused(json!([{"op":"put","id":"card:c4f9x2a","kind":"note","body":{"text":"x"}}])),
        "ops[0].id: a card is made by cards_ui; replace a finished card instead"
    );
    assert_eq!(
        refused(json!([{"op":"put","id":"g","kind":"group","body":{"members":["a","b"]}}])),
        "ops[0].kind: a group is made by merge"
    );
    assert_eq!(refused(json!([{"op":"put","id":"n","kind":"note"}])), "ops[0].body: required: the note's body");
}

/// §1.5: `stop` is the user's, and `u-` ids are too.
#[test]
fn the_agent_never_names_stop_or_mints_u_ids() {
    let plan = |step: Value| json!([{"op":"put","id":"p","kind":"plan","body":{"steps":[step]}}]);
    assert_eq!(
        refused(plan(json!({"id":"a","label":"A","stop":false}))),
        "ops[0].body.steps[0].stop: stop is the user's: an agent op can't name it"
    );
    assert_eq!(
        refused(plan(json!({"id":"u-k3x7qa","label":"A"}))),
        "ops[0].body.steps[0].id: \"u-k3x7qa\" starts with u-, which is the user's; pick another id"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"p","set":{"steps.a.stop":true}}])),
        "ops[0].set.\"steps.a.stop\": stop is the user's: an agent op can't name it"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"p","append":{"steps":[{"id":"u-aaaaaa","label":"A"}]}}])),
        "ops[0].append.steps[0].id: \"u-aaaaaa\" starts with u-, which is the user's; pick another id"
    );
    // The agent may patch the user's items.
    ok(json!([{"op":"patch","id":"p","set":{"steps.u-k3x7qa.label":"Renamed"},"state":{"u-k3x7qa":"done"}}]));
}

/// §1.3: `state` sugar is `set` on the main list, read from the words.
#[test]
fn state_sugar_is_set_on_the_main_list() {
    let v = ok(json!([{"op":"patch","id":"plan","state":{"fix":"done","test":"doing"}}]));
    assert_eq!(v[0], json!({"op":"patch","id":"plan","set":{"steps.fix.state":"done","steps.test.state":"doing"}}));
    // The same patch written out is the same normalized op.
    assert_eq!(ok(json!([{"op":"patch","id":"plan","set":{"steps.test.state":"doing","steps.fix.state":"done"}}])), v);
    assert_eq!(
        refused(json!([{"op":"patch","id":"p","state":{"fix":"finished"}}])),
        "ops[0].state.fix: \"finished\" is not a plan state (todo, doing, done, failed, skipped)"
    );
    // §1.6's example refusal, exactly.
    assert_eq!(
        refused(json!([{"op":"remove","id":"x"},{"op":"patch","id":"plan","state":{"fix":"done"},"set":{"steps.fix.state":"failed"}}])),
        "ops[1]: \"steps.fix.state\" is named twice (state and set)"
    );
}

/// §3.4: a hunk's keep or drop is the user's. The agent writes a hunk
/// `open` (in a put, a replace or an append) and never sets its state: not
/// by `set`, not by sugar, not even back to `open`. The user's edit keeps
/// and drops.
#[test]
fn a_hunks_keep_or_drop_is_the_users() {
    let teach = "a hunk's state is the user's keep or drop; an agent can't set it (remove the hunk to withdraw it)";
    let hunk = |state: &str| json!({"id":"h1","lines":["+x"],"state":state});
    let diff = |state: &str| json!({"file":"f","state":"proposed","hunks":[hunk(state)]});
    for w in ["kept", "dropped"] {
        let user = format!("\"{w}\" is the user's keep or drop; an agent's hunk is open, so leave state out");
        assert_eq!(refused(json!([{"op":"patch","id":"d","set":{"hunks.h1.state":w}}])), format!("ops[0].set.\"hunks.h1.state\": {teach}"));
        assert_eq!(refused(json!([{"op":"patch","id":"d","state":{"h1":w}}])), format!("ops[0].state.h1: {teach}"));
        assert_eq!(refused(json!([{"op":"put","id":"d","kind":"diff","body":diff(w)}])), format!("ops[0].body.hunks[0].state: {user}"));
        assert_eq!(refused(json!([{"op":"replace","id":"d","kind":"diff","body":diff(w)}])), format!("ops[0].body.hunks[0].state: {user}"));
        assert_eq!(refused(json!([{"op":"patch","id":"d","append":{"hunks":[hunk(w)]}}])), format!("ops[0].append.hunks[0].state: {user}"));
    }
    // Not even back to open, and not beside a plan's tick.
    assert_eq!(
        refused(json!([{"op":"patch","id":"d","set":{"hunks.h1.state":"open"}}])),
        format!("ops[0].set.\"hunks.h1.state\": {teach}")
    );
    assert_eq!(refused(json!([{"op":"patch","id":"x","state":{"fix":"done","h1":"kept"}}])), format!("ops[0].state.h1: {teach}"));
    assert_eq!(
        refused(json!([{"op":"put","id":"d","kind":"diff","body":diff("applied")}])),
        "ops[0].body.hunks[0].state: \"applied\" is not a hunk state; an agent's hunk is open, so leave state out"
    );
    // An agent's hunk is open, given or not; the rest of a diff stays the agent's.
    for v in [
        ok(json!([{"op":"put","id":"d","kind":"diff","body":diff("open")}])),
        ok(json!([{"op":"put","id":"d","kind":"diff","body":{"file":"f","state":"proposed","hunks":[{"id":"h1","lines":["+x"]}]}}])),
    ] {
        assert_eq!(v[0]["body"]["hunks"][0]["state"], "open");
    }
    let v = ok(
        json!([{"op":"patch","id":"d","append":{"hunks":[hunk("open")],"hunks.h2.lines":["+y"]},"set":{"state":"written","hunks.h2.at":"f()"}}]),
    );
    assert_eq!(v[0]["append"]["hunks"][0]["state"], "open");
    let p = Path::parse("hunks.h1.state").unwrap();
    assert_eq!(set_spec(Kind::Diff, Author::Agent, &p), Some(Spec::UserOnly { teach }));
    // The user keeps and drops, by an edit; never opens.
    let e = edit(json!({"set":{"hunks.h1.state":"kept","hunks.h2.state":"dropped"}}));
    assert_eq!(e.admit(Kind::Diff), Ok(()));
    assert!(e.admit(Kind::Plan).is_err());
    let text = message::build(&e, &Summary { title: None, back_to: None, pieces: vec!["keep h1", "drop h2"] });
    assert_eq!(message::parse(&text).as_ref(), Some(&e));
    let of = Tag::parse("bab3b9916631463b1935420c710c9287").unwrap();
    let open = Edit::new(BlockId::parse("d").unwrap(), 1, of, &json!({"set":{"hunks.h1.state":"open"}})).unwrap_err();
    assert_eq!(open.to_string(), "set.\"hunks.h1.state\": \"open\" is not a keep or a drop (kept, dropped)");
    // The fold's per-path check says the same: a keep fits the user's diff, not the agent's.
    assert!(e.parts.fits(Kind::Diff, Author::User).is_ok());
    assert_eq!(e.parts.fits(Kind::Diff, Author::Agent).unwrap_err().to_string(), format!("hunks.h1.state: {teach}"));
}

/// §1.4's overlap rule, every clause.
#[test]
fn the_overlap_rule() {
    let p = |parts: Value| {
        let mut op = json!({"op":"patch","id":"plan"});
        op.as_object_mut().unwrap().extend(parts.as_object().unwrap().clone());
        refused(json!([op]))
    };
    assert_eq!(p(json!({"remove":["steps.a","steps.a"]})), "ops[0]: \"steps.a\" is named twice (remove)");
    assert_eq!(
        p(json!({"append":{"steps":[{"id":"a","label":"A"},{"id":"a","label":"B"}]}})),
        "ops[0]: \"steps.a\" is named twice (append)"
    );
    assert_eq!(
        p(json!({"append":{"steps":[{"id":"a","label":"A"}]},"remove":["steps.a"]})),
        "ops[0]: \"steps.a\" is named twice (remove and append)"
    );
    assert_eq!(
        p(json!({"remove":["steps.a"],"set":{"steps.a.label":"A"}})),
        "ops[0]: \"steps.a.label\" sets a field of an item the patch removes"
    );
    assert_eq!(
        p(json!({"remove":["steps.a"],"state":{"a":"done"}})),
        "ops[0]: \"steps.a.state\" sets a field of an item the patch removes"
    );
    assert_eq!(
        p(json!({"append":{"steps":[{"id":"a","label":"A"}]},"set":{"steps.a.note":"n"}})),
        "ops[0]: \"steps.a.note\" sets a field of an item the patch appends"
    );
    let d = |parts: Value| {
        let mut op = json!({"op":"patch","id":"d"});
        op.as_object_mut().unwrap().extend(parts.as_object().unwrap().clone());
        refused(json!([op]))
    };
    assert_eq!(
        d(json!({"remove":["hunks.h1"],"append":{"hunks.h1.lines":["+x"]}})),
        "ops[0]: \"hunks.h1.lines\" appends to an item the patch removes"
    );
    assert_eq!(
        d(json!({"append":{"hunks":[{"id":"h1","lines":["+x"]}],"hunks.h1.lines":["+y"]}})),
        "ops[0]: \"hunks.h1.lines\" appends to an item the patch appends"
    );
    assert_eq!(p(json!({"order":{"steps":["a","b","a"]}})), "ops[0].order.steps[2]: \"a\" is listed twice");
    // Ordering an item the patch appends, or one it removes, isn't an overlap:
    // append runs before order, and order ignores ids that aren't live.
    ok(json!([{"op":"patch","id":"plan","append":{"steps":[{"id":"c","label":"C"}]},"order":{"steps":["c","a"]},"remove":["steps.b"]}]));
    // Fields of one item, set together, aren't an overlap either.
    ok(json!([{"op":"patch","id":"plan","set":{"steps.a.label":"A","steps.a.note":"n"},"state":{"a":"done"}}]));
    assert_eq!(p(json!({})), "ops[0]: a patch needs set, append, remove, order or state");
    assert_eq!(p(json!({"set":{}})), "ops[0]: a patch needs set, append, remove, order or state");
}

/// A patch names no kind: one kind must take every path, with its value.
#[test]
fn a_patch_fits_one_kind() {
    // `text` is a status's or a note's: 81 characters fit only a note; and
    // it keeps its line breaks until the fold knows which.
    let long = "x".repeat(81);
    ok(json!([{"op":"patch","id":"n","set":{"text":long}}]));
    assert_eq!(ok(json!([{"op":"patch","id":"n","set":{"text":"a\nb"}}]))[0]["set"]["text"], "a\nb");
    assert_eq!(
        refused(json!([{"op":"patch","id":"n","set":{"text":"x".repeat(1501)}}])),
        "ops[0].set.text: at most 1500 characters (got 1501); split it over notes"
    );
    // A status's text with a detail: a status, so 81 characters don't fit.
    assert_eq!(
        refused(json!([{"op":"patch","id":"s","set":{"text":long,"detail":"d"}}])),
        "ops[0].set.text: at most 80 characters (got 81)"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","set":{"file":"f","steps.a.label":"A"}}])),
        "ops[0]: \"file\" and \"steps.a.label\" are paths of different kinds"
    );
    // `state` is four kinds' field: a word none of them has lists them all.
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","set":{"state":"finished"}}])),
        "ops[0].set.state: \"finished\" is not a state of a status (working, waiting, blocked, done), a diff (editing, proposed, written, reverted), a metric (ok, warn, bad, neutral) or a file (reading, editing, written, done)"
    );
    // A word that fits some kinds, with a path of another: the value is why.
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","set":{"state":"done","file":"f"}}])),
        "ops[0].set.state: \"done\" is not a diff state (editing, proposed, written, reverted)"
    );
    ok(json!([{"op":"patch","id":"x","set":{"state":"done","path":"f"}}]));
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","set":{"colour":"red"}}])),
        "ops[0].set.colour: \"colour\" is not a field of any kind"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","set":{"steps":"red"}}])),
        "ops[0].set.steps: \"steps\" is a list: append to it, remove its items or order it"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","set":{"steps.a":"red"}}])),
        "ops[0].set.\"steps.a\": names an item; set one of its fields, like steps.fix.label"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","append":{"notes":["x"]}}])),
        "ops[0].append.notes: \"notes\" is not a list of any kind (steps, hunks, hunks.<id>.lines, series)"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","remove":["series.a"]}])),
        "ops[0].remove[0]: \"series.a\" is not an item of a list with ids (steps.<id>, hunks.<id>)"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"x","order":{"series":["a"]}}])),
        "ops[0].order.series: \"series\" is not a list with ids (steps, hunks)"
    );
    // A group's only path is its title.
    ok(json!([{"op":"patch","id":"g","set":{"title":"Research"}}]));
    // Values: a progress, a line number, a series.
    let v = ok(
        json!([{"op":"patch","id":"s","set":{"progress":{"n":4,"of":7}}},{"op":"patch","id":"f","set":{"line":84}},{"op":"patch","id":"m","append":{"series":[1.0,0.5]}}]),
    );
    assert_eq!(v[2]["append"]["series"], json!([1, 0.5]));
    assert_eq!(refused(json!([{"op":"patch","id":"s","set":{"progress":{"n":8,"of":7}}}])), "ops[0].set.progress.n: 8 is more than of (7)");
}

#[test]
fn parts_fit_a_kind_and_an_author() {
    let Op::Patch(p) = &call(json!([{"op":"patch","id":"plan","set":{"steps.a.label":"A"},"state":{"a":"done"}}])).unwrap().ops[0] else {
        panic!()
    };
    assert_eq!(p.parts.fits(Kind::Plan, Author::Agent), Ok(()));
    assert_eq!(p.parts.fits(Kind::Diff, Author::Agent).unwrap_err().to_string(), "steps.a.label: not a path a diff takes here");
    // The user may rename a step, but not tick it done.
    assert_eq!(
        p.parts.fits(Kind::Plan, Author::User).unwrap_err().to_string(),
        "steps.a.state: \"done\" is not a skip or an unskip (todo, skipped)"
    );
    // A status's text, written with a line break: the fold folds it.
    let Op::Patch(p) = &call(json!([{"op":"patch","id":"s","set":{"text":"a\nb"}}])).unwrap().ops[0] else { panic!() };
    assert_eq!(p.parts.fits(Kind::Status, Author::Agent), Ok(()));
    assert_eq!(p.parts.fits(Kind::Note, Author::Agent), Ok(()));
    assert!(p.parts.fits(Kind::Plan, Author::Agent).is_err());
}

#[test]
fn collapse_merge_move_remove_replace() {
    assert_eq!(ok(json!([{"op":"collapse","id":"diff"}]))[0], json!({"op":"collapse","id":"diff","open":false}));
    assert_eq!(ok(json!([{"op":"collapse","ids":["t1","t2"],"open":true}]))[0], json!({"op":"collapse","ids":["t1","t2"],"open":true}));
    assert_eq!(
        ok(json!([{"op":"collapse","run":true,"summary":"Explored relay","open":false}]))[0],
        json!({"op":"collapse","run":true,"summary":"Explored relay"})
    );
    assert_eq!(refused(json!([{"op":"collapse","run":true}])), "ops[0].summary: required with run: the run's title");
    assert_eq!(
        refused(json!([{"op":"collapse","run":true,"id":"x","summary":"s"}])),
        "ops[0].run: folds this call's tool run; give it without id or ids"
    );
    assert_eq!(refused(json!([{"op":"collapse","run":true,"open":true,"summary":"s"}])), "ops[0].open: opens blocks, not a run");
    assert_eq!(refused(json!([{"op":"collapse","id":"x","summary":"s"}])), "ops[0].summary: titles a run; give it with run:true");
    assert_eq!(refused(json!([{"op":"collapse"}])), "ops[0].id: required: id, or ids, or run:true with a summary");
    assert_eq!(refused(json!([{"op":"collapse","id":"a","ids":["b"]}])), "ops[0].ids: give id or ids, not both");
    assert_eq!(refused(json!([{"op":"remove","ids":["a","b","a"]}])), "ops[0].ids[2]: \"a\" is listed twice");
    assert_eq!(refused(json!([{"op":"remove","ids":[]}])), "ops[0].ids: at least 1 id");
    assert_eq!(refused(json!([{"op":"merge","into":"g","ids":["a","g"]}])), "ops[0].ids[1]: is into; a group can't be its own member");
    ok(json!([{"op":"merge","into":"g","ids":["a"]}]));
    // A card among the members is the fold's to skip (§1.3), not the call's
    // to refuse: the merge keeps it, in place, and the next op stands.
    let v = ok(json!([{"op":"merge","into":"g","ids":["a","card:c4f9x2a","b"]},{"op":"collapse","id":"n"}]));
    assert_eq!(v[0], json!({"op":"merge","into":"g","ids":["a","card:c4f9x2a","b"]}));
    let Ok(b) = call(json!([{"op":"merge","into":"g","ids":["card:c4f9x2a"]}])) else { panic!() };
    let Op::Merge(m) = &b.ops[0] else { panic!() };
    assert_eq!(m.ids, [Ref::parse("card:c4f9x2a").unwrap()]);
    // Still refused: a card as into, a member twice, a malformed one, into among them.
    assert_eq!(
        refused(json!([{"op":"merge","into":"card:c4f9x2a","ids":["a"]}])),
        "ops[0].into: \"card:c4f9x2a\" names a card; only a block can be here"
    );
    assert_eq!(
        refused(json!([{"op":"merge","into":"g","ids":["card:c4f9x2a","a","card:c4f9x2a"]}])),
        "ops[0].ids[2]: \"card:c4f9x2a\" is listed twice"
    );
    assert_eq!(
        refused(json!([{"op":"merge","into":"g","ids":["a","card:C4F9X2A"]}])),
        "ops[0].ids[1]: \"card:C4F9X2A\" must be a block id (^[a-z][a-z0-9_-]{0,23}$) or card:<card id> for a finished card"
    );
    assert_eq!(
        refused(json!([{"op":"merge","into":"g","ids":["card:c4f9x2a","g"]}])),
        "ops[0].ids[1]: is into; a group can't be its own member"
    );
    assert_eq!(ok(json!([{"op":"move","id":"m","slot":"end"}]))[0], json!({"op":"move","id":"m","target":"rail","slot":"end"}));
    assert_eq!(ok(json!([{"op":"move","id":"plan"}]))[0], json!({"op":"move","id":"plan"}));
    assert_eq!(refused(json!([{"op":"move","id":"card:c4f9x2a","target":"rail"}])), "ops[0].target: cards don't go to the rail");
    ok(json!([{"op":"move","id":"card:c4f9x2a","at":{"after":"plan"}}]));
    // §1.3 (amended): replace names its kind, even to keep it.
    assert_eq!(
        refused(json!([{"op":"replace","id":"n","body":{"text":"Corrected conclusion"}}])),
        "ops[0].kind: required, even to keep the block's: plan, status, diff, note, metric or file"
    );
    assert_eq!(
        refused(json!([{"op":"put","id":"n","body":{"text":"x"}}])),
        "ops[0].kind: required: plan, status, diff, note, metric or file"
    );
    assert_eq!(
        refused(json!([{"op":"frobnicate"}])),
        "ops[0].op: \"frobnicate\" is not an op; use put, patch, replace, collapse, merge, move or remove"
    );
    assert_eq!(refused(json!([{"op":"move","id":"m","colour":"red"}])), "ops[0].colour: unknown key; a move takes id, target, slot and at");
}

#[test]
fn numbers_are_canonical() {
    let series = |xs: Value| -> String {
        let b = call(json!([{"op":"patch","id":"m","append":{"series":xs}}])).unwrap();
        let s = String::from_utf8(b.preimage()).unwrap();
        s[s.find("\"series\":").unwrap() + 9..s.find("]}").unwrap() + 1].to_string()
    };
    assert_eq!(
        series(json!([1.0, -0.0, 2, -3, 1.5, 0.00012, 0.00001, 1e-7, 1.5e-7, 123456.75, 999999999999999u64])),
        "[1,0,2,-3,1.5,0.00012,0.00001,1e-7,1.5e-7,123456.75,999999999999999]"
    );
    assert_eq!(
        refused(json!([{"op":"patch","id":"m","append":{"series":[1e15]}}])),
        "ops[0].append.series[0]: must be a number between -10^15 and 10^15 (got 1000000000000000.0)"
    );
    assert!(Num::parse(&json!(-999999999999999.9)).is_some() && Num::parse(&json!("1")).is_none());
    // 17 digits, read from text, back as written: every build reads a number
    // correctly rounded (serde_json's float_roundtrip), so normalizing is a
    // fixed point, and the desk's build and the extension's agree.
    let text = r#"{"ops":[{"op":"patch","id":"m","append":{"series":[0.0012000000000000001,0.30000000000000004,-123456789.12345679]}}]}"#;
    let b = parse_call(&crate::decode_json(text).unwrap()).unwrap();
    let Op::Patch(p) = &b.ops[0] else { unreachable!() };
    let Some(Appended::Series(xs)) = p.parts.append.values().next() else { unreachable!() };
    assert_eq!(xs.iter().map(|x| x.get()).collect::<Vec<_>>(), [0.00012 * 10.0, 0.1 + 0.2, -123456789.12345679]);
    let bytes = b.preimage();
    assert!(String::from_utf8(bytes.clone()).unwrap().contains("[0.0012000000000000001,0.30000000000000004,-123456789.12345679]"));
    assert_eq!(parse_call(&crate::decode_json(std::str::from_utf8(&bytes).unwrap()).unwrap()), Ok(b));
}

#[test]
fn receipts() {
    let b = call(json!([{"op":"remove","id":"x"},{"op":"collapse","id":"y"}])).unwrap();
    let r = b.receipt();
    let line = r.to_string();
    assert_eq!(line, format!("ok 2 ops (blocks/1 #{})", b.digest()));
    assert_eq!(Receipt::parse(&line), Some(r));
    assert!(r.binds(&b));
    // §1.6's example parses (its digest is made up, so it binds nothing here).
    let ex = Receipt::parse("ok 3 ops (blocks/1 #3f9a1c2e4b5d6f70)").unwrap();
    assert_eq!((ex.ops, ex.catalog, ex.digest.to_string().as_str()), (3, 1, "3f9a1c2e4b5d6f70"));
    assert!(!ex.binds(&b));
    // The widest: 16 ops of a nine-digit catalog fits 64 bytes.
    assert!(format!("ok 16 ops (blocks/999999999 #{})", b.digest()).len() <= RESULT_MAX_BYTES);
    let d = b.digest();
    for bad in [
        format!("ok 02 ops (blocks/1 #{d})"),
        format!("ok 0 ops (blocks/1 #{d})"),
        format!("ok 2 ops (blocks/01 #{d})"),
        format!("ok 2 ops (blocks/1 #{d}"),
        format!("ok 2 ops (blocks/1 #{})", &d.to_string()[..15]),
        format!("ok 2 ops (blocks/1 #{}) ", d),
        format!(" ok 2 ops (blocks/1 #{d})"),
        format!("ok 2 op (blocks/1 #{d})"),
        format!("ok 2 ops (cards/1 #{d})"),
        format!("ok 2 ops (blocks/1 #{})", d.to_string().to_uppercase()),
        "cards_block: ops[0]: nope".to_string(),
    ] {
        assert_eq!(Receipt::parse(&bad), None, "{bad}");
    }
    // Another catalog, another count, another digest: unbound.
    for other in [Receipt { catalog: 2, ..r }, Receipt { ops: 1, ..r }, Receipt { digest: ex.digest, ..r }] {
        assert!(!other.binds(&b), "{other}");
    }
    // A second line doesn't matter; the first is the receipt.
    assert_eq!(Receipt::parse(&format!("{line}\nmore")), Some(r));
}

#[test]
fn the_generation_and_the_tag() {
    let first = json!({
        "role": "user",
        "_synaps_at": "2026-11-26T00:00:00Z",
        "content": [{"type": "text", "text": "hi", "_synaps_note": 1, "cache": {"z": 1, "a": "é"}}],
        "id": "ignored"
    });
    let pre = Generation::preimage(&first);
    assert_eq!(pre, r#"{"content":[{"cache":{"a":"é","z":1},"text":"hi","type":"text"}],"role":"user"}"#);
    let g = Generation::of(&first);
    assert_eq!(g, Generation::of(&json!({"content":[{"type":"text","cache":{"a":"é","z":1},"text":"hi"}],"role":"user"})));
    assert_eq!(Generation::parse(&g.to_string()), Some(g));
    assert_eq!(Generation::preimage(&json!("not an object")), "{}");
    let id = Ref::parse("plan").unwrap();
    let pre = Tag::preimage(&g, &id, 7, "toolu_01", 2);
    assert_eq!(pre, format!("blocks/1\0{g}\0plan\x007\0toolu_01\x002").into_bytes());
    let t = Tag::new(&g, &id, 7, "toolu_01", 2);
    assert_eq!(t.to_string().len(), 32);
    assert_eq!(Tag::parse(&t.to_string()), Some(t));
    // Every input matters.
    for other in [
        Tag::new(&g, &id, 8, "toolu_01", 2),
        Tag::new(&g, &id, 7, "toolu_02", 2),
        Tag::new(&g, &id, 7, "toolu_01", 3),
        Tag::new(&g, &Ref::parse("plan2").unwrap(), 7, "toolu_01", 2),
        Tag::new(&Generation::of(&json!({"role":"user","content":"x"})), &id, 7, "toolu_01", 2),
    ] {
        assert_ne!(other, t);
    }
    for bad in [
        "",
        "BAB3B9916631463B1935420C710C9287",
        "bab3b9916631463b1935420c710c928",
        "bab3b9916631463b1935420c710c92870",
        "gab3b9916631463b1935420c710c9287",
    ] {
        assert_eq!(Tag::parse(bad), None, "{bad}");
    }
}

fn notices(list: &[(&str, u32)]) -> Vec<Notice> {
    list.iter().map(|(id, v)| Notice { id: BlockId::parse(id).unwrap(), version: *v }).collect()
}

#[test]
fn notice_lines_and_trailers() {
    let line = NoticeLine::new(Visibility::Dismissed, notices(&[("tests", 3)])).unwrap();
    assert_eq!(line.to_string(), "[block tests v3 dismissed]");
    assert_eq!(NoticeLine::parse("[block tests v3 dismissed]"), Some(line.clone()));
    let eight: Vec<(&str, u32)> = ["a", "b", "c", "d", "e", "f", "g", "h"].iter().map(|id| (*id, 999_999_999)).collect();
    let full = NoticeLine::new(Visibility::Shown, notices(&eight)).unwrap();
    assert_eq!(NoticeLine::parse(&full.to_string()), Some(full.clone()));
    let mut nine = eight.clone();
    nine.push(("i", 1));
    assert_eq!(NoticeLine::new(Visibility::Shown, notices(&nine)), None);
    assert_eq!(NoticeLine::parse(&full.to_string().replace(" shown]", ", i v1 shown]")), None);
    assert_eq!(NoticeLine::new(Visibility::Shown, notices(&[("a", 0)])), None);
    assert_eq!(NoticeLine::new(Visibility::Shown, notices(&[("a", 1_000_000_000)])), None);
    assert_eq!(NoticeLine::new(Visibility::Shown, notices(&[("a", 1), ("a", 2)])), None);
    assert_eq!(NoticeLine::new(Visibility::Shown, Vec::new()), None);
    for bad in [
        "[block tests v0 dismissed]",
        "[block tests v03 dismissed]",
        "[block tests v1000000000 dismissed]",
        "[block tests v3 hidden]",
        "[block tests v3 dismissed] ",
        "[block tests v3 dismissed]\r",
        "[block tests v3  dismissed]",
        "[block tests v3,plan v4 dismissed]",
        "[block tests v3, dismissed]",
        "[block Tests v3 dismissed]",
        "[block card:c4f9x2a v3 dismissed]",
        "[block tests 3 dismissed]",
        "[block  dismissed]",
        "[card tests v3 dismissed]",
        "block tests v3 dismissed",
    ] {
        assert_eq!(NoticeLine::parse(bad), None, "{bad:?}");
    }
    let shown = NoticeLine::new(Visibility::Shown, notices(&[("plan", 4)])).unwrap();
    let t = Trailer::new(vec![line.clone(), shown.clone()]).unwrap();
    assert_eq!(t.text(), "\n[block tests v3 dismissed]\n[block plan v4 shown]");
    assert_eq!(Trailer::new(vec![line.clone(), line.clone()]), None, "one line of each kind");
    assert_eq!(
        Trailer::new(vec![line.clone(), NoticeLine::new(Visibility::Shown, notices(&[("tests", 4)])).unwrap()]),
        None,
        "an id in both"
    );
    assert_eq!(Trailer::new(vec![line.clone(), shown.clone(), line.clone()]), None);
}

#[test]
fn split_reads_only_final_notice_lines() {
    let (body, t) = message::split("run the tests\n[block tests v3 dismissed]");
    assert_eq!((body, t.lines.len()), ("run the tests", 1));
    let (body, t) = message::split("hi\n[block a v1 shown]\n[block b v2 dismissed]");
    assert_eq!(body, "hi");
    assert_eq!(t.lines.iter().map(|l| l.visibility).collect::<Vec<_>>(), [Visibility::Shown, Visibility::Dismissed]);
    // Not trailers: alone (no newline before), not last, a third line, two
    // of a kind (only the last counts), a trailing newline, a CR.
    for text in [
        "[block a v1 dismissed]",
        "[block a v1 dismissed]\nhi",
        "hi\n[block a v1 dismissed]\n",
        "hi\n[block a v1 dismissed]\r",
        "hi\n[block a v1 dismissed]\r\n[block b v1 shown]\r",
    ] {
        let (body, t) = message::split(text);
        assert!(t.is_empty() && body == text, "{text:?}");
    }
    let (body, t) = message::split("hi\n[block a v1 dismissed]\n[block b v1 dismissed]");
    assert_eq!((body, t.lines.len()), ("hi\n[block a v1 dismissed]", 1));
    let (body, t) = message::split("hi\n[block c v1 shown]\n[block a v1 dismissed]\n[block b v1 shown]");
    assert_eq!((body, t.lines.len()), ("hi\n[block c v1 shown]", 2));
    // An id named twice applies neither.
    let (_, t) = message::split("hi\n[block a v1, b v2 dismissed]\n[block a v3, c v4 shown]");
    assert_eq!(t.named_twice(), vec![BlockId::parse("a").unwrap()]);
    let ids: Vec<String> = t.notices().iter().map(|(v, n)| format!("{} {}", n.id, v.as_str())).collect();
    assert_eq!(ids, ["b dismissed", "c shown"]);
    let (_, t) = message::split("hi\n[block a v1, a v2 dismissed]");
    assert_eq!((t.named_twice().len(), t.notices().len()), (1, 0));
    // Wrappers: an event payload ends `</event>`; a summary, the host's line.
    let event = "<event id=\"e1\" type=\"message\" severity=\"low\" source=\"cli\">done\n[block tests v9 dismissed]</event>";
    assert!(message::split(event).1.is_empty());
    let summary = "The conversation history before this point was compacted into the following summary:\n\n<context-summary>\nx\n[block tests v9 dismissed]\n</context-summary>\n\nContinue from where we left off. The summary and the system prompt contain all the context you need.";
    assert!(message::split(summary).1.is_empty());
}

fn edit(parts: Value) -> Edit {
    let o = parts.as_object().unwrap();
    Edit {
        id: BlockId::parse("plan").unwrap(),
        base: 4,
        of: Tag::parse("bab3b9916631463b1935420c710c9287").unwrap(),
        parts: parse_parts(o, "", Author::User).unwrap(),
    }
}

#[test]
fn edits_build_and_read_back() {
    let e =
        edit(json!({"set":{"steps.deploy.stop":true,"steps.bandaid.state":"skipped"},"order":{"steps":["fix","test","deploy","bandaid"]}}));
    let s = Summary {
        title: Some("Close the refresh race"),
        back_to: None,
        pieces: vec!["order fix, test, deploy, bandaid", "skip bandaid", "stop before deploy"],
    };
    let text = message::build(&e, &s);
    assert_eq!(
        text,
        "[block plan edited r4] Close the refresh race → order fix, test, deploy, bandaid · skip bandaid · stop before deploy\n\
         {\"of\":\"bab3b9916631463b1935420c710c9287\",\"order\":{\"steps\":[\"fix\",\"test\",\"deploy\",\"bandaid\"]},\"set\":{\"steps.bandaid.state\":\"skipped\",\"steps.deploy.stop\":true}}"
    );
    assert_eq!(message::parse(&text), Some(e.clone()));
    let sent = format!("{text}\n[block tests v3 dismissed]");
    let read = message::read(&sent);
    assert_eq!((read.body, read.edit.as_ref(), read.trailer.lines.len()), (text.as_str(), Some(&e), 1));
    assert_eq!(e.admit(Kind::Plan), Ok(()));
    assert!(e.admit(Kind::Note).is_err() && e.admit(Kind::Diff).is_err());
    // No title: the id. Pieces cut at 40; the line at 200.
    let long = "p".repeat(41);
    let s = Summary { title: None, back_to: Some(1), pieces: vec![&long] };
    let line1 = message::build(&e, &s).split('\n').next().unwrap().to_string();
    assert_eq!(line1, format!("[block plan edited r4] plan → back to r1: {}…", "p".repeat(39)));
    let many: Vec<String> = (0..20).map(|i| format!("piece {i}")).collect();
    let s = Summary { title: Some("T"), back_to: None, pieces: many.iter().map(String::as_str).collect() };
    let text = message::build(&e, &s);
    let line1 = text.split('\n').next().unwrap();
    assert!(line1.chars().count() == 200 && line1.ends_with('…'));
    assert_eq!(message::parse(&text), Some(e));
}

#[test]
fn edit_look_alikes_are_ordinary_text() {
    let e = edit(json!({"set":{"steps.bandaid.state":"skipped"}}));
    let text = message::build(&e, &Summary { title: Some("T"), back_to: None, pieces: vec!["skip bandaid"] });
    let (l1, l2) = text.split_once('\n').unwrap();
    assert!(message::parse(&text).is_some());
    let of = "\"of\":\"bab3b9916631463b1935420c710c9287\"";
    let set = l2.strip_prefix(&format!("{{{of},")).unwrap().strip_suffix('}').unwrap();
    // §3.3 (amended): line 2 is read for what it says. Any JSON spelling of
    // the same patch is the same edit; the builder writes one.
    for same in [
        format!("{{{set},{of}}}"),
        format!("{l2} "),
        format!(" {l2}"),
        l2.replace(":\"skipped\"", ": \"skipped\"").replace(',', " , "),
        l2.replace("skipped", "skip\\u0070ed"),
        l2.replace("\"of\"", "\"\\u006f\\u0066\""),
    ] {
        assert_eq!(message::parse(&format!("{l1}\n{same}")).as_ref(), Some(&e), "{same}");
    }
    // Shady's example: an escape the builder doesn't write, in a note's text.
    let cafe = edit(json!({"set":{"text":"café"}}));
    assert_eq!(cafe.patch_text(), "{\"of\":\"bab3b9916631463b1935420c710c9287\",\"set\":{\"text\":\"café\"}}");
    let typed = "[block plan edited r4] n → text café\n{\"of\":\"bab3b9916631463b1935420c710c9287\",\"set\":{\"text\":\"caf\\u00e9\"}}";
    assert_eq!(message::parse(typed), Some(cafe));
    for bad in [
        l1.to_string(),
        format!("{l1}\n{l2}\n"),
        format!("{l1}\n{l2}\t"),
        format!("{l1}\n{l2}\r"),
        format!("{l1}\n{}", l2.replace(':', ":\t")),
        format!("{l1}\n{}", l2.replace("skipped", "done")),
        format!("{l1}\n{}", l2.replace("\"set\"", "\"remove\":[\"steps.x\"],\"set\"")),
        format!("{l1}\n{}", l2.replace("bab3", "BAB3")),
        format!("{l1}\n{}", l2.replace(&format!("{of},"), "")),
        format!("{l1}\n[{l2}]"),
        format!("{l1}\n{}", l2.replace("set", "state")),
        l1.replace("r4", "r0") + "\n" + l2,
        l1.replace("r4", "r04") + "\n" + l2,
        l1.replace("r4", "r10000") + "\n" + l2,
        l1.replace("[block plan", "[block card:c4f9x2a") + "\n" + l2,
        l1.replace("edited", "changed") + "\n" + l2,
        "[block plan edited r4] \n".to_string() + l2,
        format!("{l1}\u{7}\n{l2}"),
        format!("[block plan edited r4] {}\n{l2}", "x".repeat(178)),
        format!("{l1}\n{{\"of\":\"bab3b9916631463b1935420c710c9287\"}}"),
        format!("{l1}\n{{\"of\":\"bab3b9916631463b1935420c710c9287\",\"set\":{{\"steps.a.label\":\"A\",\"steps.a.label\":\"B\"}}}}"),
        format!("{l1}\n{{\"of\":\"bab3b9916631463b1935420c710c9287\",\"set\":{{\"steps.fix.note\":\"n\"}}}}"),
        format!(
            "{l1}\n{{\"of\":\"bab3b9916631463b1935420c710c9287\",\"append\":{{\"steps\":[{{\"id\":\"fix\",\"label\":\"L\",\"state\":\"todo\"}}]}}}}"
        ),
    ] {
        assert_eq!(message::parse(&bad), None, "{bad:?}");
    }
    // Exactly 200 characters of line 1, and 201.
    let at = |n: usize| format!("[block plan edited r4] {}\n{l2}", "x".repeat(n - 23));
    assert!(message::parse(&at(200)).is_some() && message::parse(&at(201)).is_none());
    assert!(message::parse(&format!("[block plan edited r9999] T → x\n{l2}")).is_some());
}

/// A label of `b` bytes (1 to 400) in at most 100 characters.
fn label(b: usize) -> String {
    "😀".repeat(b / 4) + ["", "a", "é", "€"][b % 4]
}

/// Line 2's 8 KiB, at N and N+1: 20 appended steps whose labels fill it.
#[test]
fn line_2_is_at_most_8_kib() {
    let with = |bytes: &[usize]| -> Edit {
        let steps: Vec<Value> = bytes
            .iter()
            .enumerate()
            .map(|(i, &b)| json!({"id": ItemId::mint(i as u32).as_str(), "label": label(b), "state": "todo"}))
            .collect();
        edit(json!({"append":{"steps":steps}}))
    };
    let overhead = with(&[1; 20]).patch_text().len() - 20;
    let of_len = |len: usize| -> String {
        let mut need = len - overhead;
        let bytes: Vec<usize> = (0..20)
            .map(|i| {
                let b = (need - (19 - i)).min(400);
                need -= b;
                b
            })
            .collect();
        let e = with(&bytes);
        let text = message::build(&e, &Summary { title: None, back_to: None, pieces: vec!["add 20 steps"] });
        assert_eq!(text.split_once('\n').unwrap().1.len(), len);
        text
    };
    assert!(message::parse(&of_len(message::EDIT_PATCH_MAX_BYTES)).is_some());
    assert!(message::parse(&of_len(message::EDIT_PATCH_MAX_BYTES + 1)).is_none());
}
