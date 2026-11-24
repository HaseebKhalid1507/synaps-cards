//! The crate root (errors, ids, the JSON ingress, map order) and `types`.

use super::*;

/// One line, bounded, nothing that can hide or reorder text.
pub(crate) fn safe_line(s: &str) -> bool {
    s.chars().count() <= ERROR_PATH_MAX + 2 + ERROR_MESSAGE_MAX
        && s.chars().all(|c| !c.is_control() && !sanitize::is_invisible(c) && !matches!(c, '\u{2028}' | '\u{2029}'))
}

/// Shady #439 M1: whatever a key holds, a refusal is one short line with
/// no control, bidi control, invisible character or newline.
#[test]
fn errors_are_one_safe_bounded_line() {
    let hostile = "\u{1b}[31m\n\u{202E}forged\u{200B}\u{85}";
    assert_eq!(key_path(hostile), r#""\u001b[31m\u000a\u202eforged\u200b\u0085""#);
    assert_eq!(key_path("blocks"), "blocks");
    assert_eq!(key_path("wait-for"), "wait-for");
    assert_eq!(key_path(""), r#""""#);
    assert_eq!(key_path("a\"b\\c"), r#""a\"b\\c""#);
    assert_eq!(key_path("\u{1F600}"), r#""\ud83d\ude00""#);
    assert_eq!(key_path(&"k".repeat(25)), format!("\"{}…\"", "k".repeat(24)));
    let e = Error::new(format!("x{hostile}"), format!("m{hostile}\r\n{}", "y".repeat(500)));
    let shown = e.to_string();
    assert!(safe_line(&shown), "{shown:?}");
    assert_eq!(e.path(), "x[31m forged");
    assert!(shown.chars().count() <= ERROR_PATH_MAX + 2 + ERROR_MESSAGE_MAX);
}

#[test]
fn card_ids() {
    for ok in ["c4f9x2a", "c000000", "czzzzzz", "c81kq0z"] {
        assert_eq!(CardId::parse(ok).unwrap().as_str(), ok);
        assert_eq!(CardId::parse(ok).unwrap().to_string(), ok);
    }
    for bad in [
        "",
        "c",
        "c4f9x2",
        "c4f9x2a0",
        "C4f9x2a",
        "c4F9x2a",
        "c4f9-2a",
        "c4f9_2a",
        "x4f9x2a",
        "s_3f2a91c0d4e2a871",
        "c4f9x2\u{e9}",
        " c4f9x2a",
    ] {
        assert_eq!(CardId::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn json_len_stops_at_the_cap() {
    let v = serde_json::json!({"a": "x".repeat(100)});
    let n = serde_json::to_vec(&v).unwrap().len();
    assert_eq!(json_len_within(&v, n), Some(n));
    assert_eq!(json_len_within(&v, n - 1), None);
}

/// Maps iterate sorted, or in insertion order with `preserve_order` (CI
/// runs both lanes: this test says which one ran). The helpers read either
/// the same way.
#[test]
fn keys_are_read_in_sorted_order_in_both_builds() {
    let v: serde_json::Value = serde_json::from_str(r#"{"zeta":1,"beta":{"y":[{"b":2,"a":1}],"x":0},"alpha":2}"#).unwrap();
    let o = v.as_object().unwrap();
    let built = o.keys().next().map(String::as_str);
    assert_eq!(built, Some(if cfg!(feature = "preserve_order") { "zeta" } else { "alpha" }));
    assert_eq!(first_key(o, |_, _| true).map(String::as_str), Some("alpha"));
    assert_eq!(first_key(o, |k, _| k != "alpha").map(String::as_str), Some("beta"));
    assert_eq!(first_key(o, |_, v| v.is_object()).map(String::as_str), Some("beta"));
    assert_eq!(sorted_json(&v), r#"{"alpha":2,"beta":{"x":0,"y":[{"a":1,"b":2}]},"zeta":1}"#);
}

/// Shady #439 L2: the raw-JSON ingress.
#[test]
fn decode_json_is_strict() {
    assert_eq!(
        decode_json(r#"{"a":[1,{"b":null}],"c":"\ud83d\ude00"}"#).unwrap(),
        serde_json::json!({"a": [1, {"b": null}], "c": "\u{1F600}"})
    );
    for (text, want) in [
        (r#"{"a":1,"a":2}"#, "json: the key a is given twice"),
        (r#"{"x":{"b":1,"b":1}}"#, "json: the key b is given twice"),
        (r#"[{"\u001b":1,"\u001b":2}]"#, r#"json: the key "\u001b" is given twice"#),
    ] {
        assert!(decode_json(text).unwrap_err().to_string().starts_with(want), "{text}");
    }
    for bad in [r#""\ud83d""#, r#""\ude00""#, r#""\ud83dx""#, "1e400", "-1e400", "{} {}", "{\"a\":1,}"] {
        let e = decode_json(bad).expect_err(bad);
        assert!(e.to_string().starts_with("json: "), "{e}");
    }
    assert!(decode_json(&format!("{}1{}", "[".repeat(128), "]".repeat(128))).is_err());
    assert!(decode_json(&format!("{}1{}", "[".repeat(127), "]".repeat(127))).is_ok());
    // Big integers stay numbers (u64, then f64); the model refuses them by type.
    assert!(decode_json("18446744073709551616").unwrap().is_f64());
    assert!(decode_json("-9223372036854775809").unwrap().is_f64());
}

/// The growth bound adds up as C1 documented it.
#[test]
fn doc_max_is_the_input_cap_plus_normalizing() {
    assert_eq!(NORMALIZE_GROWTH_MAX, 1396);
    assert_eq!(DOC_MAX_BYTES, 9588);
}

#[test]
fn answers_refuse_repeated_keys_and_field_ids() {
    for t in [
        r#"{"choice":"a","choice":"b"}"#,
        r#"{"note":"a","note":"b"}"#,
        r#"{"fields":{},"fields":{}}"#,
        r#"{"fields":{"a":"x","a":"y"}}"#,
        r#"{"choice":"a","extra":1}"#,
    ] {
        assert!(serde_json::from_str::<Answer>(t).is_err(), "{t}");
    }
    let a: Answer = serde_json::from_str(r#"{"fields":{"b":"1","a":true}}"#).unwrap();
    assert_eq!(a.to_json(), r#"{"fields":{"b":"1","a":true}}"#, "read in its own order");
    // Keys in the fixed order, whatever order they came in.
    let a: Answer = serde_json::from_str(r#"{"note":"n","choice":"c"}"#).unwrap();
    assert_eq!(a.to_json(), r#"{"choice":"c","note":"n"}"#);
    assert!(Answer::skip().is_skip() && !a.is_skip());
    assert_eq!(Answer::skip().to_json(), "{}");
}

/// `SCHEMA` is the design's §1.4 file, read by S2 as the tool's
/// `input_schema`.
#[test]
fn the_schema_is_json() {
    let v: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
    assert_eq!(v["additionalProperties"], false);
}
