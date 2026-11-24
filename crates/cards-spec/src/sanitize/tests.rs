//! `sanitize`: line endings, refusals, cuts, and case folding pinned to the
//! corpus's Unicode (the whole table is checked in `tests/corpus.rs`).

use super::*;

/// Line endings first (v3.2 #1), then `\t`; a single-line field folds `\n`.
#[test]
fn line_endings_tabs_and_folds() {
    assert_eq!(ui_sanitize("a\r\nb\rc\td", Lines::Multi), "a\nb\nc d");
    assert_eq!(ui_sanitize("a\r\nb\rc\td", Lines::Single), "a b c d");
    assert_eq!(ui_sanitize("x\u{200D}y\u{200C}z\u{200B}\u{2028}\u{FEFF}", Lines::Single), "x\u{200D}y\u{200C}z");
}

#[test]
fn refusals_name_the_first_bad_character() {
    assert_eq!(refused_char("ok\nfine\t\r"), None);
    assert_eq!(refused_char("a\u{1b}b\u{202E}"), Some(('\u{1b}', "a control character")));
    assert_eq!(refused_char("a\u{2067}b"), Some(('\u{2067}', "a bidi control")));
    assert_eq!(value_control("a\nb", Lines::Multi), None);
    assert_eq!(value_control("a\nb", Lines::Single), Some('\n'));
    assert_eq!(value_control("a\rb", Lines::Multi), Some('\r'));
}

#[test]
fn cuts() {
    assert_eq!(cut_chars("abcdef", 6), "abcdef");
    assert_eq!(cut_chars("abcdefg", 6), "abcde…");
    assert_eq!(cut_bytes("abcdef", 6), "abcdef");
    assert_eq!(cut_bytes("abcdefg", 6), "abc…");
    // At a character boundary, the `…` counted: 2 + 3 bytes, not 4 + 3.
    assert_eq!(cut_bytes("aé\u{10330}\u{10330}", 6), "aé…");
    assert_eq!(cut_bytes("\u{10330}\u{10330}", 7), "\u{10330}…");
    assert_eq!(cut_bytes("\u{10330}\u{10330}", 6), "…");
    assert!(cut_bytes(&"\u{10330}".repeat(100), 203).len() <= 203);
}

/// Shady #2 P2-1: a budget smaller than the `…` (3 bytes, 1 character) never
/// grows the result: it is the longest prefix that fits, with no `…`.
#[test]
fn cuts_never_exceed_a_budget_smaller_than_the_ellipsis() {
    for (s, max, want) in [
        ("abcdef", 0, ""),
        ("abcdef", 1, "a"),
        ("abcdef", 2, "ab"),
        ("abcdef", 3, "…"),
        ("abcdef", 4, "a…"),
        ("abc", 3, "abc"),
        ("ab", 2, "ab"),
        ("", 0, ""),
        ("é", 1, ""),
        ("éa", 2, "é"),
        ("\u{10330}x", 2, ""),
        ("\u{10330}x", 3, "…"),
        ("\u{10330}x", 5, "\u{10330}x"),
    ] {
        assert_eq!(cut_bytes(s, max), want, "cut_bytes({s:?}, {max})");
    }
    for (s, max, want) in [("x", 0, ""), ("", 0, ""), ("xy", 1, "…"), ("x", 1, "x"), ("é\u{10330}", 1, "…")] {
        assert_eq!(cut_chars(s, max), want, "cut_chars({s:?}, {max})");
    }
    // Every budget, every boundary: within it, a prefix, and `…` only when cut.
    for s in ["", "a", "abcdef", "aé\u{10330}b\u{10330}", "€€€", "\u{10330}\u{10330}\u{10330}"] {
        for max in 0..=s.len() + 2 {
            let out = cut_bytes(s, max);
            let body = out.strip_suffix('…').filter(|_| out != s).unwrap_or(&out);
            assert!(out.len() <= max, "cut_bytes({s:?}, {max}) = {out:?}");
            assert!(s.starts_with(body), "cut_bytes({s:?}, {max}) = {out:?}");
            assert_eq!(out == s, s.len() <= max, "cut_bytes({s:?}, {max}) = {out:?}");
            assert_eq!(out != s && out.ends_with('…'), s.len() > max && max >= 3, "cut_bytes({s:?}, {max}) = {out:?}");
        }
        for max in 0..=char_len(s) + 2 {
            let out = cut_chars(s, max);
            assert!(char_len(&out) <= max, "cut_chars({s:?}, {max}) = {out:?}");
            assert_eq!(out == s, char_len(s) <= max, "cut_chars({s:?}, {max}) = {out:?}");
        }
    }
}

#[test]
fn full_case_folding() {
    assert_eq!(fold_key("Straße"), "strasse");
    assert_eq!(fold_key("ΣΑΣ"), fold_key("σας"));
    assert_eq!(fold_key("\u{0131}"), "\u{0131}", "dotless i folds to itself");
    assert_eq!(fold_key("\u{0130}"), "i\u{0307}", "capital I with dot: full folding (F), not Turkic (T)");
    assert_eq!(fold_key("\u{212A}"), "k", "KELVIN SIGN");
    assert_eq!(fold_key("\u{1E9E}"), "ss");
    assert_eq!(fold_key("\u{FB03}"), "ffi");
    // Cherokee folds to its uppercase.
    assert_eq!(fold_key("\u{AB70}\u{13F8}"), "\u{13A0}\u{13F0}");
    assert_eq!(fold_key("e\u{301}"), "e\u{301}", "no normalization");
}

/// SynapsCLI #447: folding is the table's, at the corpus's Unicode, never
/// the toolchain's. Characters Unicode assigned later fold to themselves
/// on every toolchain, until the table (and the catalog) moves.
#[test]
fn folding_is_pinned_to_unicode_16() {
    assert_eq!(crate::UNICODE_VERSION, (16, 0, 0));
    // 17.0: LATIN CAPITAL LETTER PHARYNGEAL VOICED FRICATIVE (A7CE → A7CF), BERIA ERFE A.
    assert_eq!(fold_key("\u{A7CE}"), "\u{A7CE}");
    assert_eq!(fold_key("\u{16EA0}"), "\u{16EA0}");
    // 18.0: LATIN SMALL LIGATURE LONG S WITH DESCENDER S (→ ss), CAPITAL BARRED A.
    assert_eq!(fold_key("\u{1DF95}"), "\u{1DF95}");
    assert_eq!(fold_key("\u{1DF40}"), "\u{1DF40}");
    let table = &crate::casefold::FOLDS;
    assert!(table.windows(2).all(|w| w[0].0 < w[1].0), "sorted, no repeats");
    assert_eq!(table.len(), 1557);
    assert!(table.iter().all(|(c, f)| *f != c.to_string() && !f.is_empty() && f.chars().count() <= 3));
}
