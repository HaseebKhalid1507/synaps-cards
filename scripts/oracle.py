#!/usr/bin/env python3
"""Fill the corpus's goldens from a second implementation of the spec.

    python3 scripts/oracle.py          # rewrite the goldens in fixtures/
    python3 scripts/oracle.py --check  # exit 1 if any golden differs

The goldens are written here, in Python, from each fixture's own `doc`,
`wait` and answer values, never by cards-spec. The Rust tests then check
cards-spec against them byte for byte. So the two implementations must agree
on:
- the digest (design §1.5): SHA-256 of the normalized doc, compact JSON,
  keys in schema order, `"wait"` last; the envelope line around it;
- the answer message (§3.3): line 1's summary and cuts, line 2's key order;
- the extension's success result (§1.5): the envelope, the options and
  fields cut to the 1 KiB cap, the trailer (`fixtures/results.json`).

And the blocks corpus (`fixtures/blocks/`, docs/blocks.md v1.3) from
`scripts/oracle_blocks.py`, a second implementation of `cards_spec::block`.

The `doc` of each fixture is the C1 corpus's (normalization is not
reimplemented here; cards-spec's output is checked against it directly).
"""

import hashlib
import json
import sys
import unicodedata
from pathlib import Path

import oracle_blocks

ROOT = Path(__file__).resolve().parent.parent
FIX = ROOT / "fixtures"
CATALOG = 1
ENVELOPE_MAX_BYTES = 256
LINE_MAX = 200
PIECE_MAX = 40
RESULT_MAX_BYTES = 1024
LABEL_MAX = 60
LIVE = "The user may also just reply in text. Keep working meanwhile."
WAIT = "You can't continue without it: end your turn now; the answer starts your next turn."

DOC_KEYS = ["question", "detail", "options", "multi", "default", "fields"]
OPTION_KEYS = ["label", "hint", "note"]
FIELD_KEYS = ["id", "label", "type", "value", "placeholder", "optional"]


def compact(v):
    return json.dumps(v, ensure_ascii=False, separators=(",", ":"))


def ordered(obj, keys):
    extra = set(obj) - set(keys)
    assert not extra, extra
    return {k: obj[k] for k in keys if k in obj}


def preimage(doc, wait):
    d = ordered(doc, DOC_KEYS)
    if "options" in d:
        d["options"] = [ordered(o, OPTION_KEYS) for o in d["options"]]
    if "fields" in d:
        d["fields"] = [ordered(f, FIELD_KEYS) for f in d["fields"]]
    d["wait"] = wait
    return compact(d).encode()


def digest(doc, wait):
    return hashlib.sha256(preimage(doc, wait)).hexdigest()[:16]


def envelope(card, doc, wait):
    """§1.5: at most 256 bytes; only the question is cut, ending in …"""
    head = f"Card {card} is showing (cards/{CATALOG} #{digest(doc, wait)}): "
    room = ENVELOPE_MAX_BYTES - len(head.encode())
    q = doc["question"]
    if len(q.encode()) > room:
        while len(q.encode()) + len("…".encode()) > room:
            q = q[:-1]
        q += "…"
    return head + q


# ── ui_sanitize, single line (cards-spec `sanitize.rs`) ──────────────────


def invisible(c):
    o = ord(c)
    ranges = [
        (0x00AD, 0x00AD), (0x034F, 0x034F), (0x061C, 0x061C), (0x115F, 0x1160), (0x17B4, 0x17B5),
        (0x180B, 0x180F), (0x200B, 0x200F), (0x202A, 0x202E), (0x2060, 0x206F), (0x3164, 0x3164),
        (0xFE00, 0xFE0F), (0xFEFF, 0xFEFF), (0xFFA0, 0xFFA0), (0xFFF0, 0xFFF8), (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A), (0xE0000, 0xE0FFF),
    ]
    return any(a <= o <= b for a, b in ranges)


def sanitize_single(s):
    s = s.replace("\r\n", "\n").replace("\r", "\n")
    out = []
    for c in s:
        if c in "\n\t":
            out.append(" ")
        elif c in "\u200c\u200d":
            out.append(c)
        elif unicodedata.category(c) == "Cc" or c in "\u2028\u2029" or invisible(c):
            continue
        else:
            out.append(c)
    return "".join(out)


def result(card, doc, wait):
    """§1.5: the envelope, then the options (numbered, the default marked)
    and the fields, as many as fit in 1024 bytes beside the fixed trailer;
    when the next one wouldn't fit, `  …and N more`."""
    size = lambda s: len(s.encode())
    head = envelope(card, doc, wait)
    tail = [f'Its answer will come as a user message starting "[card {card}]", values as JSON on the next line.',
            WAIT if wait else LIVE]
    default = doc.get("default")
    chosen = [default] if isinstance(default, str) else (default or [])
    items = [f"  {i + 1}. {cut(o['label'], LABEL_MAX)}{' (suggested)' if o['label'] in chosen else ''}"
             for i, o in enumerate(doc.get("options", []))]
    items += [f"  {f['id']}: {cut(f['label'], LABEL_MAX)}{' (optional)' if f['optional'] else ''}"
              for f in doc.get("fields", [])]
    more = lambda n: f"  …and {n} more"
    room = RESULT_MAX_BYTES - size(head) - sum(1 + size(line) for line in tail)
    lines = [head]
    for i, item in enumerate(items):
        after = len(items) - i - 1
        need = 1 + size(item) + (1 + size(more(after)) if after else 0)
        if need > room:
            lines.append(more(len(items) - i))
            break
        room -= 1 + size(item)
        lines.append(item)
    text = "\n".join(lines + tail)
    assert size(text) <= RESULT_MAX_BYTES, size(text)
    return text


def cut(s, n):
    """At most n characters, ending in … when cut; cut to 0, empty."""
    return s if len(s) <= n else (s[: n - 1] + "…" if n else "")


# ── The answer message (§3.3) ────────────────────────────────────────────


def answer_json(doc, values):
    out = {}
    if "choice" in values:
        out["choice"] = values["choice"]
    if "note" in values:
        out["note"] = values["note"]
    if "fields" in values:
        out["fields"] = {f["id"]: values["fields"][f["id"]] for f in doc["fields"]}
    assert set(values) <= {"choice", "note", "fields"}, values
    return compact(out)


def summary(doc, values):
    piece = lambda s: cut(sanitize_single(s), PIECE_MAX)  # noqa: E731
    pieces = []
    choice = values.get("choice")
    if isinstance(choice, str):
        pieces.append(piece(choice))
    elif isinstance(choice, list):
        pieces.append(", ".join(piece(c) for c in choice) if choice else "(none)")
    if "note" in values:
        prompt = next(o.get("note") for o in doc["options"] if o["label"] == choice)
        pieces.append(piece(f"{prompt}: {values['note']}"))
    for f in doc.get("fields", []) if "fields" in values else []:
        v = values["fields"][f["id"]]
        shown = "yes" if v is True else "no" if v is False else (v or "—")
        pieces.append(piece(f"{f['label']}: {shown}"))
    return " · ".join(pieces)


def message(card, doc, kind, values):
    tag = {"answer": "", "changed": " changed", "skipped": " skipped"}[kind]
    if kind == "skipped":
        assert values == {}, values
        body = "(skipped)"
    else:
        body = summary(doc, values)
    line = f"[card {card}{tag}] {sanitize_single(doc['question'])} → {body}"
    return cut(line, LINE_MAX) + "\n" + answer_json(doc, values)


# ── The corpus ───────────────────────────────────────────────────────────


def escape(text):
    """The corpus's style: printable text as is, and every control, format,
    separator (but the space), mark, private-use or unassigned character as
    a \\u escape, so a reader can see it."""
    out = []
    for c in text:
        cat = unicodedata.category(c)
        if c != " " and c not in "\n" and cat[0] in "CZM":
            for unit in c.encode("utf-16-be").hex(" ", 2).split():
                out.append(f"\\u{unit}")
        else:
            out.append(c)
    return "".join(out)


def dump(v):
    return escape(json.dumps(v, ensure_ascii=False, indent=2)) + "\n"


def doc_of(case, name):
    """A case's doc: its own `doc`, or the valid fixture it names."""
    if "doc" in case:
        return case["doc"]
    return json.loads((FIX / "valid" / f"{case['fixture']}.json").read_text())["doc"]


def fill_valid(name, f):
    if "close" in f:
        f["envelope"] = f"Card {f['close']} closed (cards/{CATALOG})."
        return
    f["digest"] = digest(f["doc"], f["wait"])
    f["envelope"] = envelope(f["card"], f["doc"], f["wait"])
    for a in f.get("answers", []):
        a["message"] = message(f["card"], f["doc"], a["kind"], a["values"])


def fill_round_trips(f):
    for case in f["cases"]:
        doc = doc_of(case, case["about"])
        case["text"] = message(case["card"], doc, case["kind"], case["values"])


def fill_results(f):
    for case in f["cases"]:
        if "fixture" in case:
            v = json.loads((FIX / "valid" / f"{case['fixture']}.json").read_text())
            card, doc, wait = v["card"], v["doc"], v["wait"]
        else:
            card, doc, wait = case["card"], case["doc"], case["wait"]
        case["result"] = result(card, doc, wait)


def main():
    check = sys.argv[1:] == ["--check"]
    stale = []
    files = sorted((FIX / "valid").glob("*.json")) + [FIX / "messages" / "round_trips.json", FIX / "results.json"]
    for path in files:
        before = path.read_text()
        f = json.loads(before)
        if path.parent.name == "valid":
            fill_valid(path.stem, f)
        elif path.name == "results.json":
            fill_results(f)
        else:
            fill_round_trips(f)
        after = dump(f)
        if after != before:
            stale.append(path.relative_to(ROOT))
            if not check:
                path.write_text(after)
    for path in oracle_blocks.files():
        before, after = oracle_blocks.fill(path, dump)
        if after != before:
            stale.append(path.relative_to(ROOT))
            if not check:
                path.write_text(after)
    if stale:
        print(("stale: " if check else "wrote: ") + ", ".join(map(str, stale)))
    sys.exit(1 if check and stale else 0)


if __name__ == "__main__":
    main()
