//! B5's fold report (`docs/blocks.md` §2.8): what cards-spec's board made of
//! every op, edit and notice of a stored history. It is the same fold the
//! desk draws from ([`Board`]), so the eval counts what the board did, not
//! a second reading of it.
//!
//! The harness (`eval/`) reads three of §2.8's numbers from it:
//! - **skipped ops by reason** (the gate's "Skipped ops" row): an op is
//!   its `(event, op)`, not a line. Its outcome line says what it did (a
//!   `skipped` patch is one none of whose paths applied), and each
//!   `skipped_path` line after it is a diagnostic of one of its paths,
//!   whether the op applied or not; a `remove` of several ids logs a line
//!   per id. The harness counts each op once (`eval/b5/count.py`'s `ops`);
//! - **`put` on a live id** (the "Repairs and re-puts" row): `exists`;
//! - **ops on dismissed blocks** (the unseen patches, §3.5): an agent op
//!   whose `dismissed_before` is true, and **notices delivered**: the
//!   `notice` lines, with what each did.
//!
//! Two things a reading of the board's own state would get wrong:
//! - the board's op log keeps only the latest [`LOG_MAX`] lines, so the
//!   history is folded one event at a time and each event's new lines are
//!   taken as they come: the report has every line, however long the
//!   history;
//! - `dismissed_before` is the board's visibility record for the op's id
//!   just before the event that committed it, not at the history's end: an
//!   op before the dismissal landed is a seen op, one after it an unseen
//!   patch.
//!
//! The board as the desk draws it at the history's end is reported too:
//! its visibility records, and its live blocks in creation order (`blocks`:
//! id, kind, dismissed, pending), the ones a user can click. The harness's
//! dismissal takes the first live, undismissed, settled one.
//!
//! A line's `tool_id` is the call its op came from, so the harness puts it
//! in its turn and round. The batch still open at the history's end (a call
//! with no result yet) is reported as the desk draws it, its lines marked
//! `pending`.
//!
//! Two more of cards-spec's own answers, so the harness never re-implements
//! one ([`receipt`], [`answer`]): what the extension returns for a
//! `cards_block` input, and the message a user's answer to a card is.

use cards_spec::block::{Author, Ref};
use cards_spec::board::{Board, Did, Event, LOG_MAX, Logged, Outcome, Visibility};
use cards_spec::message::{self, Reply};
use cards_spec::{CardDoc, CardId, CardOption, FieldKind, FieldValue, Input, parse_answer, parse_input};
use serde_json::{Map, Value, json};

/// The messages of `history`: the daemon's `SessionQuery::Messages` value
/// (an array), or an object holding one under `messages`.
pub fn messages(history: &Value) -> Option<&[Value]> {
    match history {
        Value::Array(m) => Some(m),
        Value::Object(o) => o.get("messages")?.as_array().map(Vec::as_slice),
        _ => None,
    }
}

/// The fold report of `messages` (module docs): every log line, in the
/// order the board logged them, then the board's visibility records and
/// live blocks at the end.
pub fn report(messages: &[Value]) -> Value {
    let (whole, events) = Board::of_history(messages);
    let mut board = Board::new(whole.generation());
    let mut lines = Vec::new();
    let mut log: Vec<Logged> = Vec::new();
    let mut overflow = false;
    for i in 0..events.len() {
        let before = board.committed().visibility().clone();
        board.read(&events[..=i]);
        let now: Vec<Logged> = board.committed().log().cloned().collect();
        let fresh = appended(&log, &now);
        overflow |= fresh >= LOG_MAX;
        for l in &now[now.len() - fresh..] {
            lines.push(line(&events, l, &before, board.committed().visibility()));
        }
        log = now;
    }
    // The open batch, as the desk draws it: committed, then its calls.
    let view = board.view(&events);
    let drawn: Vec<Logged> = view.log().cloned().collect();
    let fresh = appended(&log, &drawn);
    let before = board.committed().visibility();
    for l in &drawn[drawn.len() - fresh..] {
        lines.push(line(&events, l, before, view.visibility()));
    }
    let records: Vec<Value> =
        view.visibility().records().map(|(id, v)| json!({"id": id.as_str(), "dismissed": v.dismissed, "version": v.version})).collect();
    let mut live: Vec<_> = view.blocks().filter_map(|b| b.id.block().map(|id| (b.creation, id, b))).collect();
    live.sort_by_key(|(creation, ..)| *creation);
    let blocks: Vec<Value> = live
        .iter()
        .map(|(_, id, b)| {
            json!({"id": id.as_str(), "kind": b.kind.as_str(), "dismissed": view.visibility().dismissed(id), "pending": b.pending})
        })
        .collect();
    json!({
        "events": events.len(),
        "lines": lines,
        "visibility": records,
        "blocks": blocks,
        "live": view.live(),
        "log_overflow": overflow,
    })
}

/// How many lines at the end of `now` are new since `prev`: the log only
/// appends, and drops its oldest past [`LOG_MAX`], so `now` is a suffix of
/// `prev` followed by the new lines.
fn appended(prev: &[Logged], now: &[Logged]) -> usize {
    (0..=prev.len())
        .find_map(|d| {
            let kept = &prev[d..];
            (now.len() >= kept.len() && now[..kept.len()] == *kept).then(|| now.len() - kept.len())
        })
        .unwrap_or(now.len())
}

fn line(events: &[Event], l: &Logged, before: &Visibility, after: &Visibility) -> Value {
    let tool_id = match events.get(l.at.event) {
        Some(Event::Call { tool_id, .. }) => Some(tool_id.as_str()),
        _ => None,
    };
    let (outcome, why, path) = match &l.outcome {
        Outcome::Applied => ("applied", None, None),
        Outcome::Here => ("here", None, None),
        Outcome::NoChange => ("no_change", None, None),
        Outcome::Exists => ("exists", None, None),
        Outcome::Skipped(w) => ("skipped", Some(format!("{w:?}")), None),
        Outcome::SkippedPath(p, w) => ("skipped_path", Some(format!("{w:?}")), Some(p.to_string())),
        Outcome::Unbound => ("unbound", None, None),
        Outcome::Stale { version, later } => ("stale", Some(format!("v{version}: v{later} is later")), None),
        Outcome::Twice => ("twice", None, None),
    };
    let block = match &l.id {
        Some(Ref::Block(id)) => Some(id),
        _ => None,
    };
    let mut v = json!({
        "event": l.at.event,
        "op": l.at.op,
        "tool_id": tool_id,
        "author": match l.author {
            Author::Agent => "agent",
            Author::User => "user",
        },
        "did": did(l.did),
        "id": l.id.as_ref().map(Ref::to_string),
        "outcome": outcome,
        "why": why,
        "path": path,
        "dismissed_before": block.is_some_and(|id| before.dismissed(id)),
        "pending": l.pending,
    });
    if l.did == Did::Notice
        && let Some(r) = block.and_then(|id| after.record(id))
    {
        v["record"] = json!({"dismissed": r.dismissed, "version": r.version});
    }
    v
}

fn did(d: Did) -> &'static str {
    match d {
        Did::Put => "put",
        Did::Patch => "patch",
        Did::Replace => "replace",
        Did::Collapse => "collapse",
        Did::Run => "run",
        Did::Merge => "merge",
        Did::Move => "move",
        Did::Remove => "remove",
        Did::Call => "call",
        Did::Edit => "edit",
        Did::Notice => "notice",
    }
}

/// What the extension returns for the `cards_block` input `input`, as
/// cards-spec validates it (§1.6): `{"ok": receipt, "ops": n}`, or
/// `{"error": the refusal line}`.
pub fn receipt(input: &Value) -> Value {
    match cards_spec::block::parse_call(input) {
        Ok(batch) => json!({"ok": batch.receipt().to_string(), "ops": input["ops"].as_array().map_or(0, Vec::len)}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// The text without a note: an option's note prompt or a text field, when
/// the task gives no `fill`.
pub const AS_YOU_SEE_FIT: &str = "As you see fit.";

/// The user's answer to a card, by the eval's fixed policy (module docs of
/// `eval/b5/harness.py`): `request` is `{"input": the cards_ui input,
/// "result": its result, "pick": [labels], "fill": text}`. The task names
/// the option it wants by its label (its prompt asks for exactly those
/// labels), so the answer means one thing whatever the card's order or
/// wording. The card is the one the result names; the answer:
/// - picks the one option whose label is one of `pick` ([`label_is`]: case,
///   outer quotes and backticks, trailing punctuation and spacing folded; a
///   label that goes on past the pick after a space or a bracket counts, so
///   `14 days (two weeks)` is `14 days`, but `Don't hide them` isn't
///   `Hide them`). None, or more than one, is an **interaction failure**:
///   `{"error": …, "interaction": true}`, never the card's default or its
///   first option. A multi-choice card picks that one alone;
/// - fills a text field with `fill`, else its prefill, else "As you see
///   fit.", and ticks a check; a picked option's note gets `fill` too.
///
/// cards-spec checks it against the card (`parse_answer`) and writes the
/// message the desk sends (`message::build`). `{"text": …, "values": …}`,
/// or `{"error": …}`.
pub fn answer(request: &Value) -> Value {
    let pick: Vec<String> =
        request["pick"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let fill = request["fill"].as_str();
    let Some(card) = request["result"].as_str().and_then(card_of) else {
        return json!({"error": "the result names no card showing"});
    };
    let doc = match parse_input(&request["input"]) {
        Ok(Input::Create { doc, .. }) => doc,
        Ok(Input::Close(_)) => return json!({"error": "a close has no answer"}),
        Err(e) => return json!({"error": e.to_string()}),
    };
    let values = match values(&doc, &pick, fill) {
        Ok(v) => v,
        Err(e) => return json!({"error": e, "interaction": true}),
    };
    match parse_answer(&doc, &values) {
        Ok(a) => json!({"text": message::build(&card, &doc, &Reply::Answer(a)), "values": values, "card": card.as_str()}),
        Err(e) => json!({"error": e.to_string(), "values": values}),
    }
}

/// The card a `cards_ui` result says is showing: `Card <id> is showing …`.
fn card_of(result: &str) -> Option<CardId> {
    let rest = result.strip_prefix("Card ")?;
    let (id, tail) = rest.split_once(' ')?;
    tail.starts_with("is showing").then(|| CardId::parse(id)).flatten()
}

/// A label as the policy compares it: case-folded, spacing collapsed, outer
/// quotes and backticks and trailing punctuation off.
fn folded(s: &str) -> String {
    let quote = |c: char| matches!(c, '`' | '"' | '\'' | '\u{201c}' | '\u{201d}' | '\u{2018}' | '\u{2019}');
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    t.trim_start_matches(quote).trim_end_matches(|c: char| quote(c) || matches!(c, '.' | ':' | ';' | '!')).trim().to_string()
}

/// Whether an option labelled `label` is the pick `pick` ([`answer`]): the
/// same folded, or the pick and then more after a non-alphanumeric
/// character.
pub fn label_is(label: &str, pick: &str) -> bool {
    let (l, p) = (folded(label), folded(pick));
    !p.is_empty() && (l == p || l.strip_prefix(&p).is_some_and(|rest| rest.starts_with(|c: char| !c.is_alphanumeric())))
}

fn values(doc: &CardDoc, pick: &[String], fill: Option<&str>) -> Result<Value, String> {
    let mut out = Map::new();
    let text = fill.unwrap_or(AS_YOU_SEE_FIT);
    if !doc.options.is_empty() {
        let hits: Vec<_> = doc.options.iter().filter(|o| pick.iter().any(|p| label_is(&o.label, p))).collect();
        let labels = |os: &[&CardOption]| os.iter().map(|o| o.label.clone()).collect::<Vec<_>>();
        let picked = match hits.as_slice() {
            [one] => *one,
            [] => {
                let all: Vec<_> = doc.options.iter().collect();
                return Err(format!("no option is {pick:?}: the card offers {:?}", labels(&all)));
            }
            more => return Err(format!("{} options are {pick:?}: {:?}", more.len(), labels(more))),
        };
        let label = Value::String(picked.label.clone());
        out.insert("choice".into(), if doc.multi { json!([label]) } else { label });
        if picked.note.is_some() {
            out.insert("note".into(), json!(text));
        }
    }
    if !doc.fields.is_empty() {
        let mut fields = Map::new();
        for f in &doc.fields {
            let v = match (&f.kind, &f.value) {
                (FieldKind::Check, _) => json!(true),
                (_, FieldValue::Text(t)) if fill.is_none() && !t.is_empty() => json!(t),
                _ => json!(text),
            };
            fields.insert(f.id.clone(), v);
        }
        out.insert("fields".into(), Value::Object(fields));
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests;
