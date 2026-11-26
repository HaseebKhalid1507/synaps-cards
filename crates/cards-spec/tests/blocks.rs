//! The blocks corpus (`fixtures/blocks/`, docs/blocks.md v1.3) against
//! `cards_spec::block`'s public API. Every golden (normalized ops, digests,
//! results, generations, tags, edit texts, a message's body and notice
//! lines) is written by `scripts/oracle_blocks.py`, a second
//! implementation; every refusal's text is written by hand. CI runs this in
//! both of serde_json's map orders, so the bytes are the same in both.

use std::path::PathBuf;

use cards_spec::block::message::{self, Edit, Summary};
use cards_spec::block::*;
use cards_spec::sanitize::{Lines, refused_char, ui_sanitize};
use cards_spec::{ERROR_MESSAGE_MAX, ERROR_PATH_MAX};
use serde_json::{Value, json};

fn cases(name: &str) -> Vec<Value> {
    let p = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/blocks")).join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let v: Value = serde_json::from_str(&text).unwrap();
    v["cases"].as_array().unwrap().clone()
}

fn about(c: &Value) -> &str {
    c["about"].as_str().unwrap()
}

/// A refusal is one line, bounded, with nothing that can hide or reorder
/// text (as cards' refusals).
fn one_safe_line(e: &str) -> bool {
    e.chars().count() <= ERROR_PATH_MAX + 2 + ERROR_MESSAGE_MAX && ui_sanitize(e, Lines::Single) == e && refused_char(e).is_none()
}

fn calls(valid: bool) -> Vec<Value> {
    let mut out = cases("valid.json");
    out.extend(cases("invalid.json"));
    out.extend(cases("limits.json"));
    out.retain(|c| c.get("error").is_none() == valid);
    out
}

#[test]
fn block_calls() {
    let all = calls(true);
    for c in &all {
        let about = about(c);
        let b = parse_call(&c["input"]).unwrap_or_else(|e| panic!("{about}: {e}"));
        let v = serde_json::to_value(&b).unwrap();
        if let Some(n) = c.get("normalized") {
            assert_eq!(v, *n, "{about}");
        }
        assert_eq!(b.digest().to_string(), c["digest"], "{about}");
        let r = b.receipt();
        assert_eq!(r.to_string(), c["result"], "{about}");
        assert!(r.to_string().len() <= RESULT_MAX_BYTES, "{about}");
        assert!(Receipt::parse(c["result"].as_str().unwrap()).is_some_and(|r| r.binds(&b)), "{about}");
        // Normalizing is a fixed point (defaults can grow a call past its
        // input cap; those are the bulky limit cases).
        if serde_json::to_vec(&v).unwrap().len() <= CALL_MAX_BYTES {
            assert_eq!(parse_call(&v).as_ref(), Ok(&b), "{about}");
        }
    }
    assert!(all.len() >= 70, "{}", all.len());
}

#[test]
fn block_refusals() {
    let all = calls(false);
    for c in &all {
        let about = about(c);
        let e = parse_call(&c["input"]).map(|_| ()).expect_err(about).to_string();
        assert_eq!(e, c["error"].as_str().unwrap(), "{about}");
        assert!(one_safe_line(&e), "{about}: {e}");
    }
    assert!(all.len() >= 115, "{}", all.len());
}

/// Every bound is at N (accepted) and N+1 (refused), in that order.
#[test]
fn block_limits_come_in_pairs() {
    let all = cases("limits.json");
    assert!(all.len() >= 88 && all.len().is_multiple_of(2), "{}", all.len());
    for pair in all.chunks(2) {
        let (n, past) = (about(&pair[0]), about(&pair[1]));
        assert_eq!(n.strip_suffix(": at the bound"), past.strip_suffix(": one past it"), "{n} / {past}");
        assert!(pair[1].get("error").is_some(), "{past}");
    }
}

/// The digest is SHA-256 over the normalized batch: two spellings of one
/// batch share it.
#[test]
fn block_spellings_share_a_digest() {
    let v = cases("valid.json");
    let digest = |about: &str| v.iter().find(|c| c["about"].as_str().unwrap().starts_with(about)).unwrap()["digest"].clone();
    assert_eq!(digest("§2.1's two-step tick"), digest("the same tick spelled out"));
    assert_eq!(digest("defaults spelled out"), digest("the same ops with the defaults left out"));
}

#[test]
fn block_tags() {
    for c in cases("tags.json") {
        let about = about(&c);
        let first = &c["first_message"];
        assert_eq!(Generation::preimage(first), c["generation_preimage"], "{about}");
        let g = Generation::of(first);
        assert_eq!(g.to_string(), c["generation"], "{about}");
        let id = Ref::parse(c["id"].as_str().unwrap()).unwrap();
        let (creation, tool, index) =
            (c["creation"].as_u64().unwrap(), c["tool_id"].as_str().unwrap(), c["op_index"].as_u64().unwrap() as usize);
        assert_eq!(Tag::preimage(&g, &id, creation, tool, index), c["tag_preimage"].as_str().unwrap().as_bytes(), "{about}");
        let t = Tag::new(&g, &id, creation, tool, index);
        assert_eq!(t.to_string(), c["tag"], "{about}");
        assert_eq!(Tag::parse(c["tag"].as_str().unwrap()), Some(t), "{about}");
    }
}

fn edit_of(c: &Value) -> Edit {
    let id = Ref::parse(c["id"].as_str().unwrap()).unwrap();
    let of = Tag::parse(c["of"].as_str().unwrap()).unwrap();
    Edit::new(id, c["base"].as_u64().unwrap() as u32, of, &c["parts"]).unwrap_or_else(|e| panic!("{}: {e}", c["about"]))
}

#[test]
fn block_edits_round_trip() {
    for c in cases("edits.json") {
        let about = about(&c);
        let e = edit_of(&c);
        let pieces: Vec<&str> = c["pieces"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        let summary = Summary { title: c["title"].as_str(), back_to: c.get("back_to").and_then(Value::as_u64).map(|r| r as u32), pieces };
        let text = message::build(&e, &summary);
        assert_eq!(text, c["text"].as_str().unwrap(), "{about}");
        if let Some(d) = c.get("design") {
            assert_eq!(text, d.as_str().unwrap(), "{about}: the design's text");
        }
        assert_eq!(message::parse(&text).as_ref(), Some(&e), "{about}");
        let (line1, line2) = text.split_once('\n').unwrap();
        assert!(line1.chars().count() <= message::EDIT_LINE_MAX && line2 == e.patch_text(), "{about}");
        // One kind admits it: its user paths are one kind's.
        let kinds: Vec<Kind> = Kind::ALL.into_iter().filter(|k| e.admit(*k).is_ok()).collect();
        assert_eq!(kinds.len(), 1, "{about}: {kinds:?}");
    }
}

#[test]
fn block_edit_look_alikes() {
    for c in cases("look_alikes.json") {
        let about = about(&c);
        let got = message::parse(c["text"].as_str().unwrap());
        match &c["edit"] {
            Value::Null => assert_eq!(got, None, "{about}"),
            e => assert_eq!(
                got,
                Some(edit_of(&json!({"about": about, "id": e["id"], "base": e["base"], "of": e["of"], "parts": e["parts"]}))),
                "{about}"
            ),
        }
    }
}

#[test]
fn block_notices() {
    for c in cases("notices.json") {
        let about = about(&c);
        let text = c["text"].as_str().unwrap();
        let (body, t) = message::split(text);
        assert_eq!(body, c["body"].as_str().unwrap(), "{about}");
        let lines: Vec<Value> = t
            .lines
            .iter()
            .map(|l| json!({"visibility": l.visibility.as_str(), "notices": l.notices.iter().map(|n| json!([n.id.as_str(), n.version])).collect::<Vec<_>>()}))
            .collect();
        assert_eq!(Value::from(lines), c["lines"], "{about}");
        let twice: Vec<&str> = t.named_twice().iter().map(|id| id.as_str()).map(|s| &text[text.find(s).unwrap()..][..s.len()]).collect();
        assert_eq!(json!(twice), c["named_twice"], "{about}");
        // The body and the trailer, written again, are the text: nothing
        // stripped that wasn't a notice line, byte for byte.
        assert_eq!(format!("{body}{}", t.text()), text, "{about}");
        let read = message::read(text);
        assert_eq!((read.body, &read.trailer), (body, &t), "{about}");
    }
}

/// The input schema (`schema-blocks.json`) and the module's bounds name the
/// same numbers and words.
#[test]
fn the_block_schema_and_the_bounds_agree() {
    let s: Value = serde_json::from_str(SCHEMA).unwrap();
    assert_eq!(s["additionalProperties"], false);
    assert_eq!(s["required"], json!(["ops"]));
    let ops = &s["properties"]["ops"];
    assert_eq!((ops["minItems"].as_u64(), ops["maxItems"].as_u64()), (Some(OPS_MIN as u64), Some(OPS_MAX as u64)));
    let op = &ops["items"];
    assert_eq!(op["additionalProperties"], false);
    let p = op["properties"].as_object().unwrap();
    let mut keys: Vec<&str> = p.keys().map(String::as_str).collect();
    keys.sort();
    // Every key some op takes (op.rs), and no other.
    let mut taken = vec![
        "op", "id", "ids", "kind", "title", "target", "slot", "at", "body", "set", "append", "remove", "order", "state", "open", "run",
        "summary", "into", "why",
    ];
    taken.sort();
    assert_eq!(keys, taken);
    assert_eq!(p["op"]["enum"], json!(["put", "patch", "replace", "collapse", "merge", "move", "remove"]));
    assert_eq!(p["kind"]["enum"], json!(Kind::AGENT.map(Kind::as_str)));
    assert_eq!(p["target"]["enum"], json!([Target::Transcript.as_str(), Target::Rail.as_str()]));
    assert_eq!(p["slot"]["enum"], json!([Slot::Upper.as_str(), Slot::End.as_str()]));
    assert_eq!(p["title"]["maxLength"], TITLE_MAX);
    assert_eq!(p["why"]["maxLength"], WHY_MAX);
    assert_eq!(p["summary"]["maxLength"], SUMMARY_MAX);
    assert_eq!(p["ids"]["maxItems"], IDS_MAX);
    assert_eq!(NAMES, ["cards_block", "cards:block"]);
}

/// Every op and every kind has valid and refused cases, and every
/// normative example section is in the corpus (Shady round 2 L2).
#[test]
fn the_block_corpus_is_whole() {
    let ops_of = |cs: &[Value]| -> Vec<String> {
        cs.iter()
            .flat_map(|c| c["input"]["ops"].as_array().cloned().unwrap_or_default())
            .filter_map(|o| o["op"].as_str().map(String::from))
            .collect()
    };
    let kinds_of = |cs: &[Value]| -> Vec<String> {
        cs.iter()
            .flat_map(|c| c["input"]["ops"].as_array().cloned().unwrap_or_default())
            .filter_map(|o| o["kind"].as_str().map(String::from))
            .collect()
    };
    let (valid, invalid) = (cases("valid.json"), cases("invalid.json"));
    for op in ["put", "patch", "replace", "collapse", "merge", "move", "remove"] {
        assert!(ops_of(&valid).iter().any(|o| o == op) && ops_of(&invalid).iter().any(|o| o == op), "{op}");
    }
    for kind in Kind::AGENT {
        let k = kind.as_str().to_string();
        assert!(kinds_of(&valid).contains(&k) && kinds_of(&invalid).contains(&k), "{k}");
    }
    let mut sections: Vec<String> = Vec::new();
    for file in ["valid.json", "invalid.json", "edits.json", "notices.json"] {
        sections.extend(cases(file).iter().filter_map(|c| c["section"].as_str().map(String::from)));
    }
    for s in ["§1.1", "§1.2", "§1.3", "§1.4", "§1.5", "§1.6", "§2.1", "§3.3", "§3.5", "§3.6"] {
        assert!(sections.iter().any(|x| x.split(", ").any(|y| y == s)), "{s}");
    }
}

// ── Fuzz-style properties: seeded, no dependency, a few thousand cases ──

/// SplitMix64: the same cases on every run and every platform.
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
}

/// Strings that sit on a rule: ids and refs near their grammar, paths,
/// state words, invisibles and controls, lengths around the bounds.
fn a_string(r: &mut Rng) -> String {
    let words = [
        "",
        "x",
        "plan",
        "card:c4f9x2a",
        "card:",
        "u-k3x7qa",
        "u-k3x9qa",
        "Plan",
        "0",
        "-a",
        "steps.fix.label",
        "steps.fix.state",
        "steps.fix",
        "steps.u-k3x7qa.stop",
        "hunks.h1.lines",
        "hunks.h1.state",
        "series",
        "title",
        "text",
        "state",
        "done",
        "kept",
        "skipped",
        "doing",
        "proposed",
        "warn",
        "reading",
        "rail",
        "transcript",
        "upper",
        "end",
        "top",
        "edits",
        "a\nb",
        "a\r\nb",
        "\t",
        "\u{202e}x",
        "\u{200b}",
        "\u{200d}",
        "\u{1b}[31m",
        "😀",
        "é",
        "[block a v1 shown]",
    ];
    let mut s = r.pick(&words).to_string();
    if r.below(4) == 0 {
        let n = *r.pick(&[15, 16, 17, 23, 24, 25, 79, 80, 81, 100, 101, 160, 161, 200, 201, 1500, 1501]);
        s = r.pick(&["x", "é", "😀", "a\n"]).repeat(n);
    }
    s
}

fn a_value(r: &mut Rng, depth: usize) -> Value {
    match r.below(if depth > 2 { 4 } else { 6 }) {
        0 => Value::from(a_string(r)),
        1 => json!(*r.pick(&[
            0.0,
            -0.0,
            1.0,
            1.5,
            -1.0,
            7.0,
            1e15,
            -1e15,
            999_999_999_999_999.0,
            1e300,
            1e-7,
            1_000_000.0,
            1_000_001.0,
            10_000_001.0
        ])),
        2 => json!(*r.pick(&[0u64, 1, 3, 9_999, 10_000, u64::MAX])),
        3 => Value::from(r.below(2) == 0),
        4 => Value::Array((0..r.below(4)).map(|_| a_value(r, depth + 1)).collect()),
        _ => {
            let keys = ["id", "label", "state", "text", "lines", "n", "of", "after", "steps", "hunks", "stop", "note"];
            Value::Object((0..r.below(4)).map(|_| (r.pick(&keys).to_string(), a_value(r, depth + 1))).collect())
        }
    }
}

/// Every place in `v`, as a JSON pointer.
fn places(v: &Value, at: String, out: &mut Vec<String>) {
    out.push(at.clone());
    match v {
        Value::Array(xs) => xs.iter().enumerate().for_each(|(i, x)| places(x, format!("{at}/{i}"), out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| places(x, format!("{at}/{}", k.replace('~', "~0").replace('/', "~1")), out)),
        _ => {}
    }
}

fn mutate(r: &mut Rng, v: &mut Value) {
    let mut at = Vec::new();
    places(v, String::new(), &mut at);
    let p = r.pick(&at).clone();
    let keys = [
        "op",
        "id",
        "ids",
        "kind",
        "title",
        "target",
        "slot",
        "at",
        "body",
        "set",
        "append",
        "remove",
        "order",
        "state",
        "open",
        "run",
        "summary",
        "into",
        "why",
        "from",
        "progress",
        "series",
        "line",
        "file",
        "path",
        "stop",
        "steps.fix.state",
        "hunks.h1.lines",
        "series",
    ];
    let new = a_value(r, 0);
    let Some(x) = v.pointer_mut(&p) else { return };
    match (r.below(5), x) {
        (0, Value::Object(o)) if !o.is_empty() => {
            let k = o.keys().nth(r.below(o.len())).unwrap().clone();
            o.remove(&k);
        }
        (1, Value::Object(o)) => {
            o.insert(r.pick(&keys).to_string(), new);
        }
        (2, Value::Array(xs)) if !xs.is_empty() => {
            let i = r.below(xs.len());
            let dup = xs[i].clone();
            if r.below(2) == 0 {
                xs.push(dup)
            } else {
                xs.remove(i);
            }
        }
        (3, x) => *x = json!([x.clone()]),
        (_, x) => *x = new,
    }
}

/// A change that mostly keeps a call valid: one string becomes plain text
/// (ids stay ids), or one number its neighbour.
fn nudge(r: &mut Rng, v: &mut Value) {
    let mut at = Vec::new();
    places(v, String::new(), &mut at);
    at.retain(|p| v.pointer(p).is_some_and(|x| x.is_string() || x.is_number()));
    if at.is_empty() {
        return;
    }
    let p = r.pick(&at).clone();
    let x = v.pointer_mut(&p).unwrap();
    *x = match x.clone() {
        Value::String(s) => {
            let plain = s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
            let extra = if plain {
                r.pick(&["a", "z9", "_x", "-"]).to_string()
            } else {
                r.pick(&[" more", "\u{200b}", "😀", "\t", "\n", "é"]).to_string()
            };
            match r.below(3) {
                0 => Value::from(format!("{s}{extra}")),
                1 => Value::from(s.chars().take(s.chars().count().saturating_sub(1).max(1)).collect::<String>()),
                _ => Value::from(format!("{extra}{s}")),
            }
        }
        Value::Number(n) => {
            let f = n.as_f64().unwrap();
            json!(*r.pick(&[f + 1.0, f - 1.0, f / 2.0, f * 10.0, -f, f + 0.25]))
        }
        other => other,
    };
}

#[test]
fn block_calls_hold_their_promises_under_mutation() {
    let seeds: Vec<Value> =
        ["valid.json", "invalid.json", "limits.json"].iter().flat_map(|f| cases(f)).map(|c| c["input"].clone()).collect();
    let mut r = Rng(0x0b10_c5ed);
    let (mut ok, mut refused) = (0, 0);
    for i in 0..6000 {
        let mut v = r.pick(&seeds).clone();
        if r.below(2) == 0 {
            nudge(&mut r, &mut v);
        } else {
            for _ in 0..=r.below(3) {
                mutate(&mut r, &mut v);
            }
        }
        match parse_call(&v) {
            Err(e) => {
                let e = e.to_string();
                assert!(one_safe_line(&e), "case {i}: {e}");
                refused += 1;
            }
            Ok(b) => {
                let bytes = serde_json::to_vec(&b).unwrap();
                assert_eq!(b.preimage(), bytes, "case {i}: the digest hashes the normalized batch");
                assert!((OPS_MIN..=OPS_MAX).contains(&b.ops.len()), "case {i}");
                for op in &b.ops {
                    let body = match op {
                        Op::Put(p) => Some(serde_json::to_value(p).unwrap()["body"].clone()),
                        Op::Replace(p) => Some(serde_json::to_value(p).unwrap()["body"].clone()),
                        _ => None,
                    };
                    if let Some(body) = body {
                        let kind = Kind::parse(serde_json::to_value(op).unwrap()["kind"].as_str().unwrap()).unwrap();
                        assert!(serde_json::to_vec(&body).unwrap().len() <= kind.max_bytes(), "case {i}");
                    }
                }
                let receipt = b.receipt().to_string();
                assert!(receipt.len() <= RESULT_MAX_BYTES && Receipt::parse(&receipt).is_some_and(|r| r.binds(&b)), "case {i}");
                if bytes.len() <= CALL_MAX_BYTES {
                    let again = parse_call(&serde_json::from_slice(&bytes).unwrap());
                    assert_eq!(again.as_ref(), Ok(&b), "case {i}: normalizing is a fixed point");
                }
                ok += 1;
            }
        }
    }
    // Both sides are reached, so the properties are tested on each.
    assert!(ok >= 600 && refused >= 600, "accepted {ok}, refused {refused}");
}

#[test]
fn messages_split_and_read_back_under_mutation() {
    let mut pieces: Vec<String> = cases("notices.json").iter().map(|c| c["text"].as_str().unwrap().to_string()).collect();
    pieces.extend(cases("look_alikes.json").iter().map(|c| c["text"].as_str().unwrap().to_string()));
    pieces.extend(cases("edits.json").iter().map(|c| c["text"].as_str().unwrap().to_string()));
    let lines: Vec<String> = pieces.iter().flat_map(|p| p.split('\n').map(String::from).collect::<Vec<_>>()).collect();
    let mut r = Rng(0x7e57_ab1e);
    let (mut trailers, mut edits) = (0, 0);
    for i in 0..6000 {
        let text = if r.below(3) == 0 {
            r.pick(&pieces).clone()
        } else {
            let n = 1 + r.below(5);
            let mut t: Vec<String> = (0..n).map(|_| r.pick(&lines).clone()).collect();
            if r.below(4) == 0 {
                let j = r.below(t.len());
                t[j] = a_string(&mut r);
            }
            t.join(r.pick(&["\n", "\n", "\n", "\r\n", " "]))
        };
        let (body, t) = message::split(&text);
        assert_eq!(format!("{body}{}", t.text()), text, "case {i}: nothing but notice lines is taken");
        assert!(t.lines.len() <= 2, "case {i}");
        if let [a, b] = &t.lines[..] {
            assert_ne!(a.visibility, b.visibility, "case {i}: one line per kind");
        }
        assert!(t.lines.iter().all(|l| (1..=8).contains(&l.notices.len())), "case {i}");
        trailers += usize::from(!t.lines.is_empty());
        if let Some(e) = message::parse(&text) {
            let line1 = text.split_once('\n').unwrap().0;
            // Line 2 is read for what it says: the builder's spelling of it reads the same.
            assert_eq!(message::parse(&format!("{line1}\n{}", e.patch_text())).as_ref(), Some(&e), "case {i}: line 2 respelled");
            let again = message::build(&e, &Summary { title: None, back_to: None, pieces: vec!["x"] });
            assert_eq!(message::parse(&again).as_ref(), Some(&e), "case {i}");
            edits += 1;
        }
    }
    assert!(trailers >= 600 && edits >= 100, "trailers {trailers}, edits {edits}");
}
