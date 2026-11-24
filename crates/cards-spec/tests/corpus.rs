//! The fixture corpus (`fixtures/`) against cards-spec's public API: the
//! C1 corpus as #75 ported it, with v1.1's ids and envelope, and the new
//! answer messages. The goldens (`digest`, `envelope`, each `message` and
//! `text`) are written by `scripts/oracle.py`, a second implementation; the
//! docs and errors are C1's.

use std::path::PathBuf;

use cards_spec::message::{self, Kind, Reply};
use cards_spec::sanitize::{Lines, fold_key, refused_char, ui_sanitize};
use cards_spec::*;
use serde_json::{Value, json};

fn corpus() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures"))
}

fn read(name: &str) -> Value {
    let p = corpus().join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn read_dir(kind: &str) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = std::fs::read_dir(corpus().join(kind))
        .unwrap_or_else(|e| panic!("{kind}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            (name.clone(), read(&format!("{kind}/{name}.json")))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn bytes(v: &Value) -> usize {
    serde_json::to_vec(v).unwrap().len()
}

fn create(input: &Value) -> (CardDoc, bool) {
    match parse_input(input) {
        Ok(Input::Create { doc, wait }) => (doc, wait),
        other => panic!("{input}: {other:?}"),
    }
}

fn id(v: &Value) -> CardId {
    CardId::parse(v.as_str().unwrap()).unwrap()
}

fn reply(doc: &CardDoc, kind: &Value, values: &Value) -> Reply {
    match kind.as_str().unwrap() {
        "answer" => Reply::Answer(parse_answer(doc, values).unwrap()),
        "changed" => Reply::Change(parse_answer(doc, values).unwrap()),
        "skipped" => {
            assert_eq!(*values, json!({}));
            Reply::Skip
        }
        other => panic!("{other}"),
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Answer => "answer",
        Kind::Change => "changed",
        Kind::Skip => "skipped",
    }
}

/// A refusal is one line, at most 322 characters, with nothing that can
/// hide or reorder text (Shady #439 M1).
fn one_safe_line(e: &str) -> bool {
    e.chars().count() <= ERROR_PATH_MAX + 2 + ERROR_MESSAGE_MAX && ui_sanitize(e, Lines::Single) == e && refused_char(e).is_none()
}

/// The message reads back as the reply it was built from, and is two
/// lines within their bounds.
#[track_caller]
fn round_trip(card: &CardId, doc: &CardDoc, r: &Reply, text: &str, what: &str) {
    assert_eq!(message::build(card, doc, r), text, "{what}: built");
    let (line1, line2) = text.split_once('\n').unwrap();
    assert!(!line2.contains('\n') && line1.chars().count() <= MESSAGE_LINE_MAX, "{what}");
    let back = message::parse(text).unwrap_or_else(|| panic!("{what}: reads"));
    assert_eq!((&back.card, back.kind), (card, r.kind()), "{what}");
    assert_eq!(&back.validate(doc).unwrap(), r, "{what}: validates as built");
}

/// Every Appendix A example, every directory, and the counts at this port.
#[test]
fn the_corpus_is_whole() {
    let valid: Vec<String> = read_dir("valid").into_iter().map(|(n, _)| n).collect();
    for a in ["a1_", "a2_", "a3_", "a4_", "a5_"] {
        assert!(valid.iter().any(|n| n.starts_with(a)), "{a}");
    }
    let invalid: Vec<String> = read_dir("invalid").into_iter().map(|(n, _)| n).collect();
    for a in [
        "a_ten_options",
        "a_duplicate_label",
        "a_default_not_an_option",
        "a_note_with_multi",
        "a_field_id_uppercase",
        "a_check_value_string",
        "a_close_with_question",
        "a_unknown_key_blocks",
        "a_question_121_chars",
        "a_question_bidi_override",
    ] {
        assert!(invalid.iter().any(|n| n == a), "{a}");
    }
    let messages = (
        read("messages/round_trips.json")["cases"].as_array().unwrap().len(),
        read("messages/look_alikes.json")["cases"].as_array().unwrap().len(),
    );
    assert_eq!((valid.len(), invalid.len(), read_dir("raw").len(), messages), (16, 34, 12, (10, 31)));
}

#[test]
fn valid_fixtures() {
    for (name, f) in read_dir("valid") {
        let parsed = parse_input(&f["input"]).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Some(close) = f.get("close") {
            assert_eq!(parsed, Input::Close(id(close)), "{name}");
            let e = Envelope::closed(id(close));
            assert_eq!(json!(e.to_string()), f["envelope"], "{name}: the close result");
            assert_eq!(Envelope::parse(f["envelope"].as_str().unwrap()), Some(e), "{name}");
            continue;
        }
        let Input::Create { doc, wait } = parsed else { panic!("{name}: not a create") };
        assert_eq!(serde_json::to_value(&doc).unwrap(), f["doc"], "{name}: normalized doc");
        let stored: CardDoc = serde_json::from_value(f["doc"].clone()).unwrap();
        assert_eq!(stored, doc, "{name}: the doc reads back");
        revalidate(&doc).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Some(n) = f.get("input_bytes") {
            assert_eq!(json!(bytes(&f["input"])), *n, "{name}: input bytes");
            assert_eq!(json!(bytes(&f["doc"])), f["doc_bytes"], "{name}: doc bytes");
            assert!(bytes(&f["doc"]) <= DOC_MAX_BYTES, "{name}");
        }
        assert_eq!(json!(wait), f["wait"], "{name}");
        assert_eq!(serde_json::to_value(doc.mode()).unwrap(), f["mode"], "{name}");

        // The digest and the envelope (§1.5), and the binding (§2.2).
        let card = id(&f["card"]);
        assert_eq!(json!(digest(&doc, wait).to_string()), f["digest"], "{name}: digest");
        let e = Envelope::showing(card.clone(), &doc, wait);
        let line = e.to_string();
        assert_eq!(json!(line), f["envelope"], "{name}: envelope");
        assert!(line.len() <= ENVELOPE_MAX_BYTES, "{name}");
        let result = format!("{line}\n  1. …\nIts answer will come as a user message starting \"[card {card}]\".");
        let back = Envelope::parse(&result).unwrap_or_else(|| panic!("{name}: the envelope reads"));
        assert!(back.binds(&doc, wait), "{name}: binds");
        assert!(!back.binds(&doc, !wait), "{name}: a flipped wait doesn't");

        for a in f["answers"].as_array().into_iter().flatten() {
            let r = reply(&doc, &a["kind"], &a["values"]);
            if let Reply::Answer(v) | Reply::Change(v) = &r {
                assert_eq!(serde_json::to_value(v).unwrap(), a["values"], "{name}");
            }
            round_trip(&card, &doc, &r, a["message"].as_str().unwrap(), &name);
        }
        for bad in f["bad_answers"].as_array().into_iter().flatten() {
            let e = parse_answer(&doc, &bad["values"]).expect_err(&name).to_string();
            assert_eq!(json!(e), bad["error"], "{name}");
            assert!(one_safe_line(&e), "{name}: {e:?}");
        }
    }
}

#[test]
fn invalid_fixtures() {
    for (name, f) in read_dir("invalid") {
        let e = parse_input(&f["input"]).expect_err(&name).to_string();
        assert_eq!(json!(e), f["error"], "{name}");
        assert!(one_safe_line(&e), "{name}: {e:?}");
    }
}

/// The raw-JSON lane (Shady #439 L2): `decode_json`, then the model.
#[test]
fn raw_fixtures() {
    for (name, f) in read_dir("raw") {
        let decoded = decode_json(f["text"].as_str().unwrap());
        let v = match (decoded, f["json_error"].as_str()) {
            (Err(e), Some(prefix)) => {
                assert!(e.to_string().starts_with(prefix), "{name}: {e}");
                assert!(one_safe_line(&e.to_string()), "{name}: {e:?}");
                continue;
            }
            (Ok(v), None) => v,
            (other, _) => panic!("{name}: {other:?}"),
        };
        match f["what"].as_str().unwrap() {
            "input" => match parse_input(&v) {
                Ok(Input::Create { doc, .. }) => assert_eq!(serde_json::to_value(&doc).unwrap(), f["doc"], "{name}"),
                Ok(other) => panic!("{name}: {other:?}"),
                Err(e) => assert_eq!(json!(e.to_string()), f["error"], "{name}"),
            },
            "answer" => {
                let (doc, _) = create(&f["card"]);
                match parse_answer(&doc, &v) {
                    Ok(a) => assert_eq!(serde_json::to_value(&a).unwrap(), f["values"], "{name}"),
                    Err(e) => assert_eq!(json!(e.to_string()), f["error"], "{name}"),
                }
            }
            other => panic!("{name}: {other}"),
        }
    }
}

/// Full case folding at the corpus's Unicode, for every scalar value, on
/// any toolchain (SynapsCLI #447): `casefold.json` is Python's
/// `str.casefold`, the crate's table is `CaseFolding.txt`.
#[test]
fn casefold_fixture() {
    let f = read("casefold.json");
    let (major, minor, update) = UNICODE_VERSION;
    assert_eq!(f["unicode"], format!("{major}.{minor}.{update}"), "the crate's table and the corpus are one Unicode");
    let table = f["folds"].as_object().unwrap();
    assert_eq!(table.len(), 1557);
    let mut wrong = Vec::new();
    for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
        let s = c.to_string();
        let expected = table.get(&format!("U+{:04X}", c as u32)).map_or(s.as_str(), |v| v.as_str().unwrap());
        if fold_key(&s) != expected {
            wrong.push(format!("U+{:04X}", c as u32));
        }
    }
    assert!(wrong.is_empty(), "fold_key differs from casefold.json at {wrong:?}");
}

#[test]
fn sanitize_fixtures() {
    let f = read("sanitize.json");
    let cases = f["cases"].as_array().unwrap();
    assert!(cases.len() >= 20);
    for c in cases {
        let s = c["in"].as_str().unwrap();
        assert_eq!(json!(ui_sanitize(s, Lines::Single)), c["single"], "{s:?}");
        assert_eq!(json!(ui_sanitize(s, Lines::Multi)), c["multi"], "{s:?}");
        let refused = refused_char(s).map(|(ch, why)| json!({"char": format!("U+{:04X}", ch as u32), "why": why}));
        assert_eq!(refused.unwrap_or(Value::Null), c["refused"], "{s:?}");
    }
}

#[test]
fn message_round_trips() {
    for c in read("messages/round_trips.json")["cases"].as_array().unwrap() {
        let what = c["about"].as_str().unwrap();
        let (doc, _) = create(&c["input"]);
        assert_eq!(serde_json::to_value(&doc).unwrap(), c["doc"], "{what}: the case's doc");
        let r = reply(&doc, &c["kind"], &c["values"]);
        round_trip(&id(&c["card"]), &doc, &r, c["text"].as_str().unwrap(), what);
    }
}

#[test]
fn message_look_alikes() {
    let f = read("messages/look_alikes.json");
    let fixture = read(&format!("valid/{}.json", f["fixture"].as_str().unwrap()));
    let (doc, _) = create(&fixture["input"]);
    for c in f["cases"].as_array().unwrap() {
        let what = c["about"].as_str().unwrap();
        let parsed = message::parse(c["text"].as_str().unwrap());
        let Some(p) = parsed else {
            assert_eq!(c["parses"], Value::Null, "{what}: ordinary text");
            continue;
        };
        assert_eq!(json!({"card": p.card.as_str(), "kind": kind_name(p.kind)}), c["parses"], "{what}");
        match (p.validate(&doc), c.get("reply"), c.get("error")) {
            (Ok(r), Some(want), None) => assert_eq!(r, reply(&doc, &want["kind"], &want["values"]), "{what}"),
            (Err(e), None, Some(want)) => assert_eq!(json!(e.to_string()), *want, "{what}"),
            (got, _, _) => panic!("{what}: {got:?}"),
        }
    }
}

/// `schema.json` (the tool's `input_schema`) and the bounds `parse_input`
/// enforces are the same numbers.
#[test]
fn the_schema_and_the_bounds_agree() {
    let s: Value = serde_json::from_str(SCHEMA).unwrap();
    let p = &s["properties"];
    let opt = &p["options"]["items"]["oneOf"];
    let field = &p["fields"]["items"]["properties"];
    for (v, n) in [
        (&p["question"]["maxLength"], QUESTION_MAX),
        (&p["question"]["minLength"], 1),
        (&p["detail"]["maxLength"], DETAIL_MAX),
        (&p["options"]["minItems"], OPTIONS_MIN),
        (&p["options"]["maxItems"], OPTIONS_MAX),
        (&opt[0]["maxLength"], OPTION_LABEL_MAX),
        (&opt[1]["properties"]["label"]["maxLength"], OPTION_LABEL_MAX),
        (&opt[1]["properties"]["hint"]["maxLength"], HINT_MAX),
        (&opt[1]["properties"]["note"]["maxLength"], NOTE_PROMPT_MAX),
        (&p["default"]["oneOf"][1]["maxItems"], DEFAULT_MAX),
        (&p["fields"]["minItems"], FIELDS_MIN),
        (&p["fields"]["maxItems"], FIELDS_MAX),
        (&field["label"]["maxLength"], FIELD_LABEL_MAX),
        (&field["placeholder"]["maxLength"], PLACEHOLDER_MAX),
    ] {
        assert_eq!(*v, json!(n), "{v} vs {n}");
    }
    assert_eq!(field["id"]["pattern"], format!("^[a-z][a-z0-9_]{{0,{}}}$", FIELD_ID_MAX - 1));
    assert_eq!(p["close"]["pattern"], "^c[a-z0-9]{6}$");
    let keys: Vec<&str> = p.as_object().unwrap().keys().map(String::as_str).collect();
    let mut want = vec!["question", "detail", "options", "multi", "default", "fields", "wait", "close"];
    want.sort();
    let mut keys = keys;
    keys.sort();
    assert_eq!(keys, want);
}
