//! A card input, validated and normalized into a [`CardDoc`] (§1.4), from
//! #75's `doc.rs`. The extension runs [`parse_input`] on every call; the
//! desk runs it again on the call in the transcript (§2.2).
//!
//! [`parse_input`] enforces the JSON Schema (`schema.json`) by hand, then
//! the rules beyond it, and reports the first failure as one teaching line
//! ([`Error`]). The result is the normalized doc:
//! - every string has been through `ui_sanitize` (single-line fields fold
//!   `\n` to a space);
//! - string options are `{label}`;
//! - `multi`, a field's `type`, `value` and `optional` are materialized;
//! - an empty optional string (`detail`, `hint`, `placeholder`) is absent;
//! - a `default` is a list with `multi` and one label without it, in the
//!   label's own spelling.
//!
//! Order of checks: depth, then bytes (counted, never built), so a value
//! of any size or depth is refused cheaply (Shady #439 M2). In a string,
//! `\r\n` and `\r` become `\n` before any check. A C0/C1 control (other
//! than `\n`/`\t`) or a bidi embedding, override or isolate is refused,
//! not rewritten (`sanitize::refused_char`). Length bounds apply to the raw
//! string (as JSON Schema does); "not empty", line counts and duplicates to
//! the sanitized one. Labels are compared by `sanitize::fold_key`.
//!
//! Normalizing is a fixed point: [`revalidate`] checks that a doc is
//! exactly what normalizing makes of it. Create and revalidate measure the
//! same thing, the normalized doc against `DOC_MAX_BYTES`, so a doc that
//! was accepted always revalidates (Shady #439 H1).

use serde_json::{Map, Value};

use crate::sanitize::{Lines, char_len, cut_chars, fold_key, normalize_newlines, refused_char, ui_sanitize};
use crate::*;

/// The schema's top-level properties, sorted.
const TOP_KEYS_SORTED: [&str; 8] = ["close", "default", "detail", "fields", "multi", "options", "question", "wait"];
const TOP_KEYS: &str = "question, detail, options, multi, default, fields, wait or close";
const OPTION_KEYS: [&str; 3] = ["label", "hint", "note"];
const FIELD_KEYS: [&str; 6] = ["id", "label", "type", "value", "placeholder", "optional"];

/// A parsed `cards_ui` call: ask (create) or withdraw (close).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Create { doc: CardDoc, wait: bool },
    Close(CardId),
}

/// Check a doc read back (deserialized rather than parsed): at most
/// `DOC_MAX_BYTES`, and exactly what normalizing makes of it. The raw
/// input's 8 KiB cap is not applied: it bounds a call, not a doc.
pub fn revalidate(doc: &CardDoc) -> Result<(), Error> {
    if json_len_within(doc, DOC_MAX_BYTES).is_none() {
        return Err(Error::new("doc", format!("over {DOC_MAX_BYTES} bytes")));
    }
    let value = serde_json::to_value(doc).map_err(|e| Error::new("doc", e.to_string()))?;
    let Some(obj) = value.as_object() else {
        return Err(Error::new("doc", "must be an object"));
    };
    if normalize(obj)? == *doc { Ok(()) } else { Err(Error::new("doc", "is not normalized")) }
}

/// Validate and normalize one `cards_ui` input: the extension runs it on
/// the call, and the desk on the same call in the transcript (§2.2).
pub fn parse_input(input: &Value) -> Result<Input, Error> {
    if deeper_than(input, INPUT_MAX_DEPTH) {
        return Err(Error::new("input", format!("nested deeper than {INPUT_MAX_DEPTH} levels")));
    }
    if json_len_within(input, INPUT_MAX_BYTES).is_none() {
        return Err(Error::new("input", format!("over {INPUT_MAX_BYTES} bytes; shorten the detail or ask in plain text")));
    }
    let Some(obj) = input.as_object() else {
        return Err(Error::new("input", "must be a JSON object"));
    };
    if let Some(k) = first_key(obj, |k, _| !TOP_KEYS_SORTED.contains(&k)) {
        return Err(Error::new(key_path(k), format!("unknown parameter; use {TOP_KEYS}")));
    }
    if let Some(close) = obj.get("close") {
        // A key at its schema default counts as absent (v3.2 #5).
        let at_default = |k: &str, v: &Value| matches!(k, "wait" | "multi") && *v == false;
        if let Some(other) = first_key(obj, |k, v| k != "close" && !at_default(k, v)) {
            return Err(Error::new("close", format!("give it alone, with no other parameter (got {})", key_path(other))));
        }
        return match close.as_str().and_then(CardId::parse) {
            Some(id) => Ok(Input::Close(id)),
            None => Err(Error::new("close", "must be the id of a card you created, like c4f9x2a")),
        };
    }
    let wait = flag(obj, "wait")?;
    let doc = normalize(obj)?;
    if json_len_within(&doc, DOC_MAX_BYTES).is_none() {
        // Unreachable for an input within its cap (`NORMALIZE_GROWTH_MAX`);
        // kept so create and revalidate can never disagree.
        return Err(Error::new(
            "input",
            format!("normalized, the card is over {DOC_MAX_BYTES} bytes; shorten the detail or ask in plain text"),
        ));
    }
    Ok(Input::Create { doc, wait })
}

/// The doc an object with only known top-level keys describes.
fn normalize(obj: &Map<String, Value>) -> Result<CardDoc, Error> {
    let multi = flag(obj, "multi")?;
    let Some(question) = obj.get("question") else {
        return Err(Error::new("question", "required: the one-line question to ask"));
    };
    let question = string(question, "question", Lines::Single, 1, QUESTION_MAX, "; put the rest in detail")?;
    let detail = match obj.get("detail") {
        Some(v) => Some(string(v, "detail", Lines::Multi, 0, DETAIL_MAX, "")?),
        None => None,
    };
    if let Some(d) = &detail {
        let lines = d.split('\n').count();
        if lines > DETAIL_MAX_LINES {
            return Err(Error::new("detail", format!("at most {DETAIL_MAX_LINES} lines (got {lines})")));
        }
    }
    if !obj.contains_key("options") && !obj.contains_key("fields") {
        return Err(Error::new("options", "a card needs options or fields (or both)"));
    }
    let options = match obj.get("options") {
        Some(v) => options(v, multi)?,
        None => Vec::new(),
    };
    let default = match obj.get("default") {
        Some(v) => Some(default(v, &options, multi)?),
        None => None,
    };
    let fields = match obj.get("fields") {
        Some(v) => fields(v)?,
        None => Vec::new(),
    };
    Ok(CardDoc { question, detail: non_empty(detail), options, multi, default, fields })
}

fn flag(obj: &Map<String, Value>, key: &str) -> Result<bool, Error> {
    match obj.get(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(Error::new(key, "must be true or false")),
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

/// A shown string: line endings, refused characters, the raw bound, then
/// `ui_sanitize` and the minimum on what is left.
fn string(v: &Value, path: &str, lines: Lines, min: usize, max: usize, advice: &str) -> Result<String, Error> {
    let Some(raw) = v.as_str() else {
        return Err(Error::new(path, "must be a string"));
    };
    let s = normalize_newlines(raw);
    if let Some((c, why)) = refused_char(&s) {
        return Err(Error::new(path, format!("holds U+{:04X} ({why}); remove it", c as u32)));
    }
    let n = char_len(&s);
    if n > max {
        return Err(Error::new(path, format!("at most {max} characters (got {n}){advice}")));
    }
    let clean = ui_sanitize(&s, lines);
    if char_len(&clean) < min {
        let msg = if s.is_empty() { "must not be empty" } else { "must not be empty (it holds only invisible characters)" };
        return Err(Error::new(path, msg));
    }
    Ok(clean)
}

/// A value quoted in an error line: sanitized and cut, so an error can't
/// carry what the card couldn't.
fn quote(s: &str) -> String {
    format!("\"{}\"", cut_chars(&ui_sanitize(s, Lines::Single), 40))
}

fn options(v: &Value, multi: bool) -> Result<Vec<CardOption>, Error> {
    let Some(items) = v.as_array() else {
        return Err(Error::new("options", format!("must be a list of {OPTIONS_MIN} to {OPTIONS_MAX} options")));
    };
    let n = items.len();
    if n < OPTIONS_MIN {
        return Err(Error::new("options", format!("at least {OPTIONS_MIN} (got {n}); one option is not a choice")));
    }
    if n > OPTIONS_MAX {
        return Err(Error::new("options", format!("at most {OPTIONS_MAX} (got {n}); ask in plain text or narrow the choices")));
    }
    let mut out: Vec<CardOption> = Vec::with_capacity(n);
    let mut keys: Vec<(String, String)> = Vec::with_capacity(n);
    let mut note_at: Option<usize> = None;
    for (i, item) in items.iter().enumerate() {
        let path = format!("options[{i}]");
        let (option, label_path) = match item {
            Value::String(_) => {
                let label = string(item, &path, Lines::Single, 1, OPTION_LABEL_MAX, "; move the rest into a hint")?;
                (CardOption { label, hint: None, note: None }, path.clone())
            }
            Value::Object(o) => {
                if let Some(k) = first_key(o, |k, _| !OPTION_KEYS.contains(&k)) {
                    return Err(Error::new(format!("{path}.{}", key_path(k)), "unknown key; an option takes label, hint and note"));
                }
                let label_path = format!("{path}.label");
                let Some(label) = o.get("label") else {
                    return Err(Error::new(label_path, "required"));
                };
                let label = string(label, &label_path, Lines::Single, 1, OPTION_LABEL_MAX, "; move the rest into a hint")?;
                let hint = match o.get("hint") {
                    Some(h) => non_empty(Some(string(h, &format!("{path}.hint"), Lines::Single, 0, HINT_MAX, "")?)),
                    None => None,
                };
                let note = match o.get("note") {
                    Some(p) => Some(string(p, &format!("{path}.note"), Lines::Single, 1, NOTE_PROMPT_MAX, "")?),
                    None => None,
                };
                (CardOption { label, hint, note }, label_path)
            }
            _ => return Err(Error::new(path, "must be a label or {label, hint?, note?}")),
        };
        let key = fold_key(&option.label);
        if let Some((_, first)) = keys.iter().find(|(k, _)| *k == key) {
            return Err(Error::new(
                label_path,
                format!("{} duplicates {first}; labels must differ (ignoring case)", quote(&option.label),),
            ));
        }
        if option.note.is_some() {
            if multi {
                return Err(Error::new(format!("{path}.note"), "not allowed with multi; use a field for free text"));
            }
            if let Some(j) = note_at {
                return Err(Error::new(format!("{path}.note"), format!("only one option may ask for a note (options[{j}] already does)")));
            }
            note_at = Some(i);
        }
        out.push(option);
        keys.push((key, label_path));
    }
    Ok(out)
}

/// v3.2 #4: matched to a label by `fold_key` and stored in the label's
/// spelling; a string with `multi` becomes `[label]`, a one-item list
/// without it the label.
fn default(v: &Value, options: &[CardOption], multi: bool) -> Result<DefaultChoice, Error> {
    if options.is_empty() {
        return Err(Error::new("default", "needs options; it names the option you recommend"));
    }
    let label = |v: &Value, path: &str| -> Result<String, Error> {
        let given = string(v, path, Lines::Single, 0, usize::MAX, "")?;
        let key = fold_key(&given);
        match options.iter().find(|o| fold_key(&o.label) == key) {
            Some(o) => Ok(o.label.clone()),
            None => Err(Error::new(path, format!("{} is not one of the option labels", quote(&given)))),
        }
    };
    match (v, multi) {
        (Value::String(_), false) => Ok(DefaultChoice::One(label(v, "default")?)),
        (Value::String(_), true) => Ok(DefaultChoice::Many(vec![label(v, "default")?])),
        (Value::Array(items), false) => match items.as_slice() {
            [one] => Ok(DefaultChoice::One(label(one, "default[0]")?)),
            _ => Err(Error::new("default", "one option label (a list of more than one only when multi is true)")),
        },
        (Value::Array(items), true) => {
            if items.len() > DEFAULT_MAX {
                return Err(Error::new("default", format!("at most {DEFAULT_MAX} labels (got {})", items.len())));
            }
            let mut out: Vec<String> = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let path = format!("default[{i}]");
                let l = label(item, &path)?;
                if out.contains(&l) {
                    return Err(Error::new(path, format!("{} is listed twice", quote(&l))));
                }
                out.push(l);
            }
            Ok(DefaultChoice::Many(out))
        }
        _ => Err(Error::new("default", "must be an option label")),
    }
}

fn fields(v: &Value) -> Result<Vec<CardField>, Error> {
    let Some(items) = v.as_array() else {
        return Err(Error::new("fields", format!("must be a list of {FIELDS_MIN} to {FIELDS_MAX} fields")));
    };
    let n = items.len();
    if n < FIELDS_MIN {
        return Err(Error::new("fields", format!("at least {FIELDS_MIN} (got {n}); leave fields out instead")));
    }
    if n > FIELDS_MAX {
        return Err(Error::new("fields", format!("at most {FIELDS_MAX} (got {n}); ask fewer things at once")));
    }
    let mut out: Vec<CardField> = Vec::with_capacity(n);
    for (i, item) in items.iter().enumerate() {
        let path = format!("fields[{i}]");
        let Some(o) = item.as_object() else {
            return Err(Error::new(path, "must be an object {id, label, type?, value?, placeholder?, optional?}"));
        };
        if let Some(k) = first_key(o, |k, _| !FIELD_KEYS.contains(&k)) {
            return Err(Error::new(
                format!("{path}.{}", key_path(k)),
                "unknown key; a field takes id, label, type, value, placeholder and optional",
            ));
        }
        let id = field_id(o.get("id"), &format!("{path}.id"))?;
        if let Some(j) = out.iter().position(|f| f.id == id) {
            return Err(Error::new(format!("{path}.id"), format!("\"{id}\" duplicates fields[{j}].id")));
        }
        let Some(label) = o.get("label") else {
            return Err(Error::new(format!("{path}.label"), "required"));
        };
        let label = string(label, &format!("{path}.label"), Lines::Single, 1, FIELD_LABEL_MAX, "")?;
        let kind = match o.get("type") {
            None => FieldKind::Text,
            Some(Value::String(t)) if t == "text" => FieldKind::Text,
            Some(Value::String(t)) if t == "long" => FieldKind::Long,
            Some(Value::String(t)) if t == "check" => FieldKind::Check,
            Some(_) => return Err(Error::new(format!("{path}.type"), "must be text, long or check")),
        };
        let value_path = format!("{path}.value");
        let value = match (kind, o.get("value")) {
            (FieldKind::Check, None) => FieldValue::Check(false),
            (FieldKind::Check, Some(Value::Bool(b))) => FieldValue::Check(*b),
            (FieldKind::Check, Some(other)) => {
                return Err(Error::new(value_path, format!("a check takes true or false (got {})", json_quote(other))));
            }
            (_, None) => FieldValue::Text(String::new()),
            (_, Some(v @ Value::String(_))) => {
                let max = kind.max_chars().unwrap_or(TEXT_MAX);
                let advice = format!(" for a {} field", kind.as_str());
                FieldValue::Text(string(v, &value_path, kind.lines(), 0, max, &advice)?)
            }
            (_, Some(other)) => {
                return Err(Error::new(value_path, format!("a {} field takes a string (got {})", kind.as_str(), json_quote(other))));
            }
        };
        let placeholder = match o.get("placeholder") {
            Some(p) => non_empty(Some(string(p, &format!("{path}.placeholder"), Lines::Single, 0, PLACEHOLDER_MAX, "")?)),
            None => None,
        };
        let optional = flag(o, "optional").map_err(|e| Error::new(format!("{path}.optional"), e.message()))?;
        if optional && kind == FieldKind::Check {
            return Err(Error::new(format!("{path}.optional"), "a check is always answered; only text and long fields can be optional"));
        }
        out.push(CardField { id, label, kind, value, placeholder, optional });
    }
    Ok(out)
}

/// `^[a-z][a-z0-9_]{0,23}$`
fn field_id(v: Option<&Value>, path: &str) -> Result<String, Error> {
    let Some(v) = v else {
        return Err(Error::new(path, "required"));
    };
    let Some(id) = v.as_str() else {
        return Err(Error::new(path, "must be a string"));
    };
    let mut bytes = id.bytes();
    let ok = bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && id.len() <= FIELD_ID_MAX;
    if ok {
        Ok(id.to_string())
    } else {
        Err(Error::new(
            path,
            format!("{} must match ^[a-z][a-z0-9_]{{0,23}}$: a lowercase letter, then up to 23 lowercase letters, digits or _", quote(id)),
        ))
    }
}

/// A JSON value quoted in an error line, cut.
fn json_quote(v: &Value) -> String {
    match v {
        Value::String(s) => quote(s),
        other => cut_chars(&ui_sanitize(&sorted_json(other), Lines::Single), 40),
    }
}

#[cfg(test)]
mod tests;
