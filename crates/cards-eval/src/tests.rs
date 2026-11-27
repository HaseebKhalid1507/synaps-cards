use super::*;
use cards_spec::block;

/// A stored history, as the daemon holds it: user messages, assistant
/// messages with block calls, and the results that bind them.
#[derive(Default)]
struct H {
    messages: Vec<Value>,
    tools: usize,
}

impl H {
    fn new() -> Self {
        let mut h = H::default();
        h.user("Fix the refresh race.");
        h
    }

    fn user(&mut self, text: &str) {
        self.messages.push(json!({"role": "user", "content": text}));
    }

    /// One assistant message with one call per `calls` entry, and the
    /// results that bind them unless `bound` is false: the tool ids.
    fn message(&mut self, calls: &[Value], bound: bool) -> Vec<String> {
        let ids: Vec<String> = calls
            .iter()
            .map(|_| {
                self.tools += 1;
                format!("toolu_{:03}", self.tools)
            })
            .collect();
        let uses: Vec<Value> = ids
            .iter()
            .zip(calls)
            .map(|(id, ops)| json!({"type": "tool_use", "id": id, "name": "cards_block", "input": {"ops": ops}}))
            .collect();
        self.messages.push(json!({"role": "assistant", "content": uses}));
        if bound {
            let results: Vec<Value> = ids
                .iter()
                .zip(calls)
                .map(|(id, ops)| {
                    let receipt = block::parse_call(&json!({"ops": ops})).unwrap().receipt().to_string();
                    json!({"type": "tool_result", "tool_use_id": id, "content": receipt})
                })
                .collect();
            self.messages.push(json!({"role": "user", "content": results}));
        }
        ids
    }

    /// One bound call: its tool id.
    fn call(&mut self, ops: Value) -> String {
        self.message(&[ops], true).remove(0)
    }

    fn report(&self) -> Value {
        report(&self.messages)
    }
}

fn put_plan(id: &str) -> Value {
    json!([{"op": "put", "id": id, "kind": "plan", "title": "Close the refresh race",
            "body": {"steps": [{"id": "fix", "label": "Fix"}, {"id": "test", "label": "Test"}]}}])
}

fn tick(id: &str, step: &str, state: &str) -> Value {
    json!([{"op": "patch", "id": id, "state": {step: state}}])
}

fn lines(r: &Value) -> &Vec<Value> {
    r["lines"].as_array().unwrap()
}

/// The agent's lines that did `did`, in order.
fn did<'a>(r: &'a Value, what: &str) -> Vec<&'a Value> {
    lines(r).iter().filter(|l| l["author"] == "agent" && l["did"] == what).collect()
}

/// §3.5's unseen patches: an op is on a dismissed block if the dismissal
/// had landed when the op did, not if the block ends dismissed.
#[test]
fn an_op_after_the_dismissal_is_unseen_and_one_before_it_is_not() {
    let mut h = H::new();
    h.call(put_plan("plan"));
    let before = h.call(tick("plan", "fix", "doing"));
    h.user("Keep going.\n[block plan v1 dismissed]");
    let after = h.call(tick("plan", "fix", "done"));
    let r = h.report();
    let patches = did(&r, "patch");
    assert_eq!(patches.len(), 2);
    assert_eq!((patches[0]["tool_id"].as_str(), &patches[0]["dismissed_before"]), (Some(before.as_str()), &json!(false)));
    assert_eq!((patches[1]["tool_id"].as_str(), &patches[1]["dismissed_before"]), (Some(after.as_str()), &json!(true)));
    // Unseen, but it applies (§3.5: patch applies, with no glow).
    assert_eq!(patches[1]["outcome"], "applied");
    assert_eq!(r["visibility"], json!([{"id": "plan", "dismissed": true, "version": 1}]));
}

/// The notices delivered: each a `notice` line with what it did, and the
/// record it left.
#[test]
fn notices_are_reported_with_their_outcome_and_record() {
    let mut h = H::new();
    h.call(put_plan("plan"));
    h.user("Next.\n[block plan v2 dismissed]");
    h.user("And then.\n[block plan v1 shown]");
    h.user("Show it.\n[block plan v3 shown]");
    let r = h.report();
    let n: Vec<(&str, Option<&str>, &Value)> = lines(&r)
        .iter()
        .filter(|l| l["did"] == "notice")
        .map(|l| (l["outcome"].as_str().unwrap(), l["why"].as_str(), &l["record"]))
        .collect();
    assert_eq!(
        n,
        [
            ("applied", None, &json!({"dismissed": true, "version": 2})),
            ("stale", Some("v1: v2 is later"), &json!({"dismissed": true, "version": 2})),
            ("applied", None, &json!({"dismissed": false, "version": 3})),
        ]
    );
    assert!(lines(&r).iter().filter(|l| l["did"] == "notice").all(|l| l["author"] == "user"));
}

/// §1.3: `put` on a live id does nothing, and the log says `exists`.
#[test]
fn a_put_on_a_live_id_is_exists() {
    let mut h = H::new();
    h.call(put_plan("plan"));
    h.call(put_plan("plan"));
    let r = h.report();
    let puts: Vec<&str> = did(&r, "put").iter().map(|l| l["outcome"].as_str().unwrap()).collect();
    assert_eq!(puts, ["applied", "exists"]);
}

/// A skipped op names its reason; a skipped path names its path and reason,
/// and its op still applies when another path did.
#[test]
fn skips_name_their_reason_and_a_skipped_path_its_path() {
    let mut h = H::new();
    h.call(put_plan("plan"));
    h.call(tick("nope", "fix", "done"));
    h.call(json!([{"op": "patch", "id": "plan", "set": {"steps.fix.label": "Fix it", "steps.gone.label": "x"}}]));
    h.call(json!([{"op": "patch", "id": "plan", "set": {"steps.gone.label": "x"}}]));
    let r = h.report();
    let p: Vec<(&str, Option<&str>, Option<&str>)> =
        did(&r, "patch").iter().map(|l| (l["outcome"].as_str().unwrap(), l["why"].as_str(), l["path"].as_str())).collect();
    assert!(p.contains(&("skipped", Some("NotThere"), None)), "{p:?}");
    assert!(p.contains(&("applied", None, None)), "{p:?}");
    assert!(p.contains(&("skipped_path", Some("NotThere"), Some("steps.gone.label"))), "{p:?}");
    assert!(p.contains(&("skipped", Some("Paths"), None)), "{p:?}");
}

/// The eval's own op fixture (`eval/tests/fixtures/`, its `Ops` tests) is
/// this fold's report of its history, exactly: ten ops whose board logged
/// twenty lines (partial patches, one skipped whole, four after a
/// dismissal), so the Python report is tested on what the board says.
#[test]
fn the_eval_op_fixture_is_this_folds_report() {
    let history: Value = serde_json::from_str(include_str!("../../../eval/tests/fixtures/ops-history.json")).unwrap();
    let want: Value = serde_json::from_str(include_str!("../../../eval/tests/fixtures/ops-fold.json")).unwrap();
    assert_eq!(report(messages(&history).unwrap()), want);
}

/// The live blocks, as the desk draws them, in creation order: a removed
/// one is gone, a dismissed one says so, and the open batch's is pending.
#[test]
fn the_live_blocks_are_reported_in_creation_order() {
    let mut h = H::new();
    h.call(put_plan("plan"));
    h.call(json!([{"op": "put", "id": "notes", "kind": "note", "title": "Notes", "body": {"text": "x"}}]));
    h.call(json!([{"op": "put", "id": "gone", "kind": "note", "title": "Gone", "body": {"text": "x"}}]));
    h.call(json!([{"op": "remove", "id": "gone"}]));
    h.user("Next.\n[block plan v1 dismissed]");
    h.message(&[json!([{"op": "put", "id": "later", "kind": "note", "title": "Later", "body": {"text": "x"}}])], false);
    let r = h.report();
    assert_eq!(
        r["blocks"],
        json!([
            {"id": "plan", "kind": "plan", "dismissed": true, "pending": false},
            {"id": "notes", "kind": "note", "dismissed": false, "pending": false},
            {"id": "later", "kind": "note", "dismissed": false, "pending": true},
        ])
    );
}

/// The board's log keeps 256 lines; the report keeps them all.
#[test]
fn every_line_of_a_long_history_is_reported() {
    let mut h = H::new();
    h.call(put_plan("plan"));
    let mut ids = Vec::new();
    for k in 0..300 {
        ids.push(h.call(tick("plan", "fix", if k % 2 == 0 { "doing" } else { "todo" })));
    }
    let r = h.report();
    let patches = did(&r, "patch");
    assert!(patches.len() > LOG_MAX);
    assert_eq!(patches.len(), 300);
    let got: Vec<&str> = patches.iter().map(|l| l["tool_id"].as_str().unwrap()).collect();
    assert_eq!(got, ids.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(r["log_overflow"], false);
}

/// Each op's line names its call, and its index in the call.
#[test]
fn a_line_names_its_call_and_its_op() {
    let mut h = H::new();
    let ids = h.message(
        &[put_plan("plan"), json!([{"op": "put", "id": "n", "kind": "note", "body": {"text": "hi"}}, {"op": "remove", "id": "plan"}])],
        true,
    );
    let r = h.report();
    let got: Vec<(&str, &str, u64)> =
        lines(&r).iter().map(|l| (l["tool_id"].as_str().unwrap(), l["did"].as_str().unwrap(), l["op"].as_u64().unwrap())).collect();
    assert_eq!(got, [(ids[0].as_str(), "put", 0), (ids[1].as_str(), "put", 0), (ids[1].as_str(), "remove", 1)]);
    assert_eq!(r["live"], 1);
}

/// A call with no result yet, at the history's end, draws pending; one
/// whose result doesn't bind never applies.
#[test]
fn the_open_batch_is_pending_and_an_unbound_call_applies_nothing() {
    let mut h = H::new();
    h.call(put_plan("plan"));
    let open = h.message(&[tick("plan", "fix", "doing")], false).remove(0);
    let r = h.report();
    let last = lines(&r).last().unwrap();
    assert_eq!((last["tool_id"].as_str(), &last["pending"]), (Some(open.as_str()), &json!(true)));

    let mut u = H::new();
    let t = u.message(&[put_plan("plan")], false).remove(0);
    u.messages.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": t, "content": "cards_block: refused"}]}));
    u.user("Next.");
    let r = u.report();
    assert_eq!(did(&r, "call").iter().map(|l| l["outcome"].as_str().unwrap()).collect::<Vec<_>>(), ["unbound"]);
    assert_eq!(r["live"], 0);
}

#[test]
fn messages_reads_an_array_or_an_object() {
    let m = json!([{"role": "user", "content": "hi"}]);
    assert_eq!(messages(&m).map(<[Value]>::len), Some(1));
    assert_eq!(messages(&json!({"messages": m})).map(<[Value]>::len), Some(1));
    assert_eq!(messages(&json!("no")), None);
}

fn ask_for(input: Value, pick: &[&str]) -> Value {
    // The card's id as the extension shows it; any well-formed one will do.
    answer(&json!({"input": input, "result": "Card c4f9x2a is showing (cards/1 #0123456789abcdef). Options: …", "pick": pick}))
}

fn ask(input: Value) -> Value {
    ask_for(input, &["(12.50)"])
}

/// The answer policy: the option the task names, as cards-spec writes the
/// message (its own `message::build`, after `parse_answer`).
#[test]
fn an_answer_picks_the_option_the_task_names() {
    let a = ask(json!({"question": "Negatives as?", "options": ["-12.50", "(12.50)"]}));
    assert_eq!(a["values"], json!({"choice": "(12.50)"}));
    let text = a["text"].as_str().unwrap();
    assert!(text.starts_with("[card c4f9x2a] Negatives as? → (12.50)\n"), "{text}");
    assert_eq!(text.lines().nth(1), Some(r#"{"choice":"(12.50)"}"#));
    let m = ask_for(json!({"question": "Q", "options": ["before", "after"], "multi": true}), &["after"]);
    assert_eq!(m["values"], json!({"choice": ["after"]}));
}

/// Shady's case: the old policy took the first label containing "hide",
/// "Don't hide zero-balance accounts". Negation and order change nothing.
#[test]
fn a_negation_or_another_order_never_picks_the_opposite() {
    for options in [["Don't hide zero-balance accounts", "Hide zero-balance accounts"], ["Hide zero-balance accounts", "Don't hide zero-balance accounts"]] {
        let a = ask_for(json!({"question": "Zeros?", "options": options}), &["Hide zero-balance accounts"]);
        assert_eq!(a["values"], json!({"choice": "Hide zero-balance accounts"}), "{options:?}");
    }
}

#[test]
fn a_label_is_compared_folded_and_may_go_on_after_the_pick() {
    for (label, pick, is) in [
        ("`Hide them.`", "Hide them", true),
        ("HIDE  them", "hide them", true),
        ("Hide them (recommended)", "Hide them", true),
        ("14 days — two weeks", "14 days", true),
        ("Hide", "Hide them", false),
        ("140 days", "14", false),
        ("Hide themes", "Hide them", false),
        ("Don't hide them", "Hide them", false),
        ("anything", "", false),
    ] {
        assert_eq!(label_is(label, pick), is, "{label:?} is {pick:?}");
    }
}

/// None or two options that are the pick: an interaction failure, never
/// the card's default or its first option.
#[test]
fn no_match_or_two_matches_is_an_interaction_failure() {
    let none = ask_for(json!({"question": "Zeros?", "options": ["Omit them", "Keep them"], "default": "Keep them"}), &["Hide them"]);
    assert_eq!(none["interaction"], true, "{none}");
    assert!(none["error"].as_str().unwrap().contains(r#"the card offers ["Omit them", "Keep them"]"#), "{none}");
    assert!(none.get("text").is_none());
    let two = ask_for(json!({"question": "Zeros?", "options": ["Hide them", "Hide them all"]}), &["Hide them"]);
    assert_eq!(two["interaction"], true, "{two}");
    assert!(two["error"].as_str().unwrap().starts_with("2 options are"), "{two}");
}

#[test]
fn fields_take_the_fill_and_checks_are_ticked() {
    let card = json!({"question": "Q", "fields": [{"id": "where", "label": "Where"}, {"id": "ok", "label": "OK?", "type": "check"}]});
    let a = answer(&json!({"input": card, "result": "Card c4f9x2a is showing (cards/1 #0123456789abcdef).", "fill": "Here"}));
    assert_eq!(a["values"], json!({"fields": {"where": "Here", "ok": true}}));
    assert!(a["text"].as_str().unwrap().contains("Where: Here · OK?: yes"));
    let b = answer(&json!({"input": card, "result": "Card c4f9x2a is showing (cards/1 #0123456789abcdef)."}));
    assert_eq!(b["values"]["fields"]["where"], AS_YOU_SEE_FIT);
}

#[test]
fn a_result_that_shows_no_card_has_no_answer() {
    let a = answer(&json!({"input": {"question": "Q", "options": ["a"]}, "result": "cards_ui: refused", "pick": ["a"]}));
    assert!(a["error"].is_string());
    let c = answer(&json!({"input": {"close": "c4f9x2a"}, "result": "Card c4f9x2a is showing", "pick": []}));
    assert!(c["error"].is_string());
}

#[test]
fn a_receipt_is_the_extensions_and_a_refusal_its_line() {
    let ok = receipt(&json!({"ops": [{"op": "remove", "id": "plan"}]}));
    assert_eq!(ok["ops"], 1);
    assert!(ok["ok"].as_str().unwrap().starts_with("ok 1 ops (blocks/1 #"));
    let bad = receipt(&json!({"ops": []}));
    assert!(bad["error"].is_string(), "{bad}");
}
