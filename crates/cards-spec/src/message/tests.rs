//! `message`: the design's four examples byte for byte, the line rules,
//! and what reading refuses.

use serde_json::json;

use super::*;

fn doc(v: Value) -> CardDoc {
    match parse_input(&v).unwrap() {
        Input::Create { doc, .. } => doc,
        Input::Close(_) => unreachable!(),
    }
}

fn id(s: &str) -> CardId {
    CardId::parse(s).unwrap()
}

fn answer(d: &CardDoc, v: Value) -> Answer {
    parse_answer(d, &v).unwrap()
}

fn approach() -> CardDoc {
    doc(json!({"question": "Which approach?", "options": ["Refactor the parser", "Rewrite from scratch"]}))
}

fn release() -> CardDoc {
    doc(json!({"question": "Release details", "fields": [{"id": "target", "label": "Target"}, {"id": "notes", "label": "Notes"}]}))
}

/// §3.3, verbatim.
#[test]
fn the_designs_examples() {
    let a = approach();
    let r = release();
    for (card, d, reply, text) in [
        (
            "c4f9x2a",
            &a,
            Reply::Answer(answer(&a, json!({"choice": "Refactor the parser"}))),
            "[card c4f9x2a] Which approach? → Refactor the parser\n{\"choice\":\"Refactor the parser\"}",
        ),
        (
            "c4f9x2a",
            &a,
            Reply::Change(answer(&a, json!({"choice": "Rewrite from scratch"}))),
            "[card c4f9x2a changed] Which approach? → Rewrite from scratch\n{\"choice\":\"Rewrite from scratch\"}",
        ),
        (
            "c81kq0z",
            &r,
            Reply::Answer(answer(&r, json!({"fields": {"target": "prod", "notes": "skip the changelog"}}))),
            "[card c81kq0z] Release details → Target: prod · Notes: skip the changelog\n{\"fields\":{\"target\":\"prod\",\"notes\":\"skip the changelog\"}}",
        ),
        ("c4f9x2a", &a, Reply::Skip, "[card c4f9x2a skipped] Which approach? → (skipped)\n{}"),
    ] {
        assert_eq!(build(&id(card), d, &reply), text);
        let back = parse(text).unwrap_or_else(|| panic!("{text}"));
        assert_eq!((&back.card, back.kind), (&id(card), reply.kind()));
        assert_eq!(back.validate(d).unwrap(), reply);
    }
}

/// Line 1 never breaks, is at most 200 characters, and holds only what
/// sanitizing keeps; line 2 is the values, untouched.
#[test]
fn line_one_is_one_safe_line() {
    let d = doc(json!({"question": "q", "fields": [{"id": "a", "label": "A", "type": "long"}, {"id": "b", "label": "B"}]}));
    for v in
        ["x\ny", "x\r\ny", "\u{202E}x\u{2066}", "x\u{200B}\u{FEFF}y", "\u{1F468}\u{200D}\u{1F469}", "x".repeat(LONG_MAX).as_str(), "\t"]
    {
        let a = answer(&d, json!({"fields": {"a": v.replace('\r', ""), "b": "b"}}));
        let text = build(&id("c4f9x2a"), &d, &Reply::Answer(a.clone()));
        let (line1, line2) = text.split_once('\n').unwrap();
        assert!(!line2.contains('\n'), "{text:?}");
        assert!(line1.chars().count() <= MESSAGE_LINE_MAX);
        assert_eq!(ui_sanitize(line1, Lines::Single), line1, "{line1:?}");
        assert_eq!(line2, a.to_json());
        assert_eq!(parse(&text).unwrap().validate(&d).unwrap(), Reply::Answer(a));
    }
}

#[test]
fn summaries() {
    let d = doc(json!({"question": "q", "options": ["a", {"label": "Other", "note": "Which one?"}]}));
    let a = answer(&d, json!({"choice": "Other", "note": "the parser"}));
    assert_eq!(
        build(&id("c4f9x2a"), &d, &Reply::Answer(a)),
        "[card c4f9x2a] q → Other · Which one?: the parser\n{\"choice\":\"Other\",\"note\":\"the parser\"}"
    );
    let d =
        doc(json!({"question": "q", "options": ["a", "b", "c"], "multi": true, "fields": [{"id": "ok", "label": "OK", "type": "check"}]}));
    let a = answer(&d, json!({"choice": ["c", "a"], "fields": {"ok": false}}));
    assert_eq!(
        build(&id("c4f9x2a"), &d, &Reply::Change(a)),
        "[card c4f9x2a changed] q → c, a · OK: no\n{\"choice\":[\"c\",\"a\"],\"fields\":{\"ok\":false}}"
    );
    // Each piece is cut at 40 characters; the whole line at 200.
    let label = "L".repeat(OPTION_LABEL_MAX);
    let d = doc(json!({"question": "Q".repeat(QUESTION_MAX), "options": [label, "b"]}));
    let text = build(&id("c4f9x2a"), &d, &Reply::Answer(answer(&d, json!({"choice": "L".repeat(60)}))));
    let line1 = text.split('\n').next().unwrap();
    assert_eq!(line1, format!("[card c4f9x2a] {} → {}…", "Q".repeat(120), "L".repeat(39)));
    assert_eq!(line1.chars().count(), 15 + 120 + 3 + 40);
    let d = doc(
        json!({"question": "Q".repeat(QUESTION_MAX), "options": (0..9).map(|i| format!("{i}{}", "L".repeat(59))).collect::<Vec<_>>(), "multi": true}),
    );
    let all: Vec<String> = d.options.iter().map(|o| o.label.clone()).collect();
    let text = build(&id("c4f9x2a"), &d, &Reply::Answer(answer(&d, json!({"choice": all}))));
    let line1 = text.split('\n').next().unwrap();
    assert_eq!(line1.chars().count(), MESSAGE_LINE_MAX);
    assert!(line1.ends_with('…'));
}

#[test]
fn reading_refuses_what_the_writer_never_writes() {
    let ok = "[card c4f9x2a] Which approach? → Refactor the parser\n{\"choice\":\"Refactor the parser\"}";
    assert!(parse(ok).is_some());
    for bad in [
        ok.replace('\n', " "),
        format!("{ok}\n"),
        ok.replace("\n", "\r\n"),
        format!(" {ok}"),
        ok.replace("[card c4f9x2a]", "[card c4f9x2a ]"),
        ok.replace("[card c4f9x2a]", "[Card c4f9x2a]"),
        ok.replace("[card c4f9x2a]", "[card c4f9x2a skipped]"),
        ok.replace("[card c4f9x2a]", "[card c4f9x2a changed skipped]"),
        ok.replace("] Which", "]Which"),
        ok.replace('{', "[{").replace("\"}", "\"}]"),
        "[card c4f9x2a changed] q → (skipped)\n{}".to_string(),
        "[card c4f9x2a skipped] q → (skipped)\n{\"choice\":\"a\"}".to_string(),
        "[card c4f9x2a skipped] q → (skipped)\n{} ".to_string(),
        "[card c4f9x2a skipped] q → (skipped)\nnull".to_string(),
        "[card c4f9x2a skipped] q\u{85} → (skipped)\n{}".to_string(),
    ] {
        assert_eq!(parse(&bad), None, "{bad:?}");
    }
}

#[test]
fn kinds() {
    let a = approach();
    let one = answer(&a, json!({"choice": "Refactor the parser"}));
    assert_eq!(
        [Reply::Answer(one.clone()).kind(), Reply::Change(one).kind(), Reply::Skip.kind()],
        [Kind::Answer, Kind::Change, Kind::Skip]
    );
    let p = parse("[card c4f9x2a skipped] anything at all\n{}").unwrap();
    assert_eq!((p.kind, p.values.clone()), (Kind::Skip, json!({})));
    assert_eq!(p.validate(&a).unwrap(), Reply::Skip, "a skip fits every card");
    let p = parse("[card c4f9x2a changed] x\n{\"choice\":\"Nope\"}").unwrap();
    assert_eq!(p.validate(&a).unwrap_err().to_string(), "choice: not one of the option labels");
}
