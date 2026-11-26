//! The tool: results against the oracle's goldens, the envelope binding the
//! doc the desk reads back, every refusal, the callers, the rate counter and
//! the ids.

use std::path::PathBuf;

use cards_spec::{Digest, parse_input};

use super::*;

const ACTOR: &str = "actor-0f1e2d3c4b5a69788796a5b4c3d2e1f0";
const GROUP: &str = "group-00112233445566778899aabbccddeeff";
const PARENT: &str = "actor-ffeeddccbbaa99887766554433221100";

fn read(rel: &str) -> Value {
    let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures")).join(rel);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn fixtures(dir: &str) -> Vec<(String, Value)> {
    let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures")).join(dir);
    let mut out: Vec<(String, Value)> = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            (p.file_stem().unwrap().to_string_lossy().into_owned(), serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap())
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A chat's `tool.call` params as `dev` writes them (`tool_call_params`).
fn chat(input: &Value) -> Value {
    json!({ "name": "ui", "input": input, "session_id": "20261123-214700-a06f", "cwd": "/home/u/project",
            "caller": { "actor": ACTOR, "group": GROUP } })
}

fn create(input: &Value) -> (CardDoc, bool) {
    match parse_input(input).unwrap() {
        Input::Create { doc, wait } => (doc, wait),
        Input::Close(_) => unreachable!(),
    }
}

fn card(s: &str) -> CardId {
    CardId::parse(s).unwrap()
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn initialize_declares_one_tool_the_host_accepts() {
    let init = initialize();
    assert_eq!(init["protocol_version"], 1);
    let tools = init["capabilities"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    // The host's checks (`validate_registered_tool_specs`): a name of at most
    // 64 characters with no `:`, whitespace or control; a description; an
    // object schema.
    let name = tools[0]["name"].as_str().unwrap();
    assert_eq!(name, "ui");
    assert!(!name.contains(':') && !name.chars().any(|c| c.is_whitespace() || c.is_control()) && name.len() <= 64);
    assert_eq!(tools[0]["description"], DESCRIPTION.trim_end());
    assert!(DESCRIPTION.contains("`[card <id>]`") && DESCRIPTION.contains("`close`") && DESCRIPTION.contains("`wait: true`"));
    assert_eq!(tools[0]["input_schema"], serde_json::from_str::<Value>(cards_spec::SCHEMA).unwrap());
    assert_eq!(tools[0]["input_schema"]["type"], "object");
}

/// Every result is `scripts/oracle.py`'s, byte for byte, at most 1 KiB, and
/// its line 1 binds the doc it was built from.
#[test]
fn results_are_the_oracles() {
    let cases = read("results.json")["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 18);
    for c in &cases {
        let what = c["about"].as_str().unwrap();
        let (id, doc, wait) = match c["fixture"].as_str() {
            Some(name) => {
                let f = read(&format!("valid/{name}.json"));
                let (doc, wait) = create(&f["input"]);
                (f["card"].as_str().unwrap().to_string(), doc, wait)
            }
            None => {
                let (doc, wait) = create(&c["input"]);
                assert_eq!(serde_json::to_value(&doc).unwrap(), c["doc"], "{what}: the case's doc");
                assert_eq!(json!(wait), c["wait"], "{what}");
                (c["card"].as_str().unwrap().to_string(), doc, wait)
            }
        };
        let got = result_text(&card(&id), &doc, wait);
        assert_eq!(got, c["result"].as_str().unwrap(), "{what}");
        assert!(got.len() <= RESULT_MAX_BYTES, "{what}: {} bytes", got.len());
        assert!(Envelope::parse(&got).unwrap().binds(&doc, wait), "{what}");
    }
}

/// The trailer is whole and last whatever the card, and line 1 is the
/// envelope of the id it names.
#[test]
fn every_result_ends_with_the_whole_trailer() {
    let mut ids = Ids::default();
    for (name, f) in fixtures("valid").into_iter().filter(|(_, f)| f.get("close").is_none()) {
        for wait in [false, true] {
            let (doc, _) = create(&f["input"]);
            let id = ids.mint().unwrap();
            let text = result_text(&id, &doc, wait);
            let lines: Vec<&str> = text.split('\n').collect();
            assert!(text.len() <= RESULT_MAX_BYTES, "{name}");
            assert_eq!(lines[lines.len() - 1], if wait { WAIT } else { LIVE }, "{name}");
            assert_eq!(
                lines[lines.len() - 2],
                format!("Its answer will come as a user message starting \"[card {id}]\", values as JSON on the next line."),
                "{name}"
            );
            assert_eq!(Envelope::parse(&text).unwrap().card(), &id, "{name}");
            assert!(lines[0].len() <= cards_spec::ENVELOPE_MAX_BYTES, "{name}");
        }
    }
}

/// The desk reads the call back from the transcript and binds only when the
/// digest is the doc's (§2.2). Spellings the normalizer makes one card are
/// one digest; another label, or another `wait`, is not the card.
#[test]
fn the_envelope_binds_the_normalized_doc() {
    let mut cards = Cards::default();
    let now = Instant::now();
    let spellings = [
        json!({ "question": "Ship it?", "detail": "a\r\nb", "options": ["Yes", "No"] }),
        json!({ "question": "Ship it?", "detail": "a\nb", "options": [{ "label": "Yes" }, { "label": "No" }], "multi": false }),
        json!({ "question": "Ship it?", "detail": "a\rb", "options": ["Yes", { "label": "No" }], "wait": false }),
    ];
    let mut digests: Vec<Digest> = Vec::new();
    for (i, input) in spellings.iter().enumerate() {
        let text = cards.call(&chat(input), now + secs(61) * i as u32).unwrap();
        let envelope = Envelope::parse(&text).unwrap();
        for other in &spellings {
            let (doc, wait) = create(other);
            assert!(envelope.binds(&doc, wait), "{input} binds {other}");
        }
        let Envelope::Showing { digest, .. } = envelope else { panic!("{text}") };
        digests.push(digest);
    }
    assert!(digests.iter().all(|d| *d == digests[0]));
    let envelope = Envelope::parse(&cards.call(&chat(&spellings[0]), now + secs(200)).unwrap()).unwrap();
    let (other, _) = create(&json!({ "question": "Ship it?", "detail": "a\nb", "options": ["Yes", "Not yet"] }));
    assert!(!envelope.binds(&other, false), "another label");
    let (doc, _) = create(&spellings[0]);
    assert!(!envelope.binds(&doc, true), "another wait");
    assert!(envelope.binds(&doc, false));
    // The valid corpus: the digest is the one cards-spec's goldens hold.
    for (name, f) in fixtures("valid").into_iter().filter(|(_, f)| f.get("close").is_none()) {
        let text = Cards::default().call(&chat(&f["input"]), now).unwrap();
        let Some(Envelope::Showing { digest, .. }) = Envelope::parse(&text) else { panic!("{name}: {text}") };
        assert_eq!(json!(digest.to_string()), f["digest"], "{name}");
    }
}

/// Every invalid fixture is refused with its teaching line, as a JSON-RPC
/// error (no envelope, so a plain row).
#[test]
fn the_invalid_corpus_is_refused_line_for_line() {
    let invalid = fixtures("invalid");
    assert_eq!(invalid.len(), 34);
    for (name, f) in invalid {
        let got = Cards::default().call(&chat(&f["input"]), Instant::now()).unwrap_err();
        assert_eq!(got, Refusal { code: INVALID_PARAMS, message: f["error"].as_str().unwrap().to_string() }, "{name}");
    }
    let mut cards = Cards::default();
    let now = Instant::now();
    let no_input = json!({ "name": "ui", "session_id": "s", "cwd": null });
    assert_eq!(cards.call(&no_input, now).unwrap_err(), Refusal { code: INVALID_PARAMS, message: "input: must be a JSON object".into() });
    let other = json!({ "name": "uix", "input": {}, "session_id": "s" });
    assert_eq!(cards.call(&other, now).unwrap_err().message, "cards: no tool \"uix\"; this extension has one, `ui`");
    assert_eq!(cards.call(&json!({}), now).unwrap_err().message, "cards: no tool (none); this extension has one, `ui`");
}

#[test]
fn closes_answer_with_their_line_and_are_not_counted() {
    let mut cards = Cards::default();
    let now = Instant::now();
    for (name, f) in fixtures("valid").into_iter().filter(|(_, f)| f.get("close").is_some()) {
        for _ in 0..5 {
            assert_eq!(json!(cards.call(&chat(&f["input"]), now).unwrap()), f["envelope"], "{name}");
        }
    }
    let input = json!({ "question": "Ship it?", "options": ["Yes", "No"] });
    for _ in 0..PER_MINUTE {
        cards.call(&chat(&input), now).unwrap();
    }
}

/// Who may show a card. `dev`'s worker sends `session_id: null` and a
/// `caller` with a `parent`; a chat under `SYNAPS_HOOK_SESSION_ID=0` sends
/// `null` and a `caller` without one.
#[test]
fn workers_are_refused_and_chats_are_not() {
    let input = json!({ "question": "Ship it?", "options": ["Yes", "No"] });
    let worker = Refusal { code: REFUSED, message: WORKER.into() };
    let chat_caller = json!({ "actor": ACTOR, "group": GROUP });
    let worker_caller = json!({ "actor": ACTOR, "group": GROUP, "parent": PARENT });
    let by_group = format!("group {GROUP}");
    for (params, chat_key) in [
        (json!({ "name": "ui", "input": input, "session_id": null, "cwd": "/p", "caller": worker_caller }), None),
        (json!({ "name": "ui", "input": input, "session_id": "s1", "cwd": "/p", "caller": worker_caller }), None),
        (json!({ "name": "ui", "input": input, "session_id": null, "cwd": "/p" }), None),
        (json!({ "name": "ui", "input": input }), None),
        (json!({ "name": "ui", "input": input, "session_id": "", "cwd": "/p" }), None),
        (json!({ "name": "ui", "input": input, "session_id": "s1", "cwd": "/p", "caller": chat_caller }), Some("session s1")),
        (json!({ "name": "ui", "input": input, "session_id": "s1", "cwd": "/p" }), Some("session s1")),
        (json!({ "name": "ui", "input": input, "session_id": null, "cwd": "/p", "caller": chat_caller }), Some(by_group.as_str())),
    ] {
        assert_eq!(chat_of(&params).as_deref(), chat_key, "{params}");
        let got = Cards::default().call(&params, Instant::now());
        match chat_key {
            Some(_) => assert!(got.is_ok(), "{params}: {got:?}"),
            None => assert_eq!(got.unwrap_err(), worker, "{params}"),
        }
    }
    // Before its input is read, so a worker can't fix its way in; a close too.
    let bad = json!({ "name": "ui", "input": { "question": "" }, "session_id": null, "caller": worker_caller });
    assert_eq!(Cards::default().call(&bad, Instant::now()).unwrap_err(), worker);
    let close = json!({ "name": "ui", "input": { "close": "c4f9x2a" }, "session_id": null, "caller": worker_caller });
    assert_eq!(Cards::default().call(&close, Instant::now()).unwrap_err(), worker);
}

/// §5.5: 3 a minute and 6 in ten, per chat; a refused card isn't counted.
#[test]
fn three_a_minute_and_six_in_ten() {
    let t0 = Instant::now();
    let mut rate = Rate::default();
    for i in 0..3 {
        rate.admit("a", t0 + secs(i)).unwrap();
    }
    assert_eq!(rate.admit("a", t0 + secs(59)).unwrap_err(), "cards_ui: 3 cards in the last minute; ask in plain text");
    rate.admit("b", t0 + secs(59)).unwrap();
    rate.admit("a", t0 + secs(60)).unwrap();
    rate.admit("a", t0 + secs(130)).unwrap();
    rate.admit("a", t0 + secs(200)).unwrap();
    for t in [400, 599] {
        assert_eq!(rate.admit("a", t0 + secs(t)).unwrap_err(), "cards_ui: 6 cards in the last 10 minutes; ask in plain text");
    }
    rate.admit("a", t0 + secs(600)).unwrap();
    rate.admit("a", t0 + secs(700)).unwrap();
    assert!(rate.window("b").is_none(), "a chat with nothing in the last 10 minutes is forgotten");
    // At 700 s the cards of 1 s, 2 s and 60 s have left: 130, 200, 600, 700.
    assert_eq!(rate.window("a").unwrap().len(), 4);

    // Through the tool: the refusal is a JSON-RPC error, and a close still
    // answers.
    let mut cards = Cards::default();
    let input = json!({ "question": "Ship it?", "options": ["Yes", "No"] });
    for _ in 0..PER_MINUTE {
        cards.call(&chat(&input), t0).unwrap();
    }
    let refused = cards.call(&chat(&input), t0 + secs(30)).unwrap_err();
    assert_eq!(refused, Refusal { code: REFUSED, message: "cards_ui: 3 cards in the last minute; ask in plain text".into() });
    assert!(cards.call(&chat(&json!({ "close": "c4f9x2a" })), t0 + secs(30)).is_ok());
    let other_chat = json!({ "name": "ui", "input": input, "session_id": "another", "cwd": null });
    assert!(cards.call(&other_chat, t0 + secs(30)).is_ok());
}

#[test]
fn an_id_is_c_and_its_draws_six_lowest_base_36_digits() {
    let pairs =
        [(0, "caaaaaa"), (35, "c9aaaaa"), (36, "cabaaaa"), (36u64.pow(6) - 1, "c999999"), (36u64.pow(6), "caaaaaa"), (u64::MAX, "cp2q2eg")];
    for (v, id) in pairs {
        assert_eq!(Ids::id_of(v).as_str(), id, "{v}");
    }
    // Each digit value has its own character, and the 36 are [a-z0-9].
    let firsts: String = (0..36).map(|v| Ids::id_of(v).as_str().chars().nth(1).unwrap()).collect();
    assert_eq!(firsts, "abcdefghijklmnopqrstuvwxyz0123456789");
}

/// The ids remember nothing per id: `Ids` owns no allocation (it has
/// nothing to drop), so the millionth id costs what the first did.
#[test]
fn ids_remember_nothing() {
    assert!(!std::mem::needs_drop::<Ids>());
    assert!(std::mem::size_of::<Ids>() <= 48, "{}", std::mem::size_of::<Ids>());
}

/// What makes the ids unique: [`permute`] is a permutation of `0..n`,
/// the code the 36^6 ids go through. Checked whole on small domains: 2, 4,
/// 6 and 8 bits, every `n` from 1 to the whole domain, every input, under
/// three keys.
#[test]
fn the_permutation_is_one() {
    let keyss = [[0; 4], [1, 2, 3, 4], [u64::MAX, 0x9e37_79b9_7f4a_7c15, 7, 36u64.pow(6)]];
    for half in 1..=4 {
        let all = 1u64 << (2 * half);
        for n in 1..=all {
            for keys in keyss {
                let mut got: Vec<u64> = (0..n).map(|x| permute(&keys, half, n, x)).collect();
                got.sort_unstable();
                assert!(got.iter().copied().eq(0..n), "half {half}, n {n}, keys {keys:?}: {got:?}");
            }
        }
    }
}

/// The n-th id is the permutation's n-th value over the 36^6 ids, under the
/// process's keys: the same keys mint the same ids, and none twice.
#[test]
fn ids_follow_their_keys_and_never_repeat() {
    let keys = [1, 2, 3, 4];
    let mut ids = Ids::keyed(keys);
    let first: Vec<String> = (0..3).map(|_| ids.mint().unwrap().as_str().to_string()).collect();
    // Computed apart from this code (permute and id_of transcribed to Python), not read back from it.
    assert_eq!(first, ["cob7j63", "clm8gjv", "cgv82qn"]);
    let mut again = Ids::keyed(keys);
    for (n, id) in first.iter().enumerate() {
        assert_eq!(again.mint().unwrap().as_str(), id);
        assert_eq!(Ids::id_of(permute(&keys, 16, IDS, n as u64)).as_str(), id);
    }
    let mut seen = std::collections::HashSet::new();
    let mut ids = Ids::keyed([0x5eed, 0, u64::MAX, 36]);
    for n in 0..200_000 {
        assert!(seen.insert(ids.mint().unwrap()), "id {n} repeated");
    }
}

/// The last id: after all 36^6, `mint` has none, and a card is refused (not
/// counted) rather than shown under an id already used. A process would
/// need decades of 256 chats at §5.5's pace to get there.
#[test]
fn the_last_id_is_the_last() {
    let mut ids = Ids::keyed([1, 2, 3, 4]);
    ids.next = IDS - 1;
    assert!(!ids.spent());
    assert_eq!(ids.mint().unwrap().as_str(), "cbekukj");
    assert!(ids.spent());
    assert_eq!(ids.mint(), None);
    assert_eq!(ids.mint(), None);

    let mut cards = Cards::default();
    cards.ids.next = IDS;
    let input = json!({ "question": "Ship it?", "options": ["Yes", "No"] });
    for _ in 0..PER_MINUTE + 1 {
        assert_eq!(cards.call(&chat(&input), Instant::now()).unwrap_err(), Refusal { code: REFUSED, message: SPENT.into() });
    }
    assert_eq!(SPENT, "cards_ui: every card id is taken until the extension restarts; ask in plain text");
    assert_eq!(cards.rate.len(), 0, "a refused card isn't counted");
    assert!(cards.call(&chat(&json!({ "close": "c4f9x2a" })), Instant::now()).is_ok());
}

/// The default ids: well-formed, and never repeated.
#[test]
fn the_default_ids_are_well_formed_and_do_not_repeat() {
    let mut ids = Ids::default();
    let minted: Vec<CardId> = (0..5000).map(|_| ids.mint().unwrap()).collect();
    let distinct: std::collections::HashSet<&CardId> = minted.iter().collect();
    assert_eq!(distinct.len(), minted.len());
    assert!(minted.iter().all(|id| CardId::parse(id.as_str()).as_ref() == Some(id)));
}

/// The cap itself, as §5.5 states it, apart from [`CHATS_MAX`]: 256 chats
/// show a card in the same instant, and the 257th is refused with the line
/// §1.5 names.
#[test]
fn the_cap_is_256_chats() {
    let t0 = Instant::now();
    let mut cards = Cards::default();
    let input = json!({ "question": "Ship it?", "options": ["Yes", "No"] });
    let in_chat = |i: u32| json!({ "name": "ui", "input": input, "session_id": format!("s{i}"), "cwd": null });
    for i in 0..256 {
        cards.call(&in_chat(i), t0).unwrap_or_else(|e| panic!("chat {i}: {e:?}"));
    }
    let line = "cards_ui: 256 other chats showed cards in the last 10 minutes; ask in plain text";
    assert_eq!(cards.call(&in_chat(256), t0).unwrap_err(), Refusal { code: REFUSED, message: line.into() });
    assert_eq!(CHATS_MAX, 256);
}

fn busy() -> String {
    format!("cards_ui: {CHATS_MAX} other chats showed cards in the last 10 minutes; ask in plain text")
}

/// Many chats: at most [`CHATS_MAX`], each with a card in its window. One
/// more is refused, and not kept, until the chat idle longest has been
/// quiet ten minutes; the chats kept go on as before, and one that shows a
/// card again keeps its place. Quiet, they all leave, and the table gives
/// back the memory the crowd took. Through the tool, the refusal is a
/// JSON-RPC error, and a close still answers.
#[test]
fn at_most_chats_max_chats() {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let name = |i: u64| format!("session chat-{i}");
    let mut rate = Rate::default();
    for i in 0..CHATS_MAX as u64 {
        rate.admit(&name(i), at(100 * i)).unwrap();
    }
    assert_eq!(rate.len(), CHATS_MAX);
    let crowd = rate.capacity();
    assert!(crowd >= CHATS_MAX);
    for t in [30_000, 599_999] {
        assert_eq!(rate.admit("session one-more", at(t)).unwrap_err(), busy());
    }
    assert_eq!(rate.len(), CHATS_MAX, "a refused chat isn't kept");
    assert!(rate.window("session one-more").is_none());
    // At 600 s chat-0's card is ten minutes old: it leaves, and one-more takes its place.
    rate.admit("session one-more", at(600_000)).unwrap();
    assert!(rate.window(&name(0)).is_none());
    assert_eq!(rate.admit("session two-more", at(600_000)).unwrap_err(), busy());
    // chat-1, idle longest now, shows a card just before its first leaves:
    // it keeps its place, and chat-2 is the one to leave.
    rate.admit(&name(1), at(600_050)).unwrap();
    assert_eq!(rate.admit("session two-more", at(600_100)).unwrap_err(), busy());
    assert_eq!(rate.window(&name(1)).unwrap().len(), 1);
    rate.admit("session two-more", at(600_200)).unwrap();
    assert!(rate.window(&name(2)).is_none());
    // Every chat kept goes on: its next card is counted beside its first.
    for i in 3..CHATS_MAX as u64 {
        rate.admit(&name(i), at(600_200)).unwrap();
        assert_eq!(rate.window(&name(i)).unwrap().len(), 2);
    }
    assert_eq!(rate.len(), CHATS_MAX);
    rate.admit("session later", at(1_200_200)).unwrap();
    assert_eq!(rate.len(), 1);
    assert!(rate.capacity() <= 16, "{} after {crowd}", rate.capacity());

    let mut cards = Cards::default();
    let input = json!({ "question": "Ship it?", "options": ["Yes", "No"] });
    let in_chat = |s: String, input: &Value| json!({ "name": "ui", "input": input, "session_id": s, "cwd": null });
    for i in 0..CHATS_MAX as u64 {
        cards.call(&in_chat(format!("s{i}"), &input), t0).unwrap();
    }
    assert_eq!(cards.call(&in_chat("new".into(), &input), t0).unwrap_err(), Refusal { code: REFUSED, message: busy() });
    assert!(cards.call(&in_chat("new".into(), &json!({ "close": "c4f9x2a" })), t0).is_ok());
    assert!(cards.call(&in_chat("s7".into(), &input), t0).is_ok(), "a chat kept still shows cards");
}

/// Chats that come and go: four tables of chats, a card each, at the pace
/// that keeps [`CHATS_MAX`] in ten minutes (its step rounded up, so the
/// oldest has left whatever the cap). Each is admitted, the table never
/// holds more, and a chat that comes back after ten quiet minutes starts
/// over, the table small again.
#[test]
fn chats_come_and_go() {
    let t0 = Instant::now();
    let step = Duration::from_nanos(TEN_MINUTES.as_nanos().div_ceil(CHATS_MAX as u128) as u64);
    let mut rate = Rate::default();
    let mut most = 0;
    for i in 0..4 * CHATS_MAX as u32 {
        rate.admit(&format!("session chat-{i}"), t0 + step * i).unwrap_or_else(|e| panic!("chat {i}: {e}"));
        most = most.max(rate.len());
    }
    assert_eq!(most, CHATS_MAX);
    let mut t = t0 + step * (4 * CHATS_MAX as u32) + TEN_MINUTES;
    for round in 0..3 {
        for _ in 0..PER_MINUTE {
            rate.admit("session chat-0", t).unwrap_or_else(|e| panic!("round {round}: {e}"));
        }
        assert!(rate.admit("session chat-0", t).is_err(), "round {round}");
        assert_eq!(rate.len(), 1);
        assert!(rate.capacity() <= 16, "round {round}: {}", rate.capacity());
        t += TEN_MINUTES;
    }
}

/// No chat escapes its limit through eviction: one that spent its minute
/// is still refused while every other place fills and more chats knock,
/// whether it keeps trying or stays quiet, though it is the chat idle
/// longest. Only its own window lets it show a card again.
#[test]
fn no_chat_escapes_its_limit_through_eviction() {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let hot = "session hot";
    let spent = "cards_ui: 3 cards in the last minute; ask in plain text";
    let mut rate = Rate::default();
    for _ in 0..PER_MINUTE {
        rate.admit(hot, at(0)).unwrap();
    }
    for i in 1..CHATS_MAX as u64 {
        rate.admit(&format!("session other-{i}"), at(100 * i)).unwrap();
        if i % 16 == 0 {
            assert_eq!(rate.admit(hot, at(100 * i)).unwrap_err(), spent);
        }
    }
    assert_eq!(rate.len(), CHATS_MAX);
    for i in 0..4 * CHATS_MAX as u64 {
        assert_eq!(rate.admit(&format!("session knock-{i}"), at(30_000)).unwrap_err(), busy());
    }
    assert_eq!(rate.window(hot).unwrap().len(), PER_MINUTE);
    assert_eq!(rate.admit(hot, at(59_999)).unwrap_err(), spent);
    rate.admit(hot, at(60_000)).unwrap();
    // It comes back to its own count: its ten minutes hold all 4 cards.
    assert_eq!(rate.window(hot).unwrap().len(), PER_MINUTE + 1);
    assert_eq!(rate.len(), CHATS_MAX);
}
