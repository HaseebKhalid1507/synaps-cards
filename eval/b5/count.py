"""The attributed numbers (docs/blocks.md §2.8): what blocks cost inside the
provider's responses, by a stated method. Never billing, and always
labelled with the counter that made them (`Counter.kind`):

- `endpoint`: the provider's token-count endpoint. A call's output is its
  own probe pair's difference less the receipt's content (calibrate.py),
  exactly; a text's count is the user message with it less without it.
- `calibrated`: the proxy tokenizer, fitted to the provider's reported
  usage over the calibration set: a call is `framing + ratio ×
  proxy(compact(input))`, a text `ratio × proxy(text)`.
- `uncalibrated`: the proxy with ratio 1 and §2.7's ~8-token framing, for a
  provider that can't be probed. Its runs can't flip the default (B6).
- `scripted`: the dry run's, against the scripted model. Never data.

Per turn (`attribute`):
- **Block-call output**: the sum over the turn's `cards_block` calls, each
  counted from the compact JSON of its input as sent. **Per round**: a
  round that carries a call, its calls' sum (one call per message, by the
  description; §2.7's ~455 over 7 calls is the gate's 65).
- **Result input**: each `cards_block` result's text, counted, times the
  rounds after it in the turn (each re-read it).
- **Ops on dismissed blocks** (from the fold report, `cards-eval fold`): the
  agent's ops whose block was dismissed when they landed, by count and by
  each op's own compact JSON, counted (no framing: that is the call's).
"""

import json
import statistics

from .calibrate import RECEIPT, compact, pair, text_probe
from .turns import BLOCK_TOOL

OP_DIDS = ("put", "patch", "replace", "collapse", "run", "merge", "move", "remove")
UNCALIBRATED_FRAMING = 8  # §2.7: "7 calls × ~8 (name, id, wrapper)"


class Counter:
    kind = "?"
    can_flip_default = False

    def call(self, call_input):
        raise NotImplementedError

    def text(self, text):
        raise NotImplementedError

    def describe(self):
        return {"kind": self.kind, "can_flip_default": self.can_flip_default}


class Calibrated(Counter):
    kind = "calibrated"
    can_flip_default = True

    def __init__(self, fit, proxy):
        self.fit, self.proxy = fit, proxy

    def call(self, call_input):
        return self.fit.framing + self.fit.ratio * self.proxy(compact(call_input))

    def text(self, text):
        return self.fit.ratio * self.proxy(text)

    def describe(self):
        return super().describe() | {"fit": self.fit.summary()}


class Scripted(Calibrated):
    """The dry run's: calibrated, by the same procedure, against the
    scripted model. It proves the method runs end to end; it is never
    data, and can't flip the default."""

    kind = "scripted"
    can_flip_default = False


class Uncalibrated(Counter):
    kind = "uncalibrated"

    def __init__(self, proxy, proxy_name):
        self.proxy, self.proxy_name = proxy, proxy_name

    def call(self, call_input):
        return UNCALIBRATED_FRAMING + self.proxy(compact(call_input))

    def text(self, text):
        return self.proxy(text)

    def describe(self):
        return super().describe() | {"proxy": self.proxy_name, "framing": UNCALIBRATED_FRAMING}


class Endpoint(Counter):
    """`count(body)` is the provider's token-count endpoint over a neutral
    request (calibrate.py); `tools` are the definitions every probe
    carries. Counts are kept by text, so a call counted once costs nothing
    the second time."""

    kind = "endpoint"
    can_flip_default = True

    def __init__(self, count, tools):
        self.count, self.tools, self.memo = count, tools, {}
        self.receipt = None

    def _diff(self, key, bodies):
        if key not in self.memo:
            w, wo = bodies()
            self.memo[key] = self.count(w) - self.count(wo)
        return self.memo[key]

    def text(self, text):
        return self._diff(("text", text), lambda: text_probe(text, self.tools))

    def call(self, call_input):
        if self.receipt is None:
            self.receipt = self.text(RECEIPT)
        return self._diff(("call", compact(call_input)), lambda: pair(call_input, self.tools)) - self.receipt

    def framing(self, calls):
        """§2.8's framing allowance, measured: the median over `calls` of a
        call's output less its arguments' own count."""
        return statistics.median(self.call(c) - self.text(compact(c)) for c in calls)


def attribute(turn, counter, fold_lines=(), block_tool=BLOCK_TOOL):
    """The attributed numbers of one turn (module docs)."""
    per_call = {c.tool_id: counter.call(c.input) for c in turn.block_calls()}
    per_round = [sum(per_call[c.tool_id] for c in r.block_calls()) for r in turn.rounds if r.block_calls()]
    result_input, rounds_after = 0.0, 0
    for item in reversed(turn.timeline):
        if item[0] == "round":
            rounds_after += 1
        elif item[0] == "result" and item[1].name == block_tool:
            result_input += counter.text(item[1].text) * rounds_after
    inputs = {c.tool_id: c.input for c in turn.block_calls()}
    unseen = [ln for ln in fold_lines if ln.get("tool_id") in inputs and ln.get("author") == "agent"
              and ln.get("did") in OP_DIDS and ln.get("dismissed_before")]
    unseen_tokens = 0.0
    for ln in unseen:
        ops = (inputs[ln["tool_id"]] or {}).get("ops") or []
        if isinstance(ln.get("op"), int) and ln["op"] < len(ops):
            unseen_tokens += counter.text(compact(ops[ln["op"]]))
    return {
        "block_calls": len(per_call),
        "block_output": sum(per_call.values()),
        "block_output_per_round": per_round,
        "result_input": result_input,
        "unseen_ops": len(unseen),
        "unseen_tokens": unseen_tokens,
        "counter": counter.kind,
    }


def definition(count, prefix, block_tool=BLOCK_TOOL):
    """The definition (§2.8): a fixed-prefix control, never the on − off
    workload delta. `prefix` is the on-run's first request as the engine
    sent it, cut to its system prompt, tools and first message (neutral
    form); the control is that prefix counted with `cards_block` declared
    and without it. `count` is the provider's count of a request (its
    endpoint, or a one-token completion's reported input)."""
    on = {"system": prefix.get("system"), "tools": prefix.get("tools") or [], "messages": prefix["messages"][:1]}
    off = dict(on, tools=[t for t in on["tools"] if tool_name(t) != block_tool])
    if len(off["tools"]) == len(on["tools"]):
        raise ValueError(f"the prefix declares no {block_tool}: it isn't an on-run's")
    return count(on) - count(off)


def tool_name(t):
    return t.get("name") or (t.get("function") or {}).get("name")


def prefix_diff(on_first, off_first, block_tool=BLOCK_TOOL):
    """What else differs between the on-run's and the off-run's first
    requests, besides the `cards_block` declaration: empty when the control
    is the whole difference. Reported beside the definition."""
    def norm(req):
        tools = [t for t in (req.get("tools") or []) if tool_name(t) != block_tool]
        return {"system": req.get("system"), "tools": tools, "first": (req.get("messages") or [None])[0]}
    a, b = norm(on_first), norm(off_first)
    return [k for k in a if json.dumps(a[k], sort_keys=True) != json.dumps(b[k], sort_keys=True)]
