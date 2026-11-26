"""The blocks corpus's goldens (docs/blocks.md v1.3, B1), from a second
implementation of `cards_spec::block`. `scripts/oracle.py` runs it, with
the cards goldens, and `--check` fails if any golden here is stale.

Written here, in Python, never by cards-spec, then checked byte for byte by
`crates/cards-spec/tests/blocks.rs`:
- a valid call's normalized ops, their digest and the success result
  (§1.4, §1.6): this file normalizes on its own, with its own canonical
  JSON writer (keys in schema order, maps in byte order, numbers as
  cards-spec writes them);
- the generation and the incarnation tag (§1.2);
- an edit message built from its parts (§3.3);
- a message's body and notice lines (§3.5), read with a regular
  expression, the design's own pattern;
- `limits.json`: every numeric bound at N and N+1, generated here (the
  inputs, and the refusal each N+1 must get).

Validation is not reimplemented: a valid case is assumed valid (cards-spec
checks that), and a refusal's text is written by hand, in the fixture or in
`limits()` below.
"""

import hashlib
import json
import re
import unicodedata
from decimal import Decimal
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIX = ROOT / "fixtures" / "blocks"

STEP_STATES = ["todo", "doing", "done", "failed", "skipped"]
HUNK_STATES = ["open", "kept", "dropped"]
PIECE_MAX = 40
LINE_MAX = 200

# ── Strings (cards-spec `sanitize.rs`) ───────────────────────────────────


def invisible(c):
    o = ord(c)
    ranges = [
        (0x00AD, 0x00AD), (0x034F, 0x034F), (0x061C, 0x061C), (0x115F, 0x1160), (0x17B4, 0x17B5),
        (0x180B, 0x180F), (0x200B, 0x200F), (0x202A, 0x202E), (0x2060, 0x206F), (0x3164, 0x3164),
        (0xFE00, 0xFE0F), (0xFEFF, 0xFEFF), (0xFFA0, 0xFFA0), (0xFFF0, 0xFFF8), (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A), (0xE0000, 0xE0FFF),
    ]
    return any(a <= o <= b for a, b in ranges)


def sanitize(s, multi=False):
    s = s.replace("\r\n", "\n").replace("\r", "\n")
    out = []
    for c in s:
        if c == "\n" and multi:
            out.append(c)
        elif c in "\n\t":
            out.append(" ")
        elif c in "\u200c\u200d":
            out.append(c)
        elif unicodedata.category(c) == "Cc" or c in "\u2028\u2029" or invisible(c):
            continue
        else:
            out.append(c)
    return "".join(out)


def cut(s, n):
    return s if len(s) <= n else (s[: n - 1] + "…" if n else "")


# ── Canonical JSON ────────────────────────────────────────────────────────


def fnum(x):
    """A non-whole float as serde_json writes an f64 (ryu): the shortest
    digits that read back; plain below 10^16, `0.000ddd` down to 10^-5,
    then `d.ddde-N`."""
    sign, digits, exp = Decimal(repr(x)).as_tuple()
    ds = "".join(map(str, digits))
    kk = len(ds) + exp
    s = "-" if sign else ""
    if 0 < kk <= 16:
        return s + ds[:kk] + "." + ds[kk:]
    if -5 < kk <= 0:
        return s + "0." + "0" * -kk + ds
    return s + ds[0] + ("." + ds[1:] if len(ds) > 1 else "") + "e" + str(kk - 1)


def canon(v):
    """Compact JSON: dicts in their own order, strings as serde_json
    escapes them, a whole number as an integer."""
    if v is True:
        return "true"
    if v is False:
        return "false"
    if v is None:
        return "null"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        return str(int(v)) if v == int(v) else fnum(v)
    if isinstance(v, str):
        return json.dumps(v, ensure_ascii=False)
    if isinstance(v, list):
        return "[" + ",".join(canon(x) for x in v) + "]"
    return "{" + ",".join(json.dumps(k, ensure_ascii=False) + ":" + canon(x) for k, x in v.items()) + "}"


def num(x):
    x = float(x)
    if x == 0:
        return 0
    return int(x) if x == int(x) else x


# ── Normalizing (§1.3–§1.5) ──────────────────────────────────────────────


def one(s):
    return sanitize(s)


def put_opt(out, key, value):
    if value:
        out[key] = value


def step(s):
    d = {"id": s["id"], "label": one(s["label"]), "state": s.get("state", "todo")}
    put_opt(d, "note", one(s.get("note", "")))
    return d


def hunk(h):
    d = {"id": h["id"]}
    put_opt(d, "at", one(h.get("at", "")))
    d["lines"] = [one(x) for x in h["lines"]]
    d["state"] = h.get("state", "open")
    return d


def body(kind, b):
    if kind == "plan":
        return {"steps": [step(s) for s in b["steps"]]}
    if kind == "status":
        d = {"text": one(b["text"])}
        put_opt(d, "detail", one(b.get("detail", "")))
        if "progress" in b:
            d["progress"] = {"n": b["progress"]["n"], "of": b["progress"]["of"]}
        d["state"] = b.get("state", "working")
        return d
    if kind == "diff":
        d = {"file": one(b["file"]), "state": b["state"]}
        if "from" in b:
            d["from"] = "edits"
        if b.get("hunks"):
            d["hunks"] = [hunk(h) for h in b["hunks"]]
        return d
    if kind == "note":
        return {"text": sanitize(b["text"], multi=True)}
    if kind == "metric":
        d = {"label": one(b["label"]), "value": one(b["value"])}
        put_opt(d, "was", one(b.get("was", "")))
        put_opt(d, "unit", one(b.get("unit", "")))
        if b.get("series"):
            d["series"] = [num(x) for x in b["series"]]
        d["state"] = b.get("state", "neutral")
        return d
    if kind == "file":
        d = {"path": one(b["path"])}
        if "line" in b:
            d["line"] = b["line"]
        put_opt(d, "note", one(b.get("note", "")))
        d["state"] = b["state"]
        return d
    raise ValueError(kind)


def set_value(path, v):
    if isinstance(v, bool):
        return v
    if isinstance(v, str):
        return sanitize(v, multi=path == "text")
    if isinstance(v, dict):
        return {"n": v["n"], "of": v["of"]}
    return v


def appended(path, items):
    if path == "steps":
        return [step(s) for s in items]
    if path == "hunks":
        return [hunk(h) for h in items]
    if path == "series":
        return [num(x) for x in items]
    return [one(x) for x in items]  # hunks.<id>.lines


def parts(o):
    """A patch's parts in byte order: append, order, remove, set (state
    sugar in set, on the list its words name)."""
    out = {}
    if o.get("append"):
        out["append"] = {k: appended(k, v) for k, v in sorted(o["append"].items())}
    if o.get("order"):
        out["order"] = {k: list(v) for k, v in sorted(o["order"].items())}
    if o.get("remove"):
        out["remove"] = list(o["remove"])
    sets = dict(o.get("set", {}))
    for item, word in o.get("state", {}).items():
        sets[f"{'steps' if word in STEP_STATES else 'hunks'}.{item}.state"] = word
    if sets:
        out["set"] = {k: set_value(k, v) for k, v in sorted(sets.items())}
    return out


def title(out, o):
    put_opt(out, "title", one(o.get("title", "")))


def op(o):
    t = o["op"]
    d = {"op": t}
    if t == "put":
        d["id"] = o["id"]
        d["kind"] = o["kind"]
        title(d, o)
        target = o.get("target") or ("rail" if "slot" in o else "transcript")
        d["target"] = target
        if target == "rail":
            d["slot"] = o.get("slot", "upper")
        if "at" in o:
            d["at"] = {"after": o["at"]["after"]}
        d["body"] = body(o["kind"], o["body"])
    elif t == "patch":
        d["id"] = o["id"]
        d.update(parts(o))
    elif t == "replace":
        d["id"] = o["id"]
        d["kind"] = o["kind"]
        title(d, o)
        d["body"] = body(o["kind"], o["body"])
        put_opt(d, "why", one(o.get("why", "")))
    elif t == "collapse":
        if o.get("run"):
            d["run"] = True
            d["summary"] = one(o["summary"])
        else:
            if "id" in o:
                d["id"] = o["id"]
            else:
                d["ids"] = list(o["ids"])
            d["open"] = bool(o.get("open", False))
    elif t == "merge":
        d["into"] = o["into"]
        title(d, o)
        d["ids"] = list(o["ids"])
    elif t == "move":
        d["id"] = o["id"]
        target = o.get("target") or ("rail" if "slot" in o else None)
        if target:
            d["target"] = target
        if "slot" in o:
            d["slot"] = o["slot"]
        if "at" in o:
            d["at"] = {"after": o["at"]["after"]}
    elif t == "remove":
        if "id" in o:
            d["id"] = o["id"]
        else:
            d["ids"] = list(o["ids"])
        put_opt(d, "why", one(o.get("why", "")))
    else:
        raise ValueError(t)
    return d


def normalize(call):
    return {"ops": [op(o) for o in call["ops"]]}


def digest(normalized):
    return hashlib.sha256(canon(normalized).encode()).hexdigest()[:16]


def result(normalized):
    return f"ok {len(normalized['ops'])} ops (blocks/1 #{digest(normalized)})"


def fill_call(case):
    if "error" in case:
        for k in ("normalized", "digest", "result"):
            case.pop(k, None)
        return
    n = normalize(case["input"])
    case["normalized"] = n
    case["digest"] = digest(n)
    case["result"] = result(n)


# ── The generation and the tag (§1.2) ────────────────────────────────────


def canon_sorted(v):
    """Compact JSON, keys sorted, `_synaps_*` keys left out, at every level."""
    if isinstance(v, dict):
        keys = sorted(k for k in v if not k.startswith("_synaps_"))
        return "{" + ",".join(json.dumps(k, ensure_ascii=False) + ":" + canon_sorted(v[k]) for k in keys) + "}"
    if isinstance(v, list):
        return "[" + ",".join(canon_sorted(x) for x in v) + "]"
    assert not isinstance(v, float), "the tag fixtures keep floats out of first messages"
    return canon(v)


def generation_preimage(first):
    keep = {k: first[k] for k in ("content", "role") if isinstance(first, dict) and k in first}
    return canon_sorted(keep)


def fill_tag(case):
    pre = generation_preimage(case["first_message"])
    gen = hashlib.sha256(pre.encode()).hexdigest()
    case["generation_preimage"] = pre
    case["generation"] = gen
    tag_pre = f"blocks/1\0{gen}\0{case['id']}\0{case['creation']}\0{case['tool_id']}\0{case['op_index']}"
    case["tag_preimage"] = tag_pre
    case["tag"] = hashlib.sha256(tag_pre.encode()).hexdigest()[:32]


# ── The edit message (§3.3) ──────────────────────────────────────────────


def user_parts(p):
    """Line 2's parts: append, order, set, in byte order; a user's step as
    {id, label, state}."""
    out = {}
    if p.get("append"):
        out["append"] = {k: [{"id": s["id"], "label": one(s["label"]), "state": s.get("state", "todo")} for s in v] for k, v in sorted(p["append"].items())}
    if p.get("order"):
        out["order"] = {k: list(v) for k, v in sorted(p["order"].items())}
    if p.get("set"):
        out["set"] = {k: set_value(k, v) for k, v in sorted(p["set"].items())}
    return out


def edit_text(case):
    name = one(case.get("title") or "") or case["id"]
    pieces = [cut(one(p), PIECE_MAX) for p in case["pieces"]]
    lead = f"back to r{case['back_to']}: " if case.get("back_to") else ""
    line = cut(f"[block {case['id']} edited r{case['base']}] {name} → {lead}{' · '.join(pieces)}", LINE_MAX)
    line2 = {"of": case["of"], **user_parts(case["parts"])}
    return line + "\n" + canon(line2)


def fill_edit(case):
    case["text"] = edit_text(case)
    if "design" in case:
        assert case["text"] == case["design"], (case["about"], case["text"], case["design"])


# ── Notices (§3.5) ───────────────────────────────────────────────────────

NOTICE = re.compile(
    r"\[block ([a-z][a-z0-9_-]{0,23} v[1-9][0-9]{0,8}(?:, [a-z][a-z0-9_-]{0,23} v[1-9][0-9]{0,8}){0,7}) (dismissed|shown)\]"
)


def split(text):
    body, lines = text, []
    while len(lines) < 2 and "\n" in body:
        i = body.rindex("\n")
        m = NOTICE.fullmatch(body[i + 1 :])
        if not m or any(line["visibility"] == m.group(2) for line in lines):
            break
        notices = [[e.split(" v")[0], int(e.split(" v")[1])] for e in m.group(1).split(", ")]
        lines.insert(0, {"visibility": m.group(2), "notices": notices})
        body = body[:i]
    ids = [n[0] for line in lines for n in line["notices"]]
    twice = sorted({i for i in ids if ids.count(i) > 1}, key=ids.index)
    return body, lines, twice


def fill_notice(case):
    case["body"], case["lines"], case["named_twice"] = split(case["text"])


# ── limits.json: every bound at N and N+1 ────────────────────────────────


def text_of(nbytes, max_chars):
    """`nbytes` of UTF-8 (at least 1) in at most `max_chars` characters:
    4-byte characters, then one of 1 to 3 bytes."""
    s = "😀" * (nbytes // 4) + ["", "a", "é", "€"][nbytes % 4]
    assert 1 <= len(s) <= max_chars, (nbytes, max_chars)
    return s


def spread(need, caps):
    """`need` split over parts, each from 1 to its cap."""
    out = []
    for i, cap in enumerate(caps):
        b = min(cap, need - (len(caps) - 1 - i))
        assert b >= 1, (need, caps)
        out.append(b)
        need -= b
    assert need == 0, (need, caps)
    return out


def sized(target, kind, make, caps):
    """The body `make(bytes)` whose normalized size is exactly `target`."""
    overhead = len(canon(body(kind, make([1] * len(caps)))).encode()) - len(caps)
    made = make(spread(target - overhead, caps))
    assert len(canon(body(kind, made)).encode()) == target, kind
    return made


def call(*ops):
    return {"ops": list(ops)}


def put(kind, b, **kw):
    return {"op": "put", "id": kw.pop("id", "b"), "kind": kind, **kw, "body": b}


def chars(n, c="x"):
    return c * n


def limits():
    """(about, input, error or None): each bound at N, then N+1."""
    cases = []

    def pair(about, ok, bad, error):
        cases.append((f"{about}: at the bound", ok, None))
        cases.append((f"{about}: one past it", bad, error))

    rm = {"op": "remove", "id": "x"}
    pair("ops in a call (16)", call(*[rm] * 16), call(*[rm] * 17), "ops: at most 16 (got 17); split them over calls")

    def note_ops(total):
        # Notes of 2-byte text, padded so the compact input is `total` bytes.
        base = call(*[put("note", {"text": "é"}, id=f"n{i}") for i in range(5)])
        need = total - len(canon(base).encode())
        texts = spread(need + 5 * 2, [2000] * 5)
        return call(*[put("note", {"text": "é" * (b // 2) + "a" * (b % 2)}, id=f"n{i}") for i, b in enumerate(texts)])

    pair("a call's bytes (8192)", note_ops(8192), note_ops(8193), "input: over 8192 bytes; split the ops over calls or patch less")
    diff = lambda line: put("diff", {"file": "f", "state": "proposed", "hunks": [{"id": "h", "lines": [line]}]})  # noqa: E731
    pair("an op's depth (6, scalars counted)", call(diff("x")), call(diff(["x"])), "input: nested too deep: an op nests at most 6 levels")
    pair(
        "a block id (24 characters)",
        call(put("note", {"text": "x"}, id="a" + chars(23, "b"))),
        call(put("note", {"text": "x"}, id="a" + chars(24, "b"))),
        f'ops[0].id: "a{chars(24, "b")}" must match ^[a-z][a-z0-9_-]{{0,23}}$: a lowercase letter, then up to 23 lowercase letters, digits, _ or -',
    )
    plan = lambda *steps: put("plan", {"steps": list(steps)})  # noqa: E731
    pair(
        "an item id (24 characters)",
        call(plan({"id": chars(24, "0"), "label": "L"})),
        call(plan({"id": chars(25, "0"), "label": "L"})),
        f'ops[0].body.steps[0].id: "{chars(25, "0")}" must match ^[a-z0-9][a-z0-9_-]{{0,23}}$: a lowercase letter or digit, then up to 23 of them, _ or -',
    )
    patch = lambda **kw: {"op": "patch", "id": "b", **kw}  # noqa: E731
    pair(
        "a path's field name (16 characters; no kind has one this long)",
        call(patch(set={chars(16, "a"): "x"})),
        call(patch(set={chars(17, "a"): "x"})),
        None,
    )
    cases[-2] = (cases[-2][0], cases[-2][1], f'ops[0].set.{chars(16, "a")}: "{chars(16, "a")}" is not a field of any kind')
    cases[-1] = (cases[-1][0], cases[-1][1], f"ops[0].set.{chars(17, 'a')}: not a path: a field (title), or an item's field (steps.fix.label)")
    note = lambda **kw: put("note", {"text": "x"}, **kw)  # noqa: E731
    pair("a title (80 characters)", call(note(title=chars(80, "😀"))), call(note(title=chars(81, "😀"))), "ops[0].title: at most 80 characters (got 81)")
    rm_why = lambda why: {"op": "remove", "id": "x", "why": why}  # noqa: E731
    pair("a why (80 characters)", call(rm_why(chars(80))), call(rm_why(chars(81))), "ops[0].why: at most 80 characters (got 81)")
    run = lambda s: {"op": "collapse", "run": True, "summary": s}  # noqa: E731
    pair("a run's summary (80 characters)", call(run(chars(80))), call(run(chars(81))), "ops[0].summary: at most 80 characters (got 81)")
    ids = lambda n: [f"b{i}" for i in range(n)]  # noqa: E731
    pair("ids a collapse names (8)", call({"op": "collapse", "ids": ids(8)}), call({"op": "collapse", "ids": ids(9)}), "ops[0].ids: at most 8 (got 9); use another op for the rest")
    pair("ids a remove names (8)", call({"op": "remove", "ids": ids(8)}), call({"op": "remove", "ids": ids(9)}), "ops[0].ids: at most 8 (got 9); use another op for the rest")
    pair("ids a merge names (8)", call({"op": "merge", "into": "g", "ids": ids(8)}), call({"op": "merge", "into": "g", "ids": ids(9)}), "ops[0].ids: at most 8 (got 9); use another op for the rest")

    # plan
    steps = lambda n: [{"id": f"s{i}", "label": "L"} for i in range(n)]  # noqa: E731
    pair("a plan's steps (20)", call(plan(*steps(20))), call(plan(*steps(21))), "ops[0].body.steps: at most 20 steps (got 21); split the plan or fold done steps into a note")
    pair(
        "a step's label (100 characters)",
        call(plan({"id": "a", "label": chars(100, "€")})),
        call(plan({"id": "a", "label": chars(101, "€")})),
        "ops[0].body.steps[0].label: at most 100 characters (got 101); put the rest in its note",
    )
    pair(
        "a step's note (80 characters)",
        call(plan({"id": "a", "label": "L", "note": chars(80)})),
        call(plan({"id": "a", "label": "L", "note": chars(81)})),
        "ops[0].body.steps[0].note: at most 80 characters (got 81)",
    )
    plan_of = lambda bs: {"steps": [{"id": f"s{i}", "label": text_of(b, 100)} for i, b in enumerate(bs)]}  # noqa: E731
    pair(
        "a plan's body (4096 bytes, normalized)",
        call(put("plan", sized(4096, "plan", plan_of, [400] * 20))),
        call(put("plan", sized(4097, "plan", plan_of, [400] * 20))),
        "ops[0].body: over 4096 bytes for a plan (got 4097, normalized); shorten it or split it over blocks",
    )
    pair(
        "steps one append adds (20)",
        call(patch(append={"steps": steps(20)})),
        call(patch(append={"steps": steps(21)})),
        "ops[0].append.steps: at most 20 steps (got 21)",
    )

    # status
    status = lambda **b: put("status", {"text": "T", **b})  # noqa: E731
    pair("a status's text (80 characters)", call(status(text=chars(80))), call(status(text=chars(81))), "ops[0].body.text: at most 80 characters (got 81); put the rest in detail")
    pair("a status's detail (120 characters)", call(status(detail=chars(120))), call(status(detail=chars(121))), "ops[0].body.detail: at most 120 characters (got 121)")
    pair(
        "a status's progress (of 10^6)",
        call(status(progress={"n": 1000000, "of": 1000000})),
        call(status(progress={"n": 1, "of": 1000001})),
        "ops[0].body.progress.of: must be a whole number from 1 to 1000000 (got 1000001)",
    )
    pair("a status's progress (n at most of)", call(status(progress={"n": 7, "of": 7})), call(status(progress={"n": 8, "of": 7})), "ops[0].body.progress.n: 8 is more than of (7)")
    status_of = lambda bs: {"text": text_of(bs[0], 80), "detail": text_of(bs[1], 120)}  # noqa: E731
    pair(
        "a status's body (512 bytes, normalized)",
        call(put("status", sized(512, "status", status_of, [320, 480]))),
        call(put("status", sized(513, "status", status_of, [320, 480]))),
        "ops[0].body: over 512 bytes for a status (got 513, normalized); shorten it or split it over blocks",
    )

    # diff
    d = lambda **b: put("diff", {"file": "f", "state": "proposed", **b})  # noqa: E731
    hunks = lambda n, lines=1: [{"id": f"h{i}", "lines": ["+x"] * lines} for i in range(n)]  # noqa: E731
    pair("a diff's file (200 characters)", call(d(file=chars(200))), call(d(file=chars(201))), "ops[0].body.file: at most 200 characters (got 201)")
    pair("a diff's hunks (6)", call(d(hunks=hunks(6))), call(d(hunks=hunks(7))), "ops[0].body.hunks: at most 6 hunks (got 7); propose fewer at once")
    pair("a diff's lines in all (120)", call(d(hunks=hunks(6, 20))), call(d(hunks=[*hunks(5, 20), {"id": "h5", "lines": ["+x"] * 21}])), "ops[0].body.hunks: at most 120 lines in all (got 121)")
    pair(
        "a hunk's lines (120)",
        call(d(hunks=[{"id": "h", "lines": ["+x"] * 120}])),
        call(d(hunks=[{"id": "h", "lines": ["+x"] * 121}])),
        "ops[0].body.hunks[0].lines: at most 120 lines (got 121)",
    )
    pair(
        "a diff line (160 characters)",
        call(d(hunks=[{"id": "h", "lines": [chars(160)]}])),
        call(d(hunks=[{"id": "h", "lines": [chars(161)]}])),
        "ops[0].body.hunks[0].lines[0]: at most 160 characters (got 161)",
    )
    pair(
        "a hunk's at (80 characters)",
        call(d(hunks=[{"id": "h", "at": chars(80), "lines": ["+x"]}])),
        call(d(hunks=[{"id": "h", "at": chars(81), "lines": ["+x"]}])),
        "ops[0].body.hunks[0].at: at most 80 characters (got 81)",
    )
    diff_of = lambda bs: {"file": "f", "state": "proposed", "hunks": [{"id": "h", "lines": [text_of(b, 160) for b in bs]}]}  # noqa: E731
    pair(
        "a diff's body (6144 bytes, normalized)",
        call(put("diff", sized(6144, "diff", diff_of, [640] * 12))),
        call(put("diff", sized(6145, "diff", diff_of, [640] * 12))),
        "ops[0].body: over 6144 bytes for a diff (got 6145, normalized); shorten it or split it over blocks",
    )
    pair(
        "lines one append adds to a hunk (120)",
        call(patch(append={"hunks.h1.lines": ["+x"] * 120})),
        call(patch(append={"hunks.h1.lines": ["+x"] * 121})),
        "ops[0].append.\"hunks.h1.lines\": at most 120 lines (got 121)",
    )

    # note
    pair("a note's text (1500 characters)", call(put("note", {"text": chars(1500)})), call(put("note", {"text": chars(1501)})), "ops[0].body.text: at most 1500 characters (got 1501); split it over notes")
    pair(
        "a note's lines (30)",
        call(put("note", {"text": "\n".join(["l"] * 30)})),
        call(put("note", {"text": "\n".join(["l"] * 31)})),
        "ops[0].body.text: at most 30 lines (got 31)",
    )
    note_of = lambda bs: {"text": text_of(bs[0], 1500)}  # noqa: E731
    pair(
        "a note's body (2048 bytes, normalized)",
        call(put("note", sized(2048, "note", note_of, [6000]))),
        call(put("note", sized(2049, "note", note_of, [6000]))),
        "ops[0].body: over 2048 bytes for a note (got 2049, normalized); shorten it or split it over blocks",
    )

    # metric
    m = lambda **b: put("metric", {"label": "L", "value": "1", **b})  # noqa: E731
    pair("a metric's label (60 characters)", call(m(label=chars(60))), call(m(label=chars(61))), "ops[0].body.label: at most 60 characters (got 61)")
    pair("a metric's value (16 characters)", call(m(value=chars(16))), call(m(value=chars(17))), "ops[0].body.value: at most 16 characters (got 17); format it shorter")
    pair("a metric's was (16 characters)", call(m(was=chars(16))), call(m(was=chars(17))), "ops[0].body.was: at most 16 characters (got 17)")
    pair("a metric's unit (16 characters)", call(m(unit=chars(16))), call(m(unit=chars(17))), "ops[0].body.unit: at most 16 characters (got 17)")
    pair("a metric's series (32)", call(m(series=[1] * 32)), call(m(series=[1] * 33)), "ops[0].body.series: at most 32 numbers (got 33); an append rolls off the oldest")
    pair(
        "a series number (below 10^15)",
        call(m(series=[999999999999999, -999999999999999])),
        call(m(series=[1000000000000000])),
        "ops[0].body.series[0]: must be a number between -10^15 and 10^15 (got 1000000000000000)",
    )
    pair(
        "numbers one append adds to a series (32)",
        call(patch(append={"series": [0.5] * 32})),
        call(patch(append={"series": [0.5] * 33})),
        "ops[0].append.series: at most 32 numbers (got 33); an append rolls off the oldest",
    )
    metric_of = lambda bs: {  # noqa: E731
        "label": text_of(bs[0], 60),
        "value": text_of(bs[1], 16),
        "was": text_of(bs[2], 16),
        "unit": text_of(bs[3], 16),
        "series": [-123456789012345] * 32,
    }
    pair(
        "a metric's body (1024 bytes, normalized)",
        call(put("metric", sized(1024, "metric", metric_of, [240, 64, 64, 64]))),
        call(put("metric", sized(1025, "metric", metric_of, [240, 64, 64, 64]))),
        "ops[0].body: over 1024 bytes for a metric (got 1025, normalized); shorten it or split it over blocks",
    )

    # file
    f = lambda **b: put("file", {"path": "p", "state": "reading", **b})  # noqa: E731
    pair("a file's path (200 characters)", call(f(path=chars(200))), call(f(path=chars(201))), "ops[0].body.path: at most 200 characters (got 201)")
    pair("a file's note (80 characters)", call(f(note=chars(80))), call(f(note=chars(81))), "ops[0].body.note: at most 80 characters (got 81)")
    pair("a file's line (10^7)", call(f(line=10000000)), call(f(line=10000001)), "ops[0].body.line: must be a whole number from 1 to 10000000 (got 10000001)")
    file_of = lambda bs: {"path": text_of(bs[0], 200), "note": text_of(bs[1], 80), "state": "reading"}  # noqa: E731
    pair(
        "a file's body (512 bytes, normalized)",
        call(put("file", sized(512, "file", file_of, [800, 320]))),
        call(put("file", sized(513, "file", file_of, [800, 320]))),
        "ops[0].body: over 512 bytes for a file (got 513, normalized); shorten it or split it over blocks",
    )
    return cases


def write_limits():
    out = {"cases": []}
    for about, inp, error in limits():
        case = {"about": about, "input": inp}
        if error is not None:
            case["error"] = error
        fill_call(case)
        if error is None:
            # The bulky cases keep their digest only: it pins the bytes.
            if len(canon(case["normalized"]).encode()) > 1024:
                del case["normalized"]
        out["cases"].append(case)
    return out


# ── The files ────────────────────────────────────────────────────────────


def fill(path, dump):
    """`(before, after)` for one fixture file."""
    before = path.read_text() if path.exists() else ""
    if path.name == "limits.json":
        return before, dump(write_limits())
    f = json.loads(before)
    for case in f["cases"]:
        {
            "valid.json": fill_call,
            "invalid.json": fill_call,
            "tags.json": fill_tag,
            "edits.json": fill_edit,
            "notices.json": fill_notice,
        }.get(path.name, lambda c: None)(case)
    return before, dump(f)


def files():
    return [FIX / n for n in ("valid.json", "invalid.json", "limits.json", "tags.json", "edits.json", "look_alikes.json", "notices.json")]
