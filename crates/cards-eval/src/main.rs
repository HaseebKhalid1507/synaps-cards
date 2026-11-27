//! B5's helper (`docs/blocks.md` §2.8; see the library docs):
//! - `cards-eval fold`: the fold report of the history on stdin, as one
//!   JSON object on stdout. The history is the daemon's
//!   `SessionQuery::Messages` value: an array of messages, or an object
//!   holding one under `messages`.
//! - `cards-eval receipts`: one `cards_block` input per stdin line; per line
//!   on stdout, its receipt or its refusal.
//! - `cards-eval answer`: one answer request per stdin line (`answer`); per
//!   line on stdout, the message or why there is none.

use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let per_line: fn(&serde_json::Value) -> serde_json::Value = match args.as_slice() {
        [c] if c == "fold" => return fold(),
        [c] if c == "receipts" => cards_eval::receipt,
        [c] if c == "answer" => cards_eval::answer,
        _ => {
            eprintln!("usage: cards-eval fold < history.json | receipts < inputs.jsonl | answer < requests.jsonl");
            return ExitCode::from(2);
        }
    };
    for line in std::io::stdin().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("cards-eval: reading stdin: {e}");
                return ExitCode::FAILURE;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let out = match serde_json::from_str(&line) {
            Ok(v) => per_line(&v),
            Err(e) => serde_json::json!({"error": format!("not JSON: {e}")}),
        };
        println!("{out}");
    }
    ExitCode::SUCCESS
}

fn fold() -> ExitCode {
    let mut raw = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut raw) {
        eprintln!("cards-eval: reading stdin: {e}");
        return ExitCode::FAILURE;
    }
    let history: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cards-eval: the history isn't JSON: {e}");
            return ExitCode::FAILURE;
        }
    };
    match cards_eval::messages(&history) {
        Some(messages) => {
            println!("{}", cards_eval::report(messages));
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("cards-eval: the history is neither an array of messages nor an object with `messages`");
            ExitCode::FAILURE
        }
    }
}
