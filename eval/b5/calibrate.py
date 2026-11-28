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
**pair's allowance** is the intercept less the receipt's own content,
`intercept − ratio × proxy(RECEIPT)`: the name, id and wrapper of a call
and of its result, one fixed allowance per call (§2.8, "Framing"; how it
splits between output and input is the output control's, below). The fit
reports its spread: r², the residuals' standard deviation and largest
size, and the ratio and intercept with each call left out in turn.

**With an endpoint** (`endpoint`), no fit is needed. The arguments are
counted by the endpoint, as text (`text_probe`), and the pair's
allowance is calibrated once: the median over the calibration set of
d(c) − text(compact(c)) − text(RECEIPT) (§2.8's `d − arguments −
receipt`: the receipt is the result input's, never the allowance's).
Every call is then its arguments' count plus one framing, the
same for every call (count.py), never its own pair's: the method's,
frozen for the run, and the raw differences it came from are kept.

**The pair's allowance holds two wrappers**, the call's and its result's:
a call can't be sent without its result (the pair must be a valid
conversation), so no input probe tells them apart. §2.8 (as ratified in
review #14 round 2, R2-4) puts them on different sides: the call's
framing is block-call output, the result's wrapper is input overhead,
reported on its own line and never attributed to block-call output. So
the call's framing is measured where it is paid, on output:

**The output control** (`output_control`): `OUTPUT_CALLS` calls of the
calibration set, spread over its sizes up to `OUTPUT_MAX_BYTES` (a call's
framing doesn't grow with its length, and a long one costs output), each
sent as a request that makes the model call `cards_block` once with
exactly that input and nothing else (`forced`: the provider's forced
tool choice, Anthropic's `tool_choice` `tool` and an OpenAI-compatible
one's `function`, parallel calls off). Asking isn't evidence (review #14
round 3, R3-1): a response is a **sample** only if it is one call and
nothing else (`alone`): a reported output count; stopped by the call,
not cut off at the cap or for another reason; exactly one call, to
`cards_block`, its arguments JSON; and no text, thinking, reasoning,
refusal or other choice. Any other response is **excluded**, its
reasons kept with it, and never folded into the framing: its output
holds more than one call's wrapper. A sample's reported output, less the
counter's count of the arguments the model actually generated (as the
run counts every call: their compact JSON), is that call's **framing on
output**; the control's is the samples' median. A call whose input isn't
the one asked for is still a sample, counted by the arguments it has
(`exact` says how many matched): it is one call, and its arguments are
counted as written. The run's counter then counts a call as its
arguments plus that framing, and **the result's framing** is the pair's
allowance less it: what the call and its result cost as input beyond the
call's own output. Each request is paid (output tokens), with an
endpoint too, a sample or not: it goes to the budget's ledger as a
probe. The control qualifies if no request was excluded, its framing is
within the framing bounds, its framings' interquartile range within
`endpoint_framing_iqr`, and the result's framing within
−`endpoint_framing_iqr` to the framing bound (a call's input framing can
sit a token under its output's, never far). Without the control a
counter attributes the pair's whole allowance to output (the
conservative reading), and can't flip the default.

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

import collections
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
FORCE_SYSTEM = "You are a probe of tool-call framing. Call the tool exactly as asked, and nothing else."
FORCE = "Call cards_block once, with exactly this input:\n"
OUTPUT_CALLS = 16
OUTPUT_MAX_BYTES = 2048
BASELINES = 3  # the shared `without`, counted first, halfway and last
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


def proxy_requests(n_calls):
    """The requests a proxy calibration of `n_calls` makes (module docs):
    one `with` a call, and the shared baseline `BASELINES` times."""
    return n_calls + BASELINES


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


def forced(call_input, tools):
    """The output control's request for one call (module docs): the model
    made to call `cards_block` with `call_input`, and nothing else. Neutral
    form, Anthropic's `tool_choice` (providers.py converts it)."""
    return {"system": FORCE_SYSTEM, "tools": tools, "tool_choice": {"type": "tool", "name": "cards_block", "disable_parallel_tool_use": True},
            "messages": [{"role": "user", "content": [{"type": "text", "text": FORCE + compact(call_input)}]}]}


def output_calls(calls, n=OUTPUT_CALLS):
    """The output control's calls (module docs): `n` of the calibration
    set's up to `OUTPUT_MAX_BYTES`, evenly spread over their sizes."""
    small = sorted((c for c in calls if len(compact(c).encode()) <= OUTPUT_MAX_BYTES), key=lambda c: len(compact(c).encode()))
    if len(small) <= n:
        return small
    return [small[round(i * (len(small) - 1) / (n - 1))] for i in range(n)]


def alone(r, name="cards_block"):
    """Why a forced call's response (providers.py's shape) isn't one call
    to `name` and nothing else (module docs, "The output control"): empty
    when it is a sample."""
    why, o = [], r.get("output")
    if not isinstance(o, (int, float)) or isinstance(o, bool) or not 0 < o < math.inf:
        why.append("no output count")
    if not r.get("by_call"):
        why.append(f"stopped by {r.get('stop')!r}, not by the call")
    calls = r.get("calls") or []
    if not calls:
        why.append("no call")
    elif len(calls) > 1:
        why.append(f"{len(calls)} calls")
    if any(c.get("name") != name for c in calls):
        why.append("a call to another tool")
    if any(c.get("input") is None for c in calls):
        why.append("arguments that aren't JSON")
    if r.get("text"):
        why.append("text")
    why += sorted(set(r.get("other") or []))
    return why


def output_control(calls, tools, force, text):
    """The call's framing on output (module docs). `force(body)` sends a
    forced call and gives its response's shape (providers.py's module
    docs); `text(s)` is the run's counter's count of a text. A dict: the
    samples' median `framing`, its spread, the requests excluded and why,
    and every request as it went."""
    rows = []
    for c in output_calls(calls):
        r = force(forced(c, tools))
        why = alone(r)
        got = None if why else r["calls"][0]["input"]
        args = None if why else text(compact(got))
        rows.append({"bytes": len(compact(c).encode()), "output": r.get("output"), "n_calls": len(r.get("calls") or []),
                     "stop": r.get("stop"), "args": args, "framing": None if why else r["output"] - args,
                     "exact": not why and got == c, "excluded": why})
    fr = sorted(r["framing"] for r in rows if not r["excluded"])
    by = collections.Counter(w for r in rows for w in r["excluded"])
    return {"framing": statistics.median(fr) if fr else None, "requests": len(rows), "samples": len(fr),
            "framing_quartiles": (statistics.quantiles(fr, n=4) if len(fr) >= 2 else fr * 3) or None,
            "framing_range": [fr[0], fr[-1]] if fr else None, "excluded": sum(bool(r["excluded"]) for r in rows),
            "excluded_by": dict(sorted(by.items())), "exact": sum(r["exact"] for r in rows), "calls": rows}


def output_problems(out, allowance):
    """Why an output control can't back a counter that flips the default
    (module docs, "The output control"): empty when it can."""
    q = QUALIFY
    if not out:
        return ["no output control: the call's framing on output wasn't measured"]
    found = []
    if out["excluded"]:
        by = "; ".join(f"{w}: {n}" for w, n in out["excluded_by"].items())
        found.append(f"{out['excluded']} of the output control's {out['requests']} requests: not one call and nothing else ({by}), "
                     "excluded, never framing")
    if out["framing"] is None:
        return found + ["no output control sample: the call's framing on output wasn't measured"]
    f, iqr = out["framing"], q["endpoint_framing_iqr"]
    if not q["framing"][0] <= f <= q["framing"][1]:
        found.append(f"the call's output framing {f:.2f} outside {q['framing'][0]:g}–{q['framing'][1]:g} tokens")
    lo, _, hi = out["framing_quartiles"]
    if hi - lo > iqr:
        found.append(f"the output control's framings span an interquartile {lo:g}–{hi:g}, over {iqr:g} tokens")
    r = allowance - f
    if not -iqr <= r <= q["framing"][1]:
        found.append(f"the result's framing {r:.2f} (the pair's allowance {allowance:.2f} less the call's {f:.2f}) "
                     f"outside {-iqr:g}–{q['framing'][1]:g} tokens")
    return found


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
