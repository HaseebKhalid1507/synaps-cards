//! Answer values (§1.4, §5.3), from #75's `answer.rs`: [`parse_answer`]
//! checks them against the doc, as the desk does before it sends and as a
//! fold does on an answer message it reads. Values are data for the model:
//! they are refused, never rewritten. [`Answer`] serializes with its keys
//! in a fixed order and its fields in the doc's order.

use serde_json::{Map, Value};

use crate::sanitize::{Lines, char_len, value_control};
use crate::*;

/// Validate answer `values` against the doc (§1.4's shapes, §5.3's
/// bounds): types, required values, membership, lengths, no C0/C1 controls
/// (except `\n` and `\t` in `long`), at most 8 KiB. A skip (`{}`) is not an
/// answer: [`crate::message`] carries it as its own kind.
///
/// Depth, then bytes (counted, never built), before anything else (Shady
/// #439 M2).
pub fn parse_answer(doc: &CardDoc, values: &Value) -> Result<Answer, Error> {
    if deeper_than(values, ANSWER_MAX_DEPTH) {
        return Err(Error::new("values", format!("nested deeper than {ANSWER_MAX_DEPTH} levels")));
    }
    if json_len_within(values, ANSWER_MAX_BYTES).is_none() {
        return Err(Error::new("values", format!("over {ANSWER_MAX_BYTES} bytes")));
    }
    let Some(obj) = values.as_object() else {
        return Err(Error::new("values", "must be an object"));
    };
    if let Some(k) = first_key(obj, |k, _| !matches!(k, "choice" | "note" | "fields")) {
        return Err(Error::new(key_path(k), "unknown key; an answer has choice, note and fields"));
    }
    let choice = choice(doc, obj)?;
    let note = note(doc, obj, choice.as_ref())?;
    let fields = fields(doc, obj)?;
    Ok(Answer { choice, note, fields })
}

fn choice(doc: &CardDoc, obj: &Map<String, Value>) -> Result<Option<Choice>, Error> {
    let Some(v) = obj.get("choice") else {
        return if doc.options.is_empty() { Ok(None) } else { Err(Error::new("choice", "required")) };
    };
    if doc.options.is_empty() {
        return Err(Error::new("choice", "this card has no options"));
    }
    let label = |v: &Value, path: &str| -> Result<String, Error> {
        match v.as_str() {
            Some(l) if doc.option(l).is_some() => Ok(l.to_string()),
            Some(_) => Err(Error::new(path, "not one of the option labels")),
            None => Err(Error::new(path, "must be an option label")),
        }
    };
    if !doc.multi {
        return label(v, "choice").map(|l| Some(Choice::One(l)));
    }
    let Some(items) = v.as_array() else {
        return Err(Error::new("choice", "must be a list of option labels"));
    };
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let path = format!("choice[{i}]");
        let l = label(item, &path)?;
        if out.contains(&l) {
            return Err(Error::new(path, "listed twice"));
        }
        out.push(l);
    }
    Ok(Some(Choice::Many(out)))
}

fn note(doc: &CardDoc, obj: &Map<String, Value>, choice: Option<&Choice>) -> Result<Option<String>, Error> {
    let asks = match choice {
        Some(Choice::One(l)) => doc.option(l).is_some_and(|o| o.note.is_some()),
        _ => false,
    };
    let Some(v) = obj.get("note") else {
        return if asks { Err(Error::new("note", "required for this option")) } else { Ok(None) };
    };
    if !asks {
        return Err(Error::new("note", "the chosen option takes no note"));
    }
    let Some(n) = v.as_str() else {
        return Err(Error::new("note", "must be a string"));
    };
    text_value(n, "note", Lines::Single, NOTE_MAX)?;
    if char_len(n) < NOTE_MIN {
        return Err(Error::new("note", "must not be empty"));
    }
    Ok(Some(n.to_string()))
}

fn fields(doc: &CardDoc, obj: &Map<String, Value>) -> Result<Option<Vec<(String, FieldValue)>>, Error> {
    let Some(v) = obj.get("fields") else {
        return if doc.fields.is_empty() { Ok(None) } else { Err(Error::new("fields", "required: every field, by id")) };
    };
    if doc.fields.is_empty() {
        return Err(Error::new("fields", "this card has no fields"));
    }
    let Some(given) = v.as_object() else {
        return Err(Error::new("fields", "must be an object of values by field id"));
    };
    if let Some(k) = first_key(given, |k, _| !doc.fields.iter().any(|f| f.id == k)) {
        return Err(Error::new(format!("fields.{}", key_path(k)), "no such field"));
    }
    let mut out = Vec::with_capacity(doc.fields.len());
    for f in &doc.fields {
        let path = format!("fields.{}", f.id);
        let Some(v) = given.get(&f.id) else {
            return Err(Error::new(path, "missing; every field is sent (\"\" for an empty optional one)"));
        };
        let value = match (f.kind, v) {
            (FieldKind::Check, Value::Bool(b)) => FieldValue::Check(*b),
            (FieldKind::Check, _) => return Err(Error::new(path, "a check takes true or false")),
            (kind, Value::String(s)) => {
                text_value(s, &path, kind.lines(), kind.max_chars().unwrap_or(TEXT_MAX))?;
                if s.is_empty() && !f.optional {
                    return Err(Error::new(path, "required"));
                }
                FieldValue::Text(s.clone())
            }
            (kind, _) => return Err(Error::new(path, format!("a {} field takes a string", kind.as_str()))),
        };
        out.push((f.id.clone(), value));
    }
    Ok(Some(out))
}

fn text_value(s: &str, path: &str, lines: Lines, max: usize) -> Result<(), Error> {
    if let Some(c) = value_control(s, lines) {
        return Err(Error::new(path, format!("holds U+{:04X} (a control character)", c as u32)));
    }
    let n = char_len(s);
    if n > max {
        return Err(Error::new(path, format!("at most {max} characters (got {n})")));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
