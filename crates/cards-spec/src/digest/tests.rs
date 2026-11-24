//! `digest`: SHA-256 against FIPS 180-4's vectors and its padding edges,
//! the preimage's exact bytes, and the envelope's line, cap and binding.
//! Every expected hash here is Python's `hashlib`.

use serde_json::json;

use super::*;
use crate::{Input, QUESTION_MAX, parse_input};

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn card(v: serde_json::Value) -> (CardDoc, bool) {
    match parse_input(&v).unwrap() {
        Input::Create { doc, wait } => (doc, wait),
        Input::Close(_) => unreachable!(),
    }
}

fn id(s: &str) -> CardId {
    CardId::parse(s).unwrap()
}

#[test]
fn sha256_is_fips_180_4() {
    let vectors: [(&[u8], &str); 4] = [
        (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq", "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"),
        (
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
        ),
    ];
    for (msg, want) in vectors {
        assert_eq!(hex(&sha256(msg)), want, "{}", String::from_utf8_lossy(msg));
    }
    assert_eq!(hex(&sha256(&vec![b'a'; 1_000_000])), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
}

/// The lengths where padding spills into another block, or doesn't.
#[test]
fn sha256_padding_edges() {
    for (n, want) in [
        (55, "d5e285683cd4efc02d021a5c62014694958901005d6f71e89e0989fac77e4072"),
        (56, "04c26261370ee7541549d16dee320c723e3fd14671e66a099afe0a377c16888e"),
        (63, "75220b47218278e656f2013bb8f0c455a25eaf01e86c64924e9d48d89776d6f2"),
        (64, "7ce100971f64e7001e8fe5a51973ecdfe1ced42befe7ee8d5fd6219506b5393c"),
        (65, "9537c5fdf120482f7d58d25e9ed583f52c02b4e304ea814db1633ad565aed7e9"),
        (119, "000b48d4edf0fa7bee3c6236ecd2785baa5db4eeb8bb54341b029e0d9fa5fb0c"),
        (120, "13f05a0b594787f5ecd315edc96141bd3243203d1b7d4f0836f37308b276ba98"),
        (128, "24da1b81d0b16df6428eee73c69fcb2a93c76bc6df706f0c6670fe6bfe800464"),
    ] {
        assert_eq!(hex(&sha256(&vec![b'x'; n])), want, "{n} bytes");
    }
}

/// The preimage is the doc as this crate writes it with `wait` last; its
/// digest is the first 8 bytes of the hash.
#[test]
fn preimage_and_digest() {
    let (doc, wait) = card(json!({"question": "Which approach?", "options": ["Refactor the parser", "Rewrite from scratch"]}));
    assert_eq!(
        String::from_utf8(preimage(&doc, wait)).unwrap(),
        r#"{"question":"Which approach?","options":[{"label":"Refactor the parser"},{"label":"Rewrite from scratch"}],"multi":false,"wait":false}"#
    );
    assert_eq!(digest(&doc, false).to_string(), "0174d7927429327c");
    assert_eq!(digest(&doc, true).to_string(), "3d1ca929adf46e7a", "wait is in the digest");
    assert_eq!(&digest(&doc, false).as_bytes()[..], &sha256(&preimage(&doc, false))[..8]);
}

#[test]
fn digests_read_back() {
    let d = Digest::parse("0174d7927429327c").unwrap();
    assert_eq!(d.to_string(), "0174d7927429327c");
    assert_eq!(d.as_bytes(), &[0x01, 0x74, 0xd7, 0x92, 0x74, 0x29, 0x32, 0x7c]);
    for bad in ["", "0174d7927429327", "0174d7927429327c0", "0174D7927429327C", "0174d7927429327g", " 174d7927429327c", "0174d79274２9327c"]
    {
        assert_eq!(Digest::parse(bad), None, "{bad}");
    }
}

#[test]
fn envelopes_read_back_and_bind() {
    let (doc, wait) = card(json!({"question": "Which approach?", "options": ["Refactor the parser", "Rewrite from scratch"]}));
    let e = Envelope::showing(id("c4f9x2a"), &doc, wait);
    let line = e.to_string();
    assert_eq!(line, "Card c4f9x2a is showing (cards/1 #0174d7927429327c): Which approach?");
    let result = format!("{line}\n  1. Refactor the parser\n  2. Rewrite from scratch\nIts answer will come …");
    assert_eq!(Envelope::parse(&result), Some(e.clone()), "line 1 of a whole result");
    assert_eq!(Envelope::parse(&line), Some(e.clone()));
    assert!(e.binds(&doc, wait));
    assert_eq!(e.card(), &id("c4f9x2a"));
    // A hook flipped wait, or rewrote the doc: no binding (§2.2).
    assert!(!e.binds(&doc, !wait));
    let (other, _) = card(json!({"question": "Which approach?", "options": ["Refactor the parser", "Rewrite it"]}));
    assert!(!e.binds(&other, wait));
    // The question is display only.
    let renamed = Envelope::parse("Card c4f9x2a is showing (cards/1 #0174d7927429327c): Something else").unwrap();
    assert!(renamed.binds(&doc, wait));
    // A catalog this build doesn't know reads, and never binds.
    let next = Envelope::parse("Card c4f9x2a is showing (cards/2 #0174d7927429327c): Which approach?").unwrap();
    assert!(matches!(next, Envelope::Showing { catalog: 2, .. }) && !next.binds(&doc, wait));

    let closed = Envelope::closed(id("c4f9x2a"));
    assert_eq!(closed.to_string(), "Card c4f9x2a closed (cards/1).");
    assert_eq!(Envelope::parse("Card c4f9x2a closed (cards/1)."), Some(closed.clone()));
    assert!(!closed.binds(&doc, wait), "a close binds no card");
}

#[test]
fn anything_else_is_no_envelope() {
    let ok = "Card c4f9x2a is showing (cards/1 #0174d7927429327c): Which approach?";
    for bad in [
        "",
        "card c4f9x2a is showing (cards/1 #0174d7927429327c): Which approach?",
        " Card c4f9x2a is showing (cards/1 #0174d7927429327c): Which approach?",
        "Card C4F9X2A is showing (cards/1 #0174d7927429327c): Which approach?",
        "Card s_3f2a91c0d4e2a871 is showing (cards/1 #0174d7927429327c): Which approach?",
        "Card c4f9x2a is showing (cards/01 #0174d7927429327c): Which approach?",
        "Card c4f9x2a is showing (cards/ #0174d7927429327c): Which approach?",
        "Card c4f9x2a is showing (cards/99999999999 #0174d7927429327c): Which approach?",
        "Card c4f9x2a is showing (cards/1 #0174D7927429327C): Which approach?",
        "Card c4f9x2a is showing (cards/1 #0174d7927429327): Which approach?",
        "Card c4f9x2a is showing (cards/1 #0174d7927429327c) Which approach?",
        "Card c4f9x2a is showing (cards/1 #0174d7927429327c): ",
        "Card c4f9x2a is showing (cards/1 #0174d7927429327c): Which approach?\r",
        "Card c4f9x2a is sho",
        "Card c4f9x2a closed (cards/1). ",
        "Card c4f9x2a closed (cards/1)",
        "Card c4f9x2a closed.",
        "Error: options: at most 9 (got 12); ask in plain text or narrow the choices",
    ] {
        assert_eq!(Envelope::parse(bad), None, "{bad:?}");
    }
    assert!(Envelope::parse(ok).is_some());
    // Over the cap: a line no extension writes.
    let long = format!("Card c4f9x2a is showing (cards/1 #0174d7927429327c): {}", "x".repeat(ENVELOPE_MAX_BYTES - 53 + 1));
    assert_eq!(long.len(), ENVELOPE_MAX_BYTES + 1);
    assert_eq!(Envelope::parse(&long), None);
    assert!(Envelope::parse(&long[..ENVELOPE_MAX_BYTES]).is_some());
}

/// §1.5: line 1 is at most 256 bytes. Only the question is cut, at a
/// character, ending in `…`; the cut line still binds. Tested at N and
/// N+1 bytes of question, in a 4-byte script.
#[test]
fn the_envelope_is_at_most_256_bytes() {
    let room = ENVELOPE_MAX_BYTES - "Card c4f9x2a is showing (cards/1 #0174d7927429327c): ".len();
    assert_eq!(room, 203);
    let gothic = '\u{10330}';
    for (question, cut) in [
        (format!("abc{}", gothic.to_string().repeat(50)), false),
        (format!("abcd{}", gothic.to_string().repeat(50)), true),
        (gothic.to_string().repeat(QUESTION_MAX), true),
    ] {
        let (doc, wait) = card(json!({"question": question, "options": ["a", "b"]}));
        let e = Envelope::showing(id("c4f9x2a"), &doc, wait);
        let line = e.to_string();
        assert!(line.len() <= ENVELOPE_MAX_BYTES, "{} bytes", line.len());
        let Envelope::Showing { question: shown, .. } = &e else { unreachable!() };
        if cut {
            assert!(shown.ends_with('…') && question.starts_with(shown.trim_end_matches('…')), "{shown}");
            assert!(line.len() + gothic.len_utf8() > ENVELOPE_MAX_BYTES, "cut no more than it must");
        } else {
            assert_eq!(shown, &question);
            assert_eq!(line.len(), ENVELOPE_MAX_BYTES);
        }
        let back = Envelope::parse(&line).unwrap();
        assert_eq!(back, e);
        assert!(back.binds(&doc, wait));
    }
}
