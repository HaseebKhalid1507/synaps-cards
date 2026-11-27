"""The probe pairs and the proxy's calibration (docs/blocks.md §2.8, "How
the attributed numbers are counted").

**A probe pair** (`pair`) is two requests that differ only by one
`cards_block` call. The same short system prompt and the same tool
definitions go in both, and so do the user's "Probe." and the assistant's
"Working.":
- `with(c)`: the assistant message also calls `cards_block` with input
  `c`, and the next user message gives that call's result, the fixed
  receipt `RECEIPT`, before its "Continue.";
- `without`: no call, no result, the same "Continue.".

So with(c) − without, in reported input tokens, is the call (its name, id,
wrapper and arguments) plus its result (its wrapper and `RECEIPT`). The
pair is a valid conversation on every provider: a call always has its
result.

**Counting a pair:** with the provider's token-count endpoint when it has
one (exact, and free); otherwise each request is sent as a completion of
one output token, and its reported input (uncached + cache read + cache
write) is the count. Both requests go uncached or both cached: the
probes set no cache breakpoint.

**The calibration** (no endpoint): for each call of the fixed calibration
set (`eval/calibration.jsonl`, ≥ 200 valid calls of varied length), the
pair's difference d(c) is fitted by least squares to

    d(c) = intercept + ratio × proxy(compact(c))

The **ratio** turns proxy counts into the provider's tokens. The
**framing allowance** is the intercept less the receipt's own content,
`intercept − ratio × proxy(RECEIPT)`: the name, id and wrapper of a call
and of its result, one fixed allowance per call (§2.8, "Framing"). Its
result's wrapper is input, not output; counting it with the call is the
one place the method leans pessimistic, by a few tokens per call. The fit
reports its spread: r², the residuals' standard deviation and largest
size, and the ratio and intercept with each call left out in turn.

**With an endpoint,** no fit is needed: a call's output is its own pair's
difference less `RECEIPT`'s content, exactly, and the framing allowance
reported is the median of d(c) − text(compact(c)) − text(RECEIPT) over the
calibration set (§2.8: "a probe message with one block call, minus the
same message without it, minus the arguments' own count").
"""

import json
import math
import random
from dataclasses import dataclass, field
from pathlib import Path

SYSTEM = "You are a token-count probe. Reply with one word."
USER = "Probe."
SAID = "Working."
NEXT = "Continue."
RECEIPT = "ok 1 ops (blocks/1 #0000000000000000)"
PROBE_ID = "toolu_b5probe0001"
CALIBRATION_MIN = 200
EVAL = Path(__file__).resolve().parent.parent
CALIBRATION = EVAL / "calibration.jsonl"


def compact(v):
    """The compact JSON of a call's input, as the model sent it (its keys in
    its order)."""
    return json.dumps(v, separators=(",", ":"), ensure_ascii=False)


def pair(call_input, tools, system=SYSTEM):
    """`(with, without)` for one call (module docs), in the neutral form
    providers.py encodes: Anthropic's Messages shape, with no model."""
    head = [{"role": "user", "content": [{"type": "text", "text": USER}]}]
    with_ = {"system": system, "tools": tools, "messages": head + [
        {"role": "assistant", "content": [{"type": "text", "text": SAID},
                                          {"type": "tool_use", "id": PROBE_ID, "name": "cards_block", "input": call_input}]},
        {"role": "user", "content": [{"type": "tool_result", "tool_use_id": PROBE_ID, "content": RECEIPT},
                                     {"type": "text", "text": NEXT}]},
    ]}
    without = {"system": system, "tools": tools, "messages": head + [
        {"role": "assistant", "content": [{"type": "text", "text": SAID}]},
        {"role": "user", "content": [{"type": "text", "text": NEXT}]},
    ]}
    return with_, without


def text_probe(text, tools, system=SYSTEM):
    """`(with, without)` for a text's own count: the user's message with and
    without it, on a new line."""
    def body(t):
        return {"system": system, "tools": tools, "messages": [{"role": "user", "content": [{"type": "text", "text": t}]}]}
    return body(USER + "\n" + text), body(USER)


@dataclass
class Fit:
    proxy: str
    n: int
    ratio: float
    intercept: float
    framing: float
    r2: float
    resid_sd: float
    resid_max: float
    loo_ratio: tuple
    loo_intercept: tuple
    points: list = field(default_factory=list, repr=False)

    def summary(self):
        return {k: (round(v, 4) if isinstance(v, float) else v) for k, v in self.__dict__.items() if k != "points"} | {
            "loo_ratio": [round(x, 4) for x in self.loo_ratio], "loo_intercept": [round(x, 2) for x in self.loo_intercept]}


def ols(points):
    """Least squares of y on x for `points` [(x, y)]: (intercept, ratio)."""
    n = len(points)
    mx = sum(x for x, _ in points) / n
    my = sum(y for _, y in points) / n
    sxx = sum((x - mx) ** 2 for x, _ in points)
    if sxx == 0:
        raise ValueError("every call has the same proxy count: the set can't calibrate a ratio")
    ratio = sum((x - mx) * (y - my) for x, y in points) / sxx
    return my - ratio * mx, ratio


def fit(points, proxy_name, receipt_proxy):
    """The calibration's fit (module docs). `points` is [(proxy(compact(c)),
    d(c))] over the calibration set; `receipt_proxy` is proxy(RECEIPT)."""
    if len(points) < CALIBRATION_MIN:
        raise ValueError(f"{len(points)} probe pairs: the calibration takes at least {CALIBRATION_MIN}")
    a, b = ols(points)
    resid = [y - (a + b * x) for x, y in points]
    my = sum(y for _, y in points) / len(points)
    sst = sum((y - my) ** 2 for _, y in points)
    sse = sum(r * r for r in resid)
    loo = [ols(points[:i] + points[i + 1:]) for i in range(len(points))]
    return Fit(proxy=proxy_name, n=len(points), ratio=b, intercept=a, framing=a - b * receipt_proxy,
               r2=1 - sse / sst if sst else 1.0, resid_sd=math.sqrt(sse / max(len(points) - 2, 1)),
               resid_max=max(abs(r) for r in resid), loo_ratio=(min(x[1] for x in loo), max(x[1] for x in loo)),
               loo_intercept=(min(x[0] for x in loo), max(x[0] for x in loo)), points=points)


def calibrate(calls, tools, count, proxy, proxy_name):
    """Run the calibration against one provider: `count(body)` is that
    provider's count of a request (its endpoint, or a one-token
    completion's reported input). The fit, and the pairs as counted."""
    _, without = pair({"ops": []}, tools)
    base = count(without)
    points, pairs = [], []
    for c in calls:
        w, _ = pair(c, tools)
        d = count(w) - base
        points.append((proxy(compact(c)), d))
        pairs.append({"bytes": len(compact(c).encode()), "proxy": points[-1][0], "diff": d})
    return fit(points, proxy_name, proxy(RECEIPT)), {"without": base, "pairs": pairs}


def load(path=CALIBRATION):
    """The fixed calibration set: each line `{"input": …, "receipt": …}`."""
    rows = [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]
    return [r["input"] for r in rows], rows


# ── The set's generation (once; the set is committed, then frozen) ─────────


def candidates(fixtures, seed=5, n=320):
    """Candidate calls for the calibration set, from B1's corpus: each
    accepted op alone, then compositions of 1–16 of them whose bytes are
    spread log-uniformly up to 8 KiB, each op's ids made distinct so no two
    ops of a call overlap. `cards-eval receipts` keeps the valid ones."""
    ops = []
    for name in ("valid", "limits"):
        for case in json.loads((Path(fixtures) / "blocks" / f"{name}.json").read_text())["cases"]:
            if "error" not in case and len(compact(case["input"]).encode()) <= 8192:
                ops.extend(case["input"]["ops"])
    rng = random.Random(seed)
    out = [{"ops": [o]} for o in ops]
    while len(out) < n:
        target = math.exp(rng.uniform(math.log(40), math.log(8000)))
        call, size = [], 0
        while size < target and len(call) < 16:
            o = json.loads(json.dumps(rng.choice(ops)))
            if "id" in o and not o["id"].startswith("card:"):
                o["id"] = f"{o['id'][:20]}-{len(call)}"
            call.append(o)
            size = len(compact({"ops": call}).encode())
        out.append({"ops": call})
    seen, unique = set(), []
    for c in out:
        k = compact(c)
        if k not in seen:
            seen.add(k)
            unique.append(c)
    return unique
