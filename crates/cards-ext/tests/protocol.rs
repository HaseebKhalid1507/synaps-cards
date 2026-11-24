//! The built extension on a real pipe, driven as SynapsCLI `dev` drives it.
//! Every frame below has the shape the host writes (`extensions/runtime/
//! process.rs`): `initialize` with `InitializeParams`, `tool.call` with
//! `tool_call_params`, `shutdown` as a request with `params: null`; and every
//! answer is read as the host reads it.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use cards_spec::{Envelope, Input, parse_input};
use serde_json::{Value, json};

const ACTOR: &str = "actor-0f1e2d3c4b5a69788796a5b4c3d2e1f0";
const GROUP: &str = "group-00112233445566778899aabbccddeeff";
const PARENT: &str = "actor-ffeeddccbbaa99887766554433221100";

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn fixture(rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(root().join("fixtures").join(rel)).unwrap()).unwrap()
}

struct Ext {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Ext {
    /// Spawned as the host spawns it: from the plugin root.
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cards-ext"))
            .current_dir(root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ext { child, stdin, stdout, next: 1 }
    }

    fn write(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    fn frame(v: &Value) -> Vec<u8> {
        let body = v.to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    /// One answer, read as `read_one_frame` reads it.
    fn read(&mut self) -> Value {
        let mut length = None;
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "the extension closed stdout");
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').unwrap();
            if name.trim().eq_ignore_ascii_case("Content-Length") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; length.expect("Content-Length")];
        self.stdout.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.write(&Self::frame(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })));
        let reply = self.read();
        assert_eq!(reply["id"], id, "{reply}");
        assert_eq!(reply["jsonrpc"], "2.0");
        reply
    }

    fn call(&mut self, input: &Value, session_id: Value, caller: Option<Value>) -> Value {
        let mut params = json!({ "name": "ui", "input": input, "session_id": session_id, "cwd": "/home/u/project" });
        if let Some(caller) = caller {
            params["caller"] = caller;
        }
        self.request("tool.call", params)
    }

    fn exit(mut self, within: Duration) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(start.elapsed() < within, "still running after {within:?}");
            sleep(Duration::from_millis(10));
        }
    }
}

/// `InitializeParams` at `dev`: the host refuses any other protocol version,
/// and checks each tool's name, description and schema.
fn initialize(ext: &mut Ext) -> Value {
    let reply = ext.request(
        "initialize",
        json!({
            "synaps_version": "0.12.0",
            "extension_protocol_version": 1,
            "plugin_id": "cards",
            "plugin_root": root().to_string_lossy(),
            "config": {},
            "features": ["inject.forget"],
        }),
    );
    let result = &reply["result"];
    assert_eq!(result["protocol_version"], 1, "{reply}");
    let tools = result["capabilities"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "ui");
    assert!(!tools[0]["description"].as_str().unwrap().trim().is_empty());
    assert!(tools[0]["input_schema"].is_object());
    reply
}

/// A chat's session: a card from Appendix A1, the same card's golden result,
/// a wait card, a refusal, a close, then `shutdown` and a clean exit.
#[test]
fn a_host_session() {
    let mut ext = Ext::start();
    initialize(&mut ext);
    let chat = Some(json!({ "actor": ACTOR, "group": GROUP }));
    let session = json!("20261123-214700-a06f");
    let golden: Vec<Value> = fixture("results.json")["cases"].as_array().unwrap().clone();
    for name in ["a1_plan_with_choices", "a2_approve_or_revise_wait", "a3_small_form"] {
        let f = fixture(&format!("valid/{name}.json"));
        let reply = ext.call(&f["input"], session.clone(), chat.clone());
        let text = reply["result"]["content"].as_str().unwrap_or_else(|| panic!("{name}: {reply}"));
        // The desk binds the call in the transcript: the same input, parsed again.
        let Input::Create { doc, wait } = parse_input(&f["input"]).unwrap() else { panic!() };
        let Some(envelope @ Envelope::Showing { .. }) = Envelope::parse(text) else { panic!("{name}: {text}") };
        assert!(envelope.binds(&doc, wait), "{name}");
        let Envelope::Showing { card, digest, .. } = &envelope else { unreachable!() };
        assert_eq!(json!(digest.to_string()), f["digest"], "{name}");
        // The oracle's result, with this card's freshly minted id.
        let want = golden.iter().find(|c| c["fixture"] == name).unwrap()["result"].as_str().unwrap();
        assert_eq!(text, want.replace(f["card"].as_str().unwrap(), card.as_str()), "{name}");
        assert!(text.len() <= 1024);
        assert_eq!(text.ends_with("end your turn now; the answer starts your next turn."), wait, "{name}");
    }
    let reply = ext.call(&json!({ "question": "Q", "options": ["only"] }), session.clone(), chat.clone());
    assert_eq!(reply["error"], json!({ "code": -32602, "message": "options: at least 2 (got 1); one option is not a choice" }));
    assert!(reply.get("result").is_none());
    let reply = ext.call(&json!({ "close": "c4f9x2a" }), session.clone(), chat.clone());
    assert_eq!(reply["result"]["content"], "Card c4f9x2a closed (cards/1).");
    // §5.5: three were shown this minute, so a fourth is refused.
    let reply = ext.call(&fixture("valid/a1_plan_with_choices.json")["input"], session, chat);
    assert_eq!(reply["error"]["message"], "cards_ui: 3 cards in the last minute; ask in plain text");
    let reply = ext.request("shutdown", Value::Null);
    assert_eq!(reply["result"], Value::Null);
    let status = ext.exit(Duration::from_secs(2));
    assert_eq!(status.code(), Some(0), "{status}");
}

/// The callers `dev` can send: a worker (no session, a `caller` with a
/// `parent`), a chat under `SYNAPS_HOOK_SESSION_ID=0` (no session, a
/// `caller` without one), and a host before 0.10.3 (neither key).
#[test]
fn workers_are_refused_on_the_wire() {
    let mut ext = Ext::start();
    initialize(&mut ext);
    let input = fixture("valid/a1_plan_with_choices.json")["input"].clone();
    let worker = json!({ "code": -32000, "message": "cards_ui: only the main chat can show cards" });
    let reply = ext.call(&input, Value::Null, Some(json!({ "actor": ACTOR, "group": GROUP, "parent": PARENT })));
    assert_eq!(reply["error"], worker);
    let reply = ext.call(&input, Value::Null, None);
    assert_eq!(reply["error"], worker);
    let reply = ext.request("tool.call", json!({ "name": "ui", "input": input }));
    assert_eq!(reply["error"], worker);
    let reply = ext.call(&input, Value::Null, Some(json!({ "actor": ACTOR, "group": GROUP })));
    assert!(reply["result"]["content"].as_str().unwrap().starts_with("Card c"), "{reply}");
    drop(ext.stdin.take());
    assert_eq!(ext.exit(Duration::from_secs(2)).code(), Some(0), "stdin closed: a clean exit");
}

/// A frame the pipe delivers in pieces, header split mid-word.
#[test]
fn a_frame_written_in_pieces() {
    let mut ext = Ext::start();
    initialize(&mut ext);
    let input = fixture("valid/a5_close.json")["input"].clone();
    let frame = Ext::frame(&json!({ "jsonrpc": "2.0", "id": 42, "method": "tool.call",
        "params": { "name": "ui", "input": input, "session_id": "s", "cwd": null } }));
    for piece in [&frame[..5], &frame[5..19], &frame[19..frame.len() - 3], &frame[frame.len() - 3..]] {
        ext.write(piece);
        sleep(Duration::from_millis(30));
    }
    let reply = ext.read();
    assert_eq!(reply["id"], 42);
    assert_eq!(reply["result"]["content"], "Card c4f9x2a closed (cards/1).");
    // A header the host never sends: the stream can't be trusted, exit 1.
    ext.write(b"Content-Type: text/plain\r\n\r\n");
    drop(ext.stdin.take());
    assert_eq!(ext.exit(Duration::from_secs(2)).code(), Some(1));
}
