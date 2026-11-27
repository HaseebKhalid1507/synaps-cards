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

fn ask(input: Value) -> Value {
    // The card's id as the extension shows it; any well-formed one will do.
    answer(
        &json!({"input": input, "result": "Card c4f9x2a is showing (cards/1 #0123456789abcdef). Options: …", "prefer": ["(12.50)", "after"]}),
    )
}

/// The answer policy: the first preferred option, as cards-spec writes the
/// message (its own `message::build`, after `parse_answer`).
#[test]
fn an_answer_picks_the_preferred_option() {
    let a = ask(json!({"question": "Negatives as?", "options": ["-12.50", "(12.50)"]}));
    assert_eq!(a["values"], json!({"choice": "(12.50)"}));
    let text = a["text"].as_str().unwrap();
    assert!(text.starts_with("[card c4f9x2a] Negatives as? → (12.50)\n"), "{text}");
    assert_eq!(text.lines().nth(1), Some(r#"{"choice":"(12.50)"}"#));
}

#[test]
fn with_nothing_preferred_the_default_then_the_first() {
    let d = ask(json!({"question": "Q", "options": ["a", "b"], "default": "b"}));
    assert_eq!(d["values"], json!({"choice": "b"}));
    let f = ask(json!({"question": "Q", "options": ["a", "b"]}));
    assert_eq!(f["values"], json!({"choice": "a"}));
    let m = ask(json!({"question": "Q", "options": ["before", "after"], "multi": true}));
    assert_eq!(m["values"], json!({"choice": ["after"]}));
}

#[test]
fn fields_take_the_first_preference_and_checks_are_ticked() {
    let a = ask(json!({"question": "Q", "fields": [{"id": "where", "label": "Where"}, {"id": "ok", "label": "OK?", "type": "check"}]}));
    assert_eq!(a["values"], json!({"fields": {"where": "(12.50)", "ok": true}}));
    assert!(a["text"].as_str().unwrap().contains("Where: (12.50) · OK?: yes"));
}

#[test]
fn a_result_that_shows_no_card_has_no_answer() {
    let a = answer(&json!({"input": {"question": "Q", "options": ["a"]}, "result": "cards_ui: refused", "prefer": []}));
    assert!(a["error"].is_string());
    let c = answer(&json!({"input": {"close": "c4f9x2a"}, "result": "Card c4f9x2a is showing", "prefer": []}));
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
