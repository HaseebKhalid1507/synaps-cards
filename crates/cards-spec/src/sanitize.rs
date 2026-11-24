//! `ui_sanitize`: the one sanitizer for every string a card renders (§5.2),
//! from #75's `sanitize.rs`. The extension applies it at normalization and
//! the desk again at display (defense in depth); `fixtures/sanitize.json`
//! pins it.
//!
//! It drops the default-ignorable set the event renderer drops (WJ, BOM,
//! soft hyphen, variation selectors, tag characters, …) and keeps ZWJ and
//! ZWNJ, for emoji and scripts.
//!
//! Line endings first: `\r\n` and a lone `\r` become `\n` before anything
//! else looks at the string. Then `\t` becomes a space, and a single-line
//! field folds `\n` to a space.
//!
//! Two uses, one table:
//! - **Validation** of what the agent wrote ([`refused_char`]): a C0/C1
//!   control (other than `\n`, `\t` and a line-ending `\r`) or a bidi
//!   embedding, override or isolate is refused with a teaching line, never
//!   rewritten.
//! - **Normalization and display** ([`ui_sanitize`]): everything else in
//!   the set is dropped. It is idempotent, and it never adds a byte.
//!
//! Display sanitizing never changes data sent to the model: answer values
//! are validated ([`crate::parse_answer`]), not rewritten. A client turns
//! CRLF into `\n` in values before it sends; a `\r` in a value is refused.
//!
//! Nothing here reads the toolchain's Unicode tables: `char::is_control`
//! is the fixed C0/C1 set, and case folding is a table generated at the
//! corpus's Unicode ([`fold_key`]).

use std::borrow::Cow;

pub use crate::types::Lines;

/// `\r\n` and a lone `\r` as `\n`.
pub fn normalize_newlines(s: &str) -> Cow<'_, str> {
    if s.contains('\r') { Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n")) } else { Cow::Borrowed(s) }
}

/// The rendered form of `s`.
pub fn ui_sanitize(s: &str, lines: Lines) -> String {
    normalize_newlines(s).chars().filter_map(|c| sanitize_char(c, lines)).collect()
}

fn sanitize_char(c: char, lines: Lines) -> Option<char> {
    match c {
        '\n' if lines == Lines::Multi => Some('\n'),
        '\n' | '\t' => Some(' '),
        // ZWNJ and ZWJ: kept for scripts and emoji sequences.
        '\u{200C}' | '\u{200D}' => Some(c),
        c if c.is_control() => None,
        // Line and paragraph separators.
        '\u{2028}' | '\u{2029}' => None,
        c if is_invisible(c) => None,
        c => Some(c),
    }
}

/// Unicode default-ignorable code points, as SynapsCLI's event renderer
/// lists them (`format.rs`; synaps-client `events::is_invisible`).
pub fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// Why an agent-written string is refused, if it is: the first character
/// that can reorder or break what the user reads. The caller names the
/// field and the code point.
pub fn refused_char(s: &str) -> Option<(char, &'static str)> {
    s.chars().find_map(|c| refusal(c).map(|why| (c, why)))
}

fn refusal(c: char) -> Option<&'static str> {
    match c {
        '\n' | '\t' | '\r' => None,
        c if c.is_control() => Some("a control character"),
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => Some("a bidi control"),
        _ => None,
    }
}

/// Whether a user-typed value holds a C0/C1 control. `\n` and `\t` are
/// allowed only where `lines` is `Multi` (`long` fields); `\r` never is.
pub fn value_control(s: &str, lines: Lines) -> Option<char> {
    s.chars().find(|&c| match c {
        '\n' | '\t' => lines == Lines::Single,
        c => c.is_control(),
    })
}

/// Unicode scalar count: the unit of every character bound, and of JSON
/// Schema's `minLength` / `maxLength`.
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `s` cut to at most `max` characters on a character boundary, ending in
/// `…` when it was cut. Never longer than `max`: cut to 0, it is empty.
pub fn cut_chars(s: &str, max: usize) -> String {
    if char_len(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// `s` cut to at most `max` bytes of UTF-8 on a character boundary, ending
/// in `…` when it was cut (the `…` counted). Never longer than `max`: below
/// 3 bytes, where the `…` can't fit, a cut `s` is the longest prefix that
/// fits, with no `…` (Shady #2 P2).
pub fn cut_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mark = '…'.len_utf8();
    let room = if max >= mark { max - mark } else { max };
    let end = s.char_indices().map(|(i, c)| i + c.len_utf8()).take_while(|&end| end <= room).last().unwrap_or(0);
    if max >= mark { format!("{}…", &s[..end]) } else { s[..end].to_string() }
}

/// The key labels are compared by: Unicode full case folding
/// (`CaseFolding.txt` statuses C and F, as Python's `str.casefold`) of the
/// sanitized label, with no other normalization (no NFC). Used for "labels
/// must differ (ignoring case)" and for matching `default` to a label, so
/// the match is never ambiguous (Shady #439 M3).
///
/// **Pinned** to [`crate::UNICODE_VERSION`], the corpus's, by a generated
/// table (`scripts/gen-casefold.py`), never std's case tables, which follow
/// the toolchain (SynapsCLI #447): the extension and the desk fold alike
/// whatever Rust built them. A character Unicode assigned later folds to
/// itself until the table moves, which is a new catalog.
pub fn fold_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match crate::casefold::FOLDS.binary_search_by_key(&c, |&(k, _)| k) {
            Ok(i) => out.push_str(crate::casefold::FOLDS[i].1),
            Err(_) => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
