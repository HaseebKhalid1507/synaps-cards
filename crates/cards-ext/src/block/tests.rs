//! The `block` tool: results against the oracle's goldens, every refusal
//! (the corpus's, the worker's, the rate's), the rate windows, the gate,
//! and the description and manifest the design fixes.

use std::path::PathBuf;

use cards_spec::block::{CATALOG, NAMES, OPS_MAX, Receipt};
use serde_json::json;

use super::*;
use crate::tool::{Cards, INVALID_PARAMS, REFUSED, initialize};

const ACTOR: &str = "actor-0f1e2d3c4b5a69788796a5b4c3d2e1f0";
const GROUP: &str = "group-00112233445566778899aabbccddeeff";
const PARENT: &str = "actor-ffeeddccbbaa99887766554433221100";

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn cases(name: &str) -> Vec<Value> {
    let path = root().join("fixtures/blocks").join(format!("{name}.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["cases"].as_array().unwrap().clone()
}

/// A chat's `tool.call` params as `dev` writes them (`tool_call_params`).
fn chat(input: &Value) -> Value {
    json!({ "name": "block", "input": input, "session_id": "20261126-161500-b3b3", "cwd": "/home/u/project",
            "caller": { "actor": ACTOR, "group": GROUP } })
}

fn in_chat(session: &str, input: &Value) -> Value {
    json!({ "name": "block", "input": input, "session_id": session, "cwd": null })
}

/// A call of `n` ops, each a valid no-op on a block that needn't exist.
fn ops(n: usize) -> Value {
    json!({ "ops": vec![json!({ "op": "remove", "id": "x" }); n] })
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn refused(message: impl Into<String>) -> Refusal {
    Refusal { code: REFUSED, message: message.into() }
}

/// Every valid call in the corpus (`valid.json`, and `limits.json` at each
/// bound) gets `scripts/oracle_blocks.py`'s result, byte for byte: one line
/// of at most 64 bytes that binds the batch the desk reads back (§1.6).
#[test]
fn results_are_the_oracles() {
    let mut n = 0;
    for case in cases("valid").iter().chain(cases("limits").iter()).filter(|c| c.get("result").is_some()) {
        let what = case["about"].as_str().unwrap();
        let got = Blocks::default().call(&chat(&case["input"]), Instant::now()).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert_eq!(got, case["result"].as_str().unwrap(), "{what}");
        assert!(got.len() <= RESULT_MAX_BYTES && !got.contains('\n'), "{what}: {got:?}");
        let receipt = Receipt::parse(&got).unwrap();
        assert!(receipt.binds(&parse_call(&case["input"]).unwrap()), "{what}");
        assert_eq!(receipt.catalog, CATALOG);
        n += 1;
    }
    assert_eq!(n, 29 + 43, "every valid case and every accepted bound");
    // The longest result: 16 ops, the most a call carries.
    let got = Blocks::default().call(&chat(&ops(OPS_MAX)), Instant::now()).unwrap();
    assert_eq!(got.len(), "ok 16 ops (blocks/1 #0123456789abcdef)".len());
    assert!(got.starts_with("ok 16 ops (blocks/1 #") && got.ends_with(')'), "{got}");
    // Two spellings of one batch are one digest: the receipt names the
    // normalized batch, not the bytes sent.
    let a = Blocks::default().call(&chat(&json!({ "ops": [{ "op": "remove", "id": "x", "why": "" }] })), Instant::now());
    let b = Blocks::default().call(&chat(&json!({ "ops": [{ "id": "x", "op": "remove" }] })), Instant::now());
    assert_eq!(a.unwrap(), b.unwrap());
}

/// Every refusal in the corpus (`invalid.json`, and `limits.json` past each
/// bound) is a JSON-RPC invalid-params error with its teaching line, after
/// `cards_block: ` (§1.6): no receipt, so the desk draws a plain row.
#[test]
fn the_invalid_corpus_is_refused_line_for_line() {
    let mut n = 0;
    for case in cases("invalid").iter().chain(cases("limits").iter()).filter(|c| c.get("error").is_some()) {
        let what = case["about"].as_str().unwrap();
        let got = Blocks::default().call(&chat(&case["input"]), Instant::now()).unwrap_err();
        let want = format!("cards_block: {}", case["error"].as_str().unwrap());
        assert_eq!(got, Refusal { code: INVALID_PARAMS, message: want }, "{what}");
        assert!(Receipt::parse(&got.message).is_none(), "{what}");
        n += 1;
    }
    assert_eq!(n, 78 + 45);
    // §1.6's first refusal, as the design prints it.
    let first = cases("invalid").into_iter().find(|c| c["about"] == "§1.6's first refusal").unwrap();
    assert_eq!(
        Blocks::default().call(&chat(&first["input"]), Instant::now()).unwrap_err().message,
        r#"cards_block: ops[0].body.steps[2].state: "finished" is not a plan state (todo, doing, done, failed, skipped)"#
    );
    let no_input = json!({ "name": "block", "session_id": "s", "cwd": null });
    assert_eq!(
        Blocks::default().call(&no_input, Instant::now()).unwrap_err(),
        Refusal { code: INVALID_PARAMS, message: r#"cards_block: input: must be a JSON object {"ops":[…]}"#.into() }
    );
}

/// Who may keep blocks: the callers `chat_of` reads (§1.1). A worker is
/// refused before its input is read, so it can't validate its way in, and
/// its calls use none of a chat's rate.
#[test]
fn workers_are_refused() {
    let input = ops(1);
    let worker_caller = json!({ "actor": ACTOR, "group": GROUP, "parent": PARENT });
    let mut blocks = Blocks::default();
    let now = Instant::now();
    for params in [
        json!({ "name": "block", "input": input, "session_id": null, "cwd": "/p", "caller": worker_caller }),
        json!({ "name": "block", "input": input, "session_id": "s1", "cwd": "/p", "caller": worker_caller }),
        json!({ "name": "block", "input": input, "session_id": null, "cwd": "/p" }),
        json!({ "name": "block", "input": input }),
        json!({ "name": "block", "input": { "ops": [] }, "session_id": null, "caller": worker_caller }),
        json!({ "name": "block", "session_id": null, "caller": worker_caller }),
    ] {
        for _ in 0..CALLS_PER_MINUTE {
            assert_eq!(blocks.call(&params, now).unwrap_err(), refused(WORKER), "{params}");
        }
    }
    assert_eq!(WORKER, "cards_block: only the main chat has blocks");
    assert!(blocks.rate.calls.is_empty(), "a worker's call isn't counted");
    // A chat under `SYNAPS_HOOK_SESSION_ID=0`: no session, a caller without
    // a parent, keyed by its group.
    let by_group = json!({ "name": "block", "input": input, "session_id": null, "caller": { "actor": ACTOR, "group": GROUP } });
    assert!(blocks.call(&by_group, now).unwrap().starts_with("ok 1 ops"));
    assert!(blocks.rate.calls.contains_key(&format!("group {GROUP}")));
}

/// §7: 30 calls a minute. The 31st is refused with §1.6's line, isn't
/// counted, and the next is admitted once the first is a minute old.
#[test]
fn thirty_calls_a_minute() {
    let t0 = Instant::now();
    let mut rate = Rate::default();
    for i in 0..CALLS_PER_MINUTE as u64 {
        rate.admit("a", 1, t0 + Duration::from_millis(1000 * i)).unwrap();
    }
    for t in [29_500, 59_999] {
        assert_eq!(
            rate.admit("a", 1, t0 + Duration::from_millis(t)).unwrap_err(),
            "cards_block: 31 calls in the last minute; batch your ops or patch less"
        );
    }
    rate.admit("b", 1, t0 + secs(30)).unwrap();
    // At 60 s the call of 0 s is a minute old: one more fits, then none.
    rate.admit("a", 1, t0 + secs(60)).unwrap();
    assert!(rate.admit("a", 1, t0 + secs(60)).is_err());
    rate.admit("a", 1, t0 + secs(61)).unwrap();
    assert_eq!(rate.calls["a"].len(), CALLS_PER_MINUTE + 2, "refused calls aren't counted");
}

/// §7: 120 ops a minute, counted with the call's own: a call that would
/// pass it is refused whole, and a smaller one may still fit.
#[test]
fn a_hundred_and_twenty_ops_a_minute() {
    let t0 = Instant::now();
    let mut rate = Rate::default();
    for i in 0..7 {
        rate.admit("a", OPS_MAX, t0 + secs(i)).unwrap();
    }
    assert_eq!(rate.admit("a", OPS_MAX, t0 + secs(10)).unwrap_err(), "cards_block: 128 ops in the last minute, over 120; patch less");
    assert_eq!(rate.admit("a", 9, t0 + secs(10)).unwrap_err(), "cards_block: 121 ops in the last minute, over 120; patch less");
    rate.admit("a", 8, t0 + secs(10)).unwrap();
    assert_eq!(rate.admit("a", 1, t0 + secs(59)).unwrap_err(), "cards_block: 121 ops in the last minute, over 120; patch less");
    rate.admit("b", OPS_MAX, t0 + secs(59)).unwrap();
    // At 60 s the first call's 16 ops have left.
    rate.admit("a", OPS_MAX, t0 + secs(60)).unwrap();
    assert!(rate.admit("a", 1, t0 + secs(60)).is_err());
    assert_eq!(OPS_PER_MINUTE, 120);
}

/// §7: 200 calls per 10 minutes, at a pace the minute allows (20 a minute).
#[test]
fn two_hundred_calls_in_ten_minutes() {
    let t0 = Instant::now();
    let at = |i: u64| t0 + Duration::from_millis(2_950 * i);
    let mut rate = Rate::default();
    for i in 0..CALLS_PER_TEN_MINUTES as u64 {
        rate.admit("a", 1, at(i)).unwrap_or_else(|e| panic!("call {i}: {e}"));
    }
    // The 200th was at 587.05 s; the first leaves at 600 s.
    for t in [588_000, 599_999] {
        assert_eq!(
            rate.admit("a", 1, t0 + Duration::from_millis(t)).unwrap_err(),
            "cards_block: 201 calls in the last 10 minutes; batch your ops or patch less"
        );
    }
    rate.admit("b", 1, t0 + secs(599)).unwrap();
    rate.admit("a", 1, t0 + secs(600)).unwrap();
    assert!(rate.admit("a", 1, t0 + secs(600)).is_err());
    assert_eq!(rate.calls["a"].len(), CALLS_PER_TEN_MINUTES);
    // Ten quiet minutes later both chats are forgotten, and a third starts.
    rate.admit("c", 1, t0 + secs(1200)).unwrap();
    assert_eq!(rate.calls.keys().collect::<Vec<_>>(), ["c"]);
}

/// Through the tool: a rate refusal is a JSON-RPC error with its own code;
/// a call the rate would refuse but the validator refuses first gets the
/// validation line, uncounted; chats and the two tools count apart.
#[test]
fn the_rate_through_the_tool() {
    let t0 = Instant::now();
    let mut blocks = Blocks::default();
    for i in 0..CALLS_PER_MINUTE as u32 {
        blocks.call(&chat(&ops(1)), t0 + Duration::from_millis(100) * i).unwrap();
    }
    let full = blocks.call(&chat(&ops(1)), t0 + secs(30)).unwrap_err();
    assert_eq!(full, refused("cards_block: 31 calls in the last minute; batch your ops or patch less"));
    let invalid = blocks.call(&chat(&json!({ "ops": [] })), t0 + secs(30)).unwrap_err();
    assert_eq!(invalid, Refusal { code: INVALID_PARAMS, message: "cards_block: ops: at least 1 op".into() });
    assert!(blocks.call(&in_chat("another", &ops(1)), t0 + secs(30)).is_ok());

    let mut cards = Cards::default();
    cards.initialize(&json!({ "config": { "blocks": true } }));
    let card = json!({ "name": "ui", "input": { "question": "Ship it?", "options": ["Yes", "No"] }, "session_id": "s", "cwd": null });
    for _ in 0..crate::tool::PER_MINUTE {
        cards.call(&card, t0).unwrap();
    }
    assert!(cards.call(&card, t0).is_err(), "the cards are spent");
    for _ in 0..CALLS_PER_MINUTE {
        assert!(cards.call(&in_chat("s", &ops(4)), t0).is_ok(), "blocks count apart from cards");
    }
    assert_eq!(
        cards.call(&in_chat("s", &ops(1)), t0).unwrap_err().message,
        "cards_block: 31 calls in the last minute; batch your ops or patch less"
    );
}

/// The gate (§1.1): the host passes the manifest's default as JSON, and an
/// environment or config value as a string.
#[test]
fn the_gate_reads_what_the_host_resolves() {
    for (config, on) in [
        (json!({}), false),
        (json!({ "blocks": null }), false),
        (json!({ "blocks": false }), false),
        (json!({ "blocks": true }), true),
        (json!({ "blocks": "true" }), true),
        (json!({ "blocks": " TRUE " }), true),
        (json!({ "blocks": "1" }), true),
        (json!({ "blocks": "yes" }), true),
        (json!({ "blocks": "On" }), true),
        (json!({ "blocks": "false" }), false),
        (json!({ "blocks": "0" }), false),
        (json!({ "blocks": "no" }), false),
        (json!({ "blocks": "off" }), false),
        (json!({ "blocks": "" }), false),
        (json!({ "blocks": "enabled" }), false),
        (json!({ "blocks": 1 }), false),
        (json!({ "blocks": ["true"] }), false),
        (json!({ "Blocks": true }), false),
        (json!({ "block": true }), false),
        (json!("blocks"), false),
        (Value::Null, false),
    ] {
        assert_eq!(gate(Some(&config)), on, "{config}");
    }
    assert!(!gate(None));
}

/// `initialize` with the gate on and off: off, one tool, and a `block`
/// call is no tool; on, `block` too, with §2.6's description and
/// `schema-blocks.json`, in a shape the host accepts. Off again (a new
/// `initialize`), it's gone.
#[test]
fn initialize_declares_block_only_when_on() {
    let mut cards = Cards::default();
    let call = in_chat("s", &ops(1));
    let off_line = "cards: no tool \"block\"; blocks are off (extension.cards.blocks)";
    assert_eq!(cards.call(&call, Instant::now()).unwrap_err(), Refusal { code: INVALID_PARAMS, message: off_line.into() });
    for config in [json!({}), json!({ "blocks": false }), json!({ "blocks": "off" })] {
        let init = cards.initialize(&json!({ "synaps_version": "0.12.0", "extension_protocol_version": 1, "config": config }));
        assert_eq!(init, initialize(false), "{config}");
        let tools = init["capabilities"]["tools"].as_array().unwrap();
        assert_eq!(tools.iter().map(|t| &t["name"]).collect::<Vec<_>>(), ["ui"], "{config}");
        assert_eq!(cards.call(&call, Instant::now()).unwrap_err().message, off_line);
        let other = cards.call(&json!({ "name": "blocks", "input": {} }), Instant::now()).unwrap_err();
        assert_eq!(other.message, "cards: no tool \"blocks\"; this extension has one, `ui`");
    }
    assert_eq!(cards.initialize(&json!({})), initialize(false), "no config at all");

    let init = cards.initialize(&json!({ "config": { "blocks": "true" } }));
    assert_eq!(init, initialize(true));
    assert_eq!(init["protocol_version"], 1);
    let tools = init["capabilities"]["tools"].as_array().unwrap();
    assert_eq!(tools.iter().map(|t| &t["name"]).collect::<Vec<_>>(), ["ui", "block"]);
    assert_eq!(tools[0], initialize(false)["capabilities"]["tools"][0], "ui is unchanged");
    let block = &tools[1];
    // The host's checks (`validate_registered_tool_specs`), and its names.
    let name = block["name"].as_str().unwrap();
    assert!(!name.contains(':') && !name.chars().any(|c| c.is_whitespace() || c.is_control()) && name.len() <= 64);
    assert_eq!(NAMES, [format!("cards_{name}"), format!("cards:{name}")]);
    assert_eq!(block["description"], DESCRIPTION.trim_end());
    assert_eq!(block["input_schema"], serde_json::from_str::<Value>(cards_spec::block::SCHEMA).unwrap());
    assert_eq!(block["input_schema"]["type"], "object");
    assert_eq!(block.as_object().unwrap().len(), 3);
    assert_eq!(cards.call(&call, Instant::now()).unwrap(), parse_call(&ops(1)).unwrap().receipt().to_string());
    let other = cards.call(&json!({ "name": "blocks", "input": {} }), Instant::now()).unwrap_err();
    assert_eq!(other.message, "cards: no tool \"blocks\"; this extension has two, `ui` and `block`");

    cards.initialize(&json!({ "config": { "blocks": false } }));
    assert_eq!(cards.call(&call, Instant::now()).unwrap_err().message, off_line);
}

/// The description is §2.6's quote, word for word: B3 ships it, and B5
/// changes the design and this file together.
#[test]
fn the_description_is_section_2_6() {
    let doc = std::fs::read_to_string(root().join("docs/blocks.md")).unwrap();
    let mut lines = doc.lines().skip_while(|l| !l.starts_with("### 2.6 ")).skip(1);
    let quote: Vec<&str> =
        lines.by_ref().skip_while(|l| !l.starts_with("> ")).take_while(|l| l.starts_with("> ")).map(|l| &l[2..]).collect();
    assert_eq!(quote.len(), 4);
    assert_eq!(DESCRIPTION, format!("{}\n", quote.join("\n")));
    for must in ["never retry for lack of a sign", "at most one closing call per turn", "[block <id> v<N> dismissed]", "use cards_ui"] {
        assert!(DESCRIPTION.contains(must), "{must}");
    }
}

/// The manifest declares the gate (§1.1): `blocks`, a switch, off.
#[test]
fn the_manifest_declares_the_gate_off() {
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(root().join(".synaps-plugin/plugin.json")).unwrap()).unwrap();
    let ext = &manifest["extension"];
    assert_eq!(ext["permissions"], json!(["tools.register"]));
    assert_eq!(ext["hooks"], json!([]));
    let config = ext["config"].as_array().unwrap();
    assert_eq!(config.len(), 1);
    let entry = &config[0];
    assert_eq!(entry["key"], GATE);
    assert_eq!(entry["type"], "bool");
    assert_eq!(entry["default"], false);
    assert!(entry.get("required").is_none_or(|r| r == false));
    assert!(entry.get("secret_env").is_none());
    assert!(!gate(Some(&json!({ GATE: entry["default"] }))));
}
