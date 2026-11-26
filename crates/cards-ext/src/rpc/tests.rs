//! Framing (split and joined frames, headers, every way the input breaks)
//! and dispatch (requests, notifications, responses, `shutdown`).

use std::io::{BufReader, Cursor, Read};

use super::*;
use crate::tool;

fn frame(v: &Value) -> Vec<u8> {
    let body = v.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

fn request(id: u64, method: &str, params: Value) -> Vec<u8> {
    frame(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
}

fn call(id: u64, input: Value) -> Vec<u8> {
    request(id, "tool.call", json!({ "name": "ui", "input": input, "session_id": "s1", "cwd": "/p" }))
}

fn replies(out: &[u8]) -> Vec<Value> {
    let mut r = Cursor::new(out);
    let mut all = Vec::new();
    while let Some(body) = read_frame(&mut r).unwrap() {
        all.push(serde_json::from_slice(&body).unwrap());
    }
    all
}

fn run(input: &mut impl BufRead) -> (Result<(), FrameError>, Vec<Value>) {
    let mut out = Vec::new();
    let done = serve(input, &mut out, &mut Cards::default(), Instant::now);
    (done, replies(&out))
}

/// A reader that hands out at most `n` bytes a read, so every frame arrives
/// split, the header too.
struct Trickle<'a>(&'a [u8], usize);

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.1.min(buf.len()).min(self.0.len());
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

fn session() -> Vec<u8> {
    [
        request(1, "initialize", json!({ "synaps_version": "0.12.0", "extension_protocol_version": 1, "plugin_id": "cards" })),
        call(2, json!({ "close": "c4f9x2a" })),
        call(3, json!({ "question": "Ship it?", "options": ["Yes", "No"] })),
        call(4, json!({ "question": "Q", "options": ["only"] })),
        request(5, "hook.handle", json!({ "kind": "before_tool_call" })),
        request(6, "shutdown", Value::Null),
        request(7, "initialize", json!({})),
    ]
    .concat()
}

fn check_session(done: Result<(), FrameError>, r: &[Value]) {
    assert!(done.is_ok(), "{done:?}");
    assert_eq!(r.len(), 6, "nothing after shutdown is read: {r:?}");
    for (i, reply) in r.iter().enumerate() {
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], i as u64 + 1);
    }
    assert_eq!(r[0]["result"]["protocol_version"], 1);
    assert_eq!(r[1]["result"], json!({ "content": "Card c4f9x2a closed (cards/1)." }));
    let shown = r[2]["result"]["content"].as_str().unwrap();
    assert!(shown.starts_with("Card c") && shown.contains(" is showing (cards/1 #"), "{shown}");
    assert_eq!(
        r[3]["error"],
        json!({ "code": tool::INVALID_PARAMS, "message": "options: at least 2 (got 1); one option is not a choice" })
    );
    assert_eq!(r[4]["error"], json!({ "code": METHOD_NOT_FOUND, "message": "cards: no method \"hook.handle\"" }));
    assert_eq!(r[5], json!({ "jsonrpc": "2.0", "id": 6, "result": null }));
}

fn block_call(id: u64, input: Value) -> Vec<u8> {
    request(id, "tool.call", json!({ "name": "block", "input": input, "session_id": "s1", "cwd": "/p" }))
}

fn plan() -> Value {
    json!({ "ops": [{ "op": "put", "id": "plan", "kind": "plan", "body": { "steps": [{ "id": "fix", "label": "Fix it" }] } }] })
}

/// A session with blocks on (`docs/blocks.md` §1.1): the gate as the host
/// passes a value from the environment, a batch, a refusal, a worker, a card
/// beside them, then `shutdown`.
fn blocks_session() -> Vec<u8> {
    let worker = json!({ "actor": "a", "group": "g", "parent": "p" });
    [
        request(
            1,
            "initialize",
            json!({ "synaps_version": "0.12.0", "extension_protocol_version": 1, "plugin_id": "cards", "config": { "blocks": "true" } }),
        ),
        block_call(2, plan()),
        block_call(3, json!({ "ops": [] })),
        request(4, "tool.call", json!({ "name": "block", "input": plan(), "session_id": null, "caller": worker })),
        call(5, json!({ "close": "c4f9x2a" })),
        request(6, "shutdown", Value::Null),
        block_call(7, plan()),
    ]
    .concat()
}

fn check_blocks_session(done: Result<(), FrameError>, r: &[Value]) {
    assert!(done.is_ok(), "{done:?}");
    assert_eq!(r.len(), 6, "nothing after shutdown is read: {r:?}");
    for (i, reply) in r.iter().enumerate() {
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], i as u64 + 1);
    }
    let names: Vec<&Value> = r[0]["result"]["capabilities"]["tools"].as_array().unwrap().iter().map(|t| &t["name"]).collect();
    assert_eq!(names, ["ui", "block"]);
    let receipt = cards_spec::block::parse_call(&plan()).unwrap().receipt().to_string();
    assert!(receipt.starts_with("ok 1 ops (blocks/1 #"), "{receipt}");
    assert_eq!(r[1]["result"], json!({ "content": receipt }));
    assert_eq!(r[2]["error"], json!({ "code": tool::INVALID_PARAMS, "message": "cards_block: ops: at least 1 op" }));
    assert_eq!(r[3]["error"], json!({ "code": tool::REFUSED, "message": "cards_block: only the main chat has blocks" }));
    assert_eq!(r[4]["result"], json!({ "content": "Card c4f9x2a closed (cards/1)." }));
    assert_eq!(r[5], json!({ "jsonrpc": "2.0", "id": 6, "result": null }));
}

#[test]
fn blocks_in_joined_and_split_frames() {
    let input = blocks_session();
    let (done, r) = run(&mut Cursor::new(&input[..]));
    check_blocks_session(done, &r);
    for n in [1, 2, 3, 7, 64] {
        let (done, r) = run(&mut BufReader::with_capacity(n, Trickle(&input, n)));
        check_blocks_session(done, &r);
    }
}

/// Off (the manifest's default, or no config at all), `block` is no tool,
/// and `ui` is answered as before.
#[test]
fn blocks_off_on_the_wire() {
    for init in [json!({ "config": { "blocks": false } }), json!({ "config": {} }), json!({})] {
        let input = [request(1, "initialize", init.clone()), block_call(2, plan()), call(3, json!({ "close": "c4f9x2a" }))].concat();
        let (done, r) = run(&mut Cursor::new(&input[..]));
        assert!(done.is_ok() && r.len() == 3, "{init}: {r:?}");
        assert_eq!(r[0]["result"]["capabilities"]["tools"].as_array().unwrap().len(), 1, "{init}");
        assert_eq!(
            r[1]["error"],
            json!({ "code": tool::INVALID_PARAMS, "message": "cards: no tool \"block\"; blocks are off (extension.cards.blocks)" }),
            "{init}"
        );
        assert_eq!(r[2]["result"]["content"], "Card c4f9x2a closed (cards/1).", "{init}");
    }
}

#[test]
fn joined_frames() {
    let input = session();
    let (done, r) = run(&mut Cursor::new(&input[..]));
    check_session(done, &r);
}

#[test]
fn split_frames() {
    let input = session();
    for n in [1, 2, 3, 7, 64] {
        let (done, r) = run(&mut BufReader::with_capacity(n, Trickle(&input, n)));
        check_session(done, &r);
    }
}

/// As the host reads: `Content-Length` in any case, other headers ignored,
/// `\n` line ends too, blank lines before a header skipped.
#[test]
fn headers() {
    let body: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
    for head in [
        format!("content-length: {}\r\n\r\n", body.len()),
        format!("Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nCONTENT-LENGTH:{}\r\n\r\n", body.len()),
        format!("Content-Length: {}\n\n", body.len()),
        format!("\r\n\r\nContent-Length: {} \r\n\r\n", body.len()),
    ] {
        let input = [head.as_bytes(), body].concat();
        let mut r = Cursor::new(&input[..]);
        assert_eq!(read_frame(&mut r).unwrap().as_deref(), Some(body), "{head:?}");
        assert_eq!(read_frame(&mut r).unwrap(), None, "{head:?}");
    }
}

#[test]
fn clean_ends() {
    assert_eq!(read_frame(&mut Cursor::new(&b""[..])).unwrap(), None);
    let (done, r) = run(&mut Cursor::new(&request(1, "initialize", json!({}))[..]));
    assert!(done.is_ok() && r.len() == 1, "the input ends without shutdown: {done:?}");
}

/// Input that can't be trusted any more ends the process (the host starts it
/// again); nothing is answered for it.
#[test]
fn broken_input() {
    let long = format!("X-Pad: {}\r\nContent-Length: 2\r\n\r\n{{}}", "x".repeat(HEADER_LINE_MAX));
    for input in [
        "Content-Type: x\r\n\r\n{}".to_string(),
        "Content-Length: two\r\n\r\n{}".to_string(),
        "Content-Length: -2\r\n\r\n{}".to_string(),
        "Content-Length: 10\r\n\r\n{}".to_string(),
        "Content-Length: 2\r\n".to_string(),
        "Content-Length: 2".to_string(),
        long,
    ] {
        let (done, r) = run(&mut Cursor::new(input.as_bytes()));
        assert!(matches!(done, Err(FrameError::Broken(_))), "{input:?}: {done:?}");
        assert!(r.is_empty(), "{input:?}");
    }
    let not_utf8: &[u8] = b"Content-Length: 2\xff\r\n\r\n{}";
    assert!(matches!(read_frame(&mut Cursor::new(not_utf8)), Err(FrameError::Broken(_))));
}

/// A body too big to read is skipped whole and the next frame is answered.
#[test]
fn an_oversized_body_is_skipped_in_step() {
    let n = BODY_MAX + 1;
    let head = format!("Content-Length: {n}\r\n\r\n");
    let input = [head.as_bytes(), &vec![b' '; n][..], &request(9, "initialize", json!({}))[..]].concat();
    let (done, r) = run(&mut Cursor::new(&input[..]));
    assert!(done.is_ok(), "{done:?}");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["id"], 9);
    let cut = [head.as_bytes(), &b"{}"[..]].concat();
    assert!(matches!(read_frame(&mut Cursor::new(&cut[..])), Err(FrameError::Broken(_))));
}

/// A body that isn't JSON gets JSON-RPC's parse error, and serving goes on.
#[test]
fn a_body_that_is_not_json() {
    let input = [&b"Content-Length: 5\r\n\r\n{nope"[..], &request(2, "initialize", json!({}))[..]].concat();
    let (done, r) = run(&mut Cursor::new(&input[..]));
    assert!(done.is_ok());
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["id"], Value::Null);
    assert_eq!(r[0]["error"]["code"], PARSE_ERROR);
    assert_eq!(r[1]["id"], 2);
}

/// Responses and notifications get no answer. A `tool.call` without an id
/// shows no card, so it uses none of the chat's rate.
#[test]
fn notifications_and_responses_are_not_answered() {
    let create = json!({ "question": "Ship it?", "options": ["Yes", "No"] });
    let mut input = vec![
        frame(&json!({ "jsonrpc": "2.0", "id": 7, "result": {} })),
        frame(&json!({ "jsonrpc": "2.0", "id": 8, "error": { "code": 1, "message": "x" } })),
        frame(&json!([1, 2])),
    ];
    for _ in 0..5 {
        input.push(frame(
            &json!({ "jsonrpc": "2.0", "method": "tool.call", "params": { "name": "ui", "input": create, "session_id": "s1" } }),
        ));
    }
    for id in 1..=tool::PER_MINUTE as u64 {
        input.push(call(id, create.clone()));
    }
    let (done, r) = run(&mut Cursor::new(&input.concat()[..]));
    assert!(done.is_ok());
    assert_eq!(r.len(), tool::PER_MINUTE);
    assert!(r.iter().all(|reply| reply["result"]["content"].is_string()), "{r:?}");
    // `shutdown` as a notification still stops, unanswered.
    let input = [frame(&json!({ "jsonrpc": "2.0", "method": "shutdown" })), request(1, "initialize", json!({}))].concat();
    let (done, r) = run(&mut Cursor::new(&input[..]));
    assert!(done.is_ok() && r.is_empty(), "{r:?}");
}

/// Ids are echoed as they came, whatever their type.
#[test]
fn ids_are_echoed() {
    for id in [json!(0), json!(18446744073709551615u64), json!("evt-001"), json!(null)] {
        let (reply, stop) = handle(
            json!({ "jsonrpc": "2.0", "id": id, "method": "initialize" }).to_string().as_bytes(),
            &mut Cards::default(),
            Instant::now(),
        );
        assert!(!stop);
        assert_eq!(reply.unwrap()["id"], id);
    }
}
