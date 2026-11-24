//! `parse_answer`: shapes, refusals, bounds at N and N+1, hostile keys
//! (ported from #75's `answer.rs`).

use super::*;
use serde_json::json;

use crate::tests::safe_line;

fn doc(v: Value) -> CardDoc {
    match parse_input(&v).unwrap() {
        Input::Create { doc, .. } => doc,
        Input::Close(_) => unreachable!(),
    }
}

fn quick() -> CardDoc {
    doc(
        json!({"question": "Plan ready. Go ahead?", "options": ["Approve", {"label": "Revise", "note": "What should change?", "hint": "send notes"}]}),
    )
}

fn form() -> CardDoc {
    doc(json!({"question": "Release details", "fields": [
        {"id": "version", "label": "Version", "value": "0.13.0"},
        {"id": "notes", "label": "Notes", "type": "long", "optional": true},
        {"id": "publish", "label": "Publish to crates.io", "type": "check", "value": true}]}))
}

fn err(d: &CardDoc, v: Value) -> String {
    parse_answer(d, &v).unwrap_err().to_string()
}

#[test]
fn shapes_in_answer_order() {
    let a = parse_answer(&form(), &json!({"fields": {"publish": true, "notes": "", "version": "0.13.0"}})).unwrap();
    assert_eq!(a.to_json(), r#"{"fields":{"version":"0.13.0","notes":"","publish":true}}"#);
    let a = parse_answer(&quick(), &json!({"note": "keep it", "choice": "Revise"})).unwrap();
    assert_eq!(a.to_json(), r#"{"choice":"Revise","note":"keep it"}"#);
    let multi = doc(json!({"question": "q", "options": ["a", "b", "c"], "multi": true}));
    assert_eq!(parse_answer(&multi, &json!({"choice": []})).unwrap().to_json(), r#"{"choice":[]}"#);
    assert_eq!(parse_answer(&multi, &json!({"choice": ["c", "a"]})).unwrap().to_json(), r#"{"choice":["c","a"]}"#);
    assert_eq!(Answer::skip().to_json(), "{}");
    // A stored answer reads back in its own order.
    let back: Answer = serde_json::from_str(r#"{"fields":{"version":"1","notes":"","publish":false}}"#).unwrap();
    assert_eq!(back.to_json(), r#"{"fields":{"version":"1","notes":"","publish":false}}"#);
    assert!(serde_json::from_str::<Answer>(r#"{"choice":"a","extra":1}"#).is_err());
    assert!(serde_json::from_str::<Answer>(r#"{"choice":"a","choice":"b"}"#).is_err());
}

#[test]
fn refusals() {
    let q = quick();
    assert_eq!(err(&q, json!({})), "choice: required");
    assert_eq!(err(&q, json!({"choice": "approve"})), "choice: not one of the option labels");
    assert_eq!(err(&q, json!({"choice": ["Approve"]})), "choice: must be an option label");
    assert_eq!(err(&q, json!({"choice": "Revise"})), "note: required for this option");
    assert_eq!(err(&q, json!({"choice": "Approve", "note": "x"})), "note: the chosen option takes no note");
    assert_eq!(err(&q, json!({"choice": "Revise", "note": ""})), "note: must not be empty");
    assert_eq!(err(&q, json!({"choice": "Revise", "note": "a\nb"})), "note: holds U+000A (a control character)");
    assert_eq!(err(&q, json!({"choice": "Approve", "fields": {}})), "fields: this card has no fields");
    assert_eq!(err(&q, json!({"choice": "Approve", "blocks": 1})), "blocks: unknown key; an answer has choice, note and fields");
    assert_eq!(err(&q, json!("Approve")), "values: must be an object");
    let f = form();
    assert_eq!(
        err(&f, json!({"fields": {"version": "1", "notes": ""}})),
        "fields.publish: missing; every field is sent (\"\" for an empty optional one)"
    );
    assert_eq!(err(&f, json!({"fields": {"version": "", "notes": "", "publish": true}})), "fields.version: required");
    assert_eq!(err(&f, json!({"fields": {"version": "1", "notes": "", "publish": "yes"}})), "fields.publish: a check takes true or false");
    assert_eq!(err(&f, json!({"fields": {"version": "1", "notes": "", "publish": true, "x": 1}})), "fields.x: no such field");
    assert_eq!(
        err(&f, json!({"fields": {"version": "a\tb", "notes": "", "publish": true}})),
        "fields.version: holds U+0009 (a control character)"
    );
    assert_eq!(
        err(&f, json!({"fields": {"version": "1", "notes": "a\u{1B}[31m", "publish": true}})),
        "fields.notes: holds U+001B (a control character)"
    );
    assert_eq!(err(&f, json!({"choice": "x", "fields": {}})), "choice: this card has no options");
    // `long` keeps `\n` and `\t`; user-typed bidi text is data, allowed.
    parse_answer(&f, &json!({"fields": {"version": "\u{202E}1", "notes": "a\n\tb", "publish": false}})).unwrap();
    let multi = doc(json!({"question": "q", "options": ["a", "b"], "multi": true}));
    assert_eq!(err(&multi, json!({"choice": ["a", "a"]})), "choice[1]: listed twice");
    assert_eq!(err(&multi, json!({"choice": "a"})), "choice: must be a list of option labels");
}

#[test]
fn value_bounds_at_n_and_n_plus_1() {
    let f = form();
    let with = |field: &str, s: String| {
        let mut v = json!({"fields": {"version": "1", "notes": "", "publish": true}});
        v["fields"][field] = json!(s);
        v
    };
    parse_answer(&f, &with("version", "x".repeat(TEXT_MAX))).unwrap();
    assert_eq!(err(&f, with("version", "x".repeat(TEXT_MAX + 1))), "fields.version: at most 200 characters (got 201)");
    parse_answer(&f, &with("notes", "\u{1F600}".repeat(LONG_MAX))).unwrap();
    assert_eq!(err(&f, with("notes", "x".repeat(LONG_MAX + 1))), "fields.notes: at most 2000 characters (got 2001)");
    let q = quick();
    parse_answer(&q, &json!({"choice": "Revise", "note": "x".repeat(NOTE_MAX)})).unwrap();
    assert_eq!(err(&q, json!({"choice": "Revise", "note": "x".repeat(NOTE_MAX + 1)})), "note: at most 500 characters (got 501)");
    parse_answer(&q, &json!({"choice": "Revise", "note": "x".repeat(NOTE_MIN)})).unwrap();
}

/// 8 KiB of values: 8192 accepted, 8193 refused.
#[test]
fn values_bytes_at_n_and_n_plus_1() {
    let d = doc(
        json!({"question": "q", "fields": (0..5).map(|i| json!({"id": format!("f{i}"), "label": "L", "type": "long"})).collect::<Vec<_>>()}),
    );
    let at = |bytes: usize| {
        let mut v = json!({"fields": {"f0": "", "f1": "", "f2": "", "f3": "", "f4": ""}});
        let mut left = bytes - serde_json::to_vec(&v).unwrap().len();
        for i in 0..5 {
            let take = left.min(LONG_MAX);
            v["fields"][format!("f{i}")] = json!("x".repeat(take));
            left -= take;
        }
        assert_eq!(serde_json::to_vec(&v).unwrap().len(), bytes);
        v
    };
    parse_answer(&d, &at(ANSWER_MAX_BYTES)).unwrap();
    assert_eq!(err(&d, at(ANSWER_MAX_BYTES + 1)), "values: over 8192 bytes");
}

/// Depth 3 / 4, and a value of any depth is refused before it is
/// serialized (Shady #439 M2).
#[test]
fn values_depth_at_n_and_n_plus_1() {
    let multi = doc(json!({"question": "q", "options": ["a", "b"], "multi": true}));
    parse_answer(&multi, &json!({"choice": ["a"]})).unwrap();
    assert_eq!(err(&multi, json!({"choice": [["a"]]})), "values: nested deeper than 3 levels");
    let mut deep = json!("a");
    for _ in 0..200_000 {
        deep = Value::Array(vec![deep]);
    }
    // Never cloned, dropped whole or put through `json!`: all recurse.
    let mut values = Map::new();
    values.insert("choice".into(), deep);
    let values = Value::Object(values);
    assert_eq!(parse_answer(&multi, &values).unwrap_err().to_string(), "values: nested deeper than 3 levels");
    let mut v = values;
    while let Some(inner) = match v {
        Value::Object(mut o) => o.remove("choice"),
        Value::Array(mut a) => a.pop(),
        _ => None,
    } {
        v = inner;
    }
}

/// Shady #439 M1: a client's hostile key never reaches the error raw.
#[test]
fn hostile_answer_keys() {
    let f = form();
    let k = "\u{1b}]0;x\u{7}\n\u{2066}";
    let mut top = json!({"fields": {"version": "1", "notes": "", "publish": true}});
    top[k] = json!(1);
    assert_eq!(err(&f, top), r#""\u001b]0;x\u0007\u000a\u2066": unknown key; an answer has choice, note and fields"#);
    let mut field = json!({"fields": {"version": "1", "notes": "", "publish": true}});
    field["fields"][k] = json!(1);
    let e = err(&f, field);
    assert_eq!(e, r#"fields."\u001b]0;x\u0007\u000a\u2066": no such field"#);
    assert!(safe_line(&e));
}
