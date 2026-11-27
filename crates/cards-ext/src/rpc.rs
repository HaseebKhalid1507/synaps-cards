//! The extension protocol on stdio (SynapsCLI `docs/extensions/protocol.md`,
//! "Message Format" and "Methods"): JSON-RPC 2.0 bodies in LSP-style frames,
//! `Content-Length: <n>\r\n\r\n<body>`, both ways.
//!
//! Frames are read as the host reads its own (`process.rs`,
//! `read_one_frame`): header lines up to a blank one, each at most 1 KiB,
//! `Content-Length` matched ignoring case, any other header ignored, then
//! exactly that many bytes. A clean end of input before a header is the end.
//!
//! The host only sends; this extension only answers, in order, one frame at
//! a time (the host's `call_lock` serializes every request anyway).

use std::io::{self, BufRead, Read, Write};
use std::time::Instant;

use serde_json::{Value, json};

use crate::tool::{Cards, Refusal};

/// One header line, as the host bounds its own.
pub const HEADER_LINE_MAX: usize = 1024;
/// The largest body read whole. A bigger one is skipped, so the stream stays
/// in step, and goes unanswered: a card's input, and a block call's, is at
/// most 8 KiB.
pub const BODY_MAX: usize = 16 * 1024 * 1024;

pub const PARSE_ERROR: i64 = -32700;
pub const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Debug)]
pub enum FrameError {
    /// Input that can't be read, or isn't in step any more (a bad header, a
    /// cut body): nothing after it can be trusted, so the process exits and
    /// the host starts it again.
    Broken(String),
    /// A body over [`BODY_MAX`], skipped whole.
    TooLarge(usize),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Broken(why) => write!(f, "the input broke off: {why}"),
            FrameError::TooLarge(n) => write!(f, "skipped a {n}-byte frame (over {BODY_MAX})"),
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Broken(e.to_string())
    }
}

/// The next frame's body, or `None` at a clean end of input.
pub fn read_frame(r: &mut impl BufRead) -> Result<Option<Vec<u8>>, FrameError> {
    let mut length = None;
    let mut seen = false;
    loop {
        let mut line = Vec::new();
        let n = r.by_ref().take(HEADER_LINE_MAX as u64 + 1).read_until(b'\n', &mut line)?;
        if n == 0 {
            return if seen { Err(FrameError::Broken("the input ended inside a header".into())) } else { Ok(None) };
        }
        if line.len() > HEADER_LINE_MAX {
            return Err(FrameError::Broken(format!("a header line over {HEADER_LINE_MAX} bytes")));
        }
        let text = std::str::from_utf8(&line).map_err(|_| FrameError::Broken("a header that is not UTF-8".into()))?.trim();
        if text.is_empty() {
            if seen {
                break;
            }
            continue;
        }
        seen = true;
        if let Some((name, value)) = text.split_once(':')
            && name.trim().eq_ignore_ascii_case("Content-Length")
        {
            let value = value.trim();
            length = Some(value.parse::<usize>().map_err(|_| FrameError::Broken(format!("Content-Length {value:?}")))?);
        }
    }
    let length = length.ok_or_else(|| FrameError::Broken("a frame without Content-Length".into()))?;
    if length > BODY_MAX {
        let skipped = io::copy(&mut r.by_ref().take(length as u64), &mut io::sink())?;
        if skipped < length as u64 {
            return Err(FrameError::Broken("the input ended inside a body".into()));
        }
        return Err(FrameError::TooLarge(length));
    }
    let mut body = vec![0; length];
    r.read_exact(&mut body).map_err(|e| FrameError::Broken(format!("the input ended inside a body ({e})")))?;
    Ok(Some(body))
}

pub fn write_frame(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(body)?;
    w.flush()
}

/// Answer frames until the input ends or the host says `shutdown`.
pub fn serve(r: &mut impl BufRead, w: &mut impl Write, cards: &mut Cards, mut now: impl FnMut() -> Instant) -> Result<(), FrameError> {
    loop {
        let body = match read_frame(r) {
            Ok(Some(body)) => body,
            Ok(None) => return Ok(()),
            Err(e @ FrameError::TooLarge(_)) => {
                eprintln!("cards: {e}");
                continue;
            }
            Err(e) => return Err(e),
        };
        let (reply, stop) = handle(&body, cards, now());
        if let Some(reply) = reply {
            write_frame(w, reply.to_string().as_bytes())?;
        }
        if stop {
            return Ok(());
        }
    }
}

/// One frame's answer, if it gets one, and whether to stop after it.
pub fn handle(body: &[u8], cards: &mut Cards, now: Instant) -> (Option<Value>, bool) {
    let message: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return (Some(error(Value::Null, PARSE_ERROR, &format!("not JSON: {e}"))), false),
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response, or no message at all: this extension sends no requests.
        return (None, false);
    };
    let stop = method == "shutdown";
    // A notification gets no answer; only `shutdown` does anything as one.
    let Some(id) = message.get("id").cloned() else { return (None, stop) };
    let params = message.get("params").unwrap_or(&Value::Null);
    let outcome: Result<Value, Refusal> = match method {
        "initialize" => Ok(cards.initialize(params)),
        "tool.call" => cards.call(params, now).map(|text| json!({ "content": text })),
        "shutdown" => Ok(Value::Null),
        other => Err(Refusal { code: METHOD_NOT_FOUND, message: format!("cards: no method {other:?}") }),
    };
    let reply = match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(refusal) => error(id, refusal.code, &refusal.message),
    };
    (Some(reply), stop)
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests;
