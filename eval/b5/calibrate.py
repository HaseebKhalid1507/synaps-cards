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
one (free); otherwise each request is sent as a completion of one output
token, and its reported input (uncached + cache read + cache write) is the
count. Both requests go uncached or both cached: the probes set no cache
breakpoint. Every call's `with` is counted against one shared `without`
(the baseline), counted three times, first, halfway and last: a count
that moved, or a second model answering, means the provider wasn't
stable, and the counter can't flip the default. So the proxy's
calibration is 288 + 3 requests, not 288 independent pairs.

**The calibration** (no endpoint): for each call of the fixed calibration
set (`eval/calibration.jsonl`, ≥ 200 valid calls of varied length), the
pair's difference d(c) is fitted by least squares to

    d(c) = intercept + ratio × proxy(compact(c))

The **ratio** turns proxy counts into the provider's tokens. The
**framing allowance** is the intercept less the receipt's own content,
`intercept − ratio × proxy(RECEIPT)`: the name, id and wrapper of a call
and of its result, one fixed allowance per call (§2.8, "Framing"). The fit
reports its spread: r², the residuals' standard deviation and largest
size, and the ratio and intercept with each call left out in turn.

**With an endpoint** (`endpoint`), no fit is needed. The arguments are
counted by the endpoint, as text (`text_probe`), and the framing
allowance is calibrated once: the median over the calibration set of
d(c) − text(compact(c)) − text(RECEIPT) (§2.8: "a probe message with one
block call, minus the same message without it, minus the arguments' own
count"). Every call is then its arguments' count plus that one allowance
(count.py), never its own pair: the allowance is the method's, frozen for
the run, and the raw differences it came from are kept.

**The result's wrapper is in the allowance** with either counter: a call
can't be sent without its result (the pair must be a valid conversation),
so no probe separates the two wrappers, and §2.8's own probe ("a probe
message with one block call") includes it. It is input, not output: a few
tokens a call on the pessimistic side. eval/README.md and the PR say this
is a reading of §2.8 for Haseeb to ratify, or to change by a probe that
can tell them apart.

**Qualifying** (`problems`, `endpoint_problems`): a counter can flip the
default only if its calibration is sound by these bounds, fixed before any
data (`QUALIFY`): a fit's r² at least 0.95, its ratio within 0.5–2 and
its framing within 0–64 tokens, the ratio left-one-out within 2% of it; an
endpoint's allowance within 0–64 and the calls' own framings within an
interquartile range of 4 tokens; and the baseline steady and one model
answering. A counter that misses one says which, and the report keeps
its run from flipping the default.

**The artifact** (`config`, and each calibration's `raw`): the provider,
its model and count URL, the probe's fixed texts, the digests of the
tools every probe carries and of the calibration set, the number of
requests made, the baselines, and every pair's raw difference.
"""

import hashlib
import json
import math
import random
import statistics
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
QUALIFY = {"r2_min": 0.95, "ratio": (0.5, 2.0), "framing": (0.0, 64.0), "loo_ratio_spread": 0.02, "endpoint_framing_iqr": 4.0}


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
    baselines: tuple = ()  # the shared `without`, counted first, halfway and last
    models: tuple = ()  # the models that answered the probes, where the provider says

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


def problems(f):
    """Why a proxy fit can't flip the default: empty when it can (module
    docs, "Qualifying")."""
    q, out = QUALIFY, []
    if f.r2 < q["r2_min"]:
        out.append(f"r² {f.r2:.4f} < {q['r2_min']}")
    if not q["ratio"][0] <= f.ratio <= q["ratio"][1]:
        out.append(f"ratio {f.ratio:.4f} outside {q['ratio'][0]}–{q['ratio'][1]}")
    if not q["framing"][0] <= f.framing <= q["framing"][1]:
        out.append(f"framing {f.framing:.2f} outside {q['framing'][0]:g}–{q['framing'][1]:g} tokens")
    if f.loo_ratio and (f.loo_ratio[1] - f.loo_ratio[0]) > q["loo_ratio_spread"] * abs(f.ratio):
        out.append(f"the ratio left-one-out spans {f.loo_ratio[0]:.4f}–{f.loo_ratio[1]:.4f}, over {q['loo_ratio_spread']:.0%} of it")
    return out + steady(f.baselines, f.models)


def steady(baselines, models):
    out = []
    if len(set(baselines)) > 1:
        out.append(f"the baseline moved: {list(baselines)}")
    if len(set(models)) > 1:
        out.append(f"more than one model answered: {sorted(set(models))}")
    return out


def calibrate(calls, tools, count, proxy, proxy_name):
    """Run the calibration against one provider: `count(body)` is that
    provider's count of a request (its endpoint, or a one-token
    completion's reported input). The fit, and the pairs as counted."""
    _, without = pair({"ops": []}, tools)
    bases = [count(without)]
    points, pairs = [], []
    for i, c in enumerate(calls):
        if i == len(calls) // 2:
            bases.append(count(without))
        w, _ = pair(c, tools)
        d = count(w) - bases[0]
        points.append((proxy(compact(c)), d))
        pairs.append({"bytes": len(compact(c).encode()), "proxy": points[-1][0], "diff": d})
    bases.append(count(without))
    f = fit(points, proxy_name, proxy(RECEIPT))
    f.baselines = tuple(bases)
    return f, {"without": bases[0], "baselines": bases, "requests": len(calls) + len(bases), "pairs": pairs}


def endpoint(calls, tools, count):
    """The endpoint's framing allowance, calibrated once (module docs):
    `(allowance, raw)`, `raw` holding every call's difference, its
    arguments' count and its own framing."""
    _, without = pair({"ops": []}, tools)
    bases = [count(without)]
    text_base = count(text_probe("", tools)[1])

    def text(s):
        return count(text_probe(s, tools)[0]) - text_base

    receipt = text(RECEIPT)
    rows = []
    for i, c in enumerate(calls):
        if i == len(calls) // 2:
            bases.append(count(without))
        d = count(pair(c, tools)[0]) - bases[0]
        args = text(compact(c))
        rows.append({"bytes": len(compact(c).encode()), "diff": d, "args": args, "framing": d - args - receipt})
    bases.append(count(without))
    fr = sorted(r["framing"] for r in rows)
    q = statistics.quantiles(fr, n=4) if len(fr) >= 2 else [fr[0]] * 3
    raw = {"receipt": receipt, "text_base": text_base, "baselines": bases, "requests": 2 * len(calls) + len(bases) + 2,
           "framing_quartiles": q, "framing_range": [fr[0], fr[-1]], "pairs": rows}
    return statistics.median(fr), raw


def endpoint_problems(allowance, raw, models=()):
    """Why an endpoint's calibration can't flip the default (module docs)."""
    q, out = QUALIFY, []
    if not q["framing"][0] <= allowance <= q["framing"][1]:
        out.append(f"framing {allowance:.2f} outside {q['framing'][0]:g}–{q['framing'][1]:g} tokens")
    lo, _, hi = raw["framing_quartiles"]
    if hi - lo > q["endpoint_framing_iqr"]:
        out.append(f"the calls' framings span an interquartile {lo:g}–{hi:g}, over {q['endpoint_framing_iqr']:g} tokens")
    return out + steady(raw["baselines"], models)


def config(provider, tools, calls_path=CALIBRATION):
    """What a calibration ran against (module docs, "The artifact"): no
    secret, only what B5b needs to say it ran the same."""
    def sha(b):
        return hashlib.sha256(b).hexdigest()
    return {"provider": getattr(provider, "name", "?"), "model": getattr(provider, "model", None),
            "count_url": getattr(provider, "count_url", None),
            "probe": {"system": SYSTEM, "user": USER, "said": SAID, "next": NEXT, "receipt": RECEIPT, "id": PROBE_ID},
            "tools_sha256": sha(json.dumps(tools, sort_keys=True, ensure_ascii=False).encode()),
            "calibration_set_sha256": sha(Path(calls_path).read_bytes()), "calibration_set": Path(calls_path).name}


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
