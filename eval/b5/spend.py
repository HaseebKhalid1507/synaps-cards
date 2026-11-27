"""Spend: list prices, the live budget guard, and the estimate of a full run.

**Prices** are SynapsCLI's own table (`0a71ea0d`, `crates/agent-core/src/
pricing.rs`: `listed_prices` 118-151, `cache_read_multiplier` 161-173,
`calculate_cost_split` 214-236; verified there 2026-11-12 against
<https://platform.claude.com/docs/en/about-claude/pricing>). USD per million
tokens; cache reads at the model's multiplier of input, 5-minute cache
writes at 1.25× (the engine's default TTL), 1-hour at 2×. They are list
prices of the direct API: a run on a subscription (OAuth) is billed by the
subscription, not by these.

**The guard.** A real run takes a budget (`--budget-usd`). Every usage the
daemon reports is priced as it comes; the run stops starting new runs once
what it spent plus the largest run so far would pass the budget, and
cancels a turn outright if the spent total passes it. The dry run prices
nothing.
"""

from dataclasses import dataclass

# model: (input, output, cache-read multiplier); Haiku 5.5 is tiered by prompt.
PRICES = {
    "claude-fable-5-2": (8.0, 40.0, 0.025),
    "claude-opus-5-5": (4.0, 20.0, 0.05),
    "claude-sonnet-5-5": (2.0, 10.0, 0.05),
    "claude-sonnet-5": (2.0, 10.0, 0.10),
    "claude-haiku-4-5": (1.0, 5.0, 0.10),
}
CANDIDATES = ("claude-fable-5-2", "claude-opus-5-5", "claude-sonnet-5-5", "claude-sonnet-5", "claude-haiku-4-5")
WRITE_5M = 1.25
# The estimate's uncached input per request: what follows the last cache
# breakpoint. Everything new since the last request is priced as a cache
# write (1.25×), the costlier of the two, so the estimate leans high.
UNCACHED_PER_REQUEST = 8


def base(model):
    """`anthropic/claude-sonnet-5` and `claude-sonnet-5-20261101` price as
    `claude-sonnet-5`; None for a model the table doesn't list (a real run
    refuses it: its guard couldn't price it)."""
    m = model.split("/", 1)[-1]
    for k in sorted(PRICES, key=len, reverse=True):
        if m == k or (m.startswith(k + "-") and m[len(k) + 1:len(k) + 9].isdigit()):
            return k
    return None


def cost(model, usage):
    """USD for one response's usage (the daemon's `session: usage`)."""
    k = base(model)
    if k is None:
        raise KeyError(f"no list price for {model!r}")
    inp, out, read = PRICES[k]
    m = 1e-6
    return (usage.get("input_tokens", 0) * inp + usage.get("output_tokens", 0) * out
            + usage.get("cache_read_input_tokens", 0) * inp * read
            + usage.get("cache_creation_input_tokens", 0) * inp * WRITE_5M) * m


class Budget:
    def __init__(self, usd, model):
        if base(model) is None:
            raise KeyError(f"no list price for {model!r}: a real run needs one to guard its budget")
        self.usd, self.model, self.spent, self.largest = usd, model, 0.0, 0.0
        self._run = 0.0

    def charge(self, usage):
        c = cost(self.model, usage)
        self.spent += c
        self._run += c
        return self.spent <= self.usd

    def run_done(self):
        self.largest = max(self.largest, self._run)
        self._run = 0.0

    def room_for_another(self):
        return self.spent + self.largest <= self.usd


# ── The estimate ───────────────────────────────────────────────────────────


@dataclass
class Shape:
    """One category's turn, as the estimate assumes it."""
    rounds: int  # provider responses per turn
    turns: int  # turns per task
    out: int  # output tokens per round (text, thinking, calls)
    tool_in: int  # tokens each round's tool results add to the context
    user_in: int  # tokens of the user's message


def turn_usage(prefix, s, block_out=0, block_in=0, extra_rounds=0):
    """One turn's reported usage, with prompt caching as the engine sets it:
    each request re-reads the context so far from the cache, and writes
    what's new since the last one. `prefix` is the system prompt and the
    tool definitions; `block_out`, `block_in` are what blocks add per round
    (their calls, their results), and `extra_rounds` the closing rounds."""
    n = s.rounds + extra_rounds
    ctx, read, write, out = prefix + s.user_in, 0, 0, 0
    cached = prefix  # the prefix is warm after the first request of a run
    for _ in range(n):
        read += cached
        write += ctx - cached
        out += s.out + block_out
        cached = ctx
        ctx += s.out + block_out + s.tool_in + block_in
    return {"input_tokens": UNCACHED_PER_REQUEST * n, "output_tokens": out, "cache_read_input_tokens": read,
            "cache_creation_input_tokens": write}


def estimate(tasks, prefix, shapes, block, repeats=3, needs_ok=()):
    """Usage of a full run: every task the run can do, both arms, `repeats`
    times; plus the calibration's probes. `block` = (out, in, closing
    rounds) per turn with blocks on, spread over its rounds. A dict of
    totals per arm."""
    tot = {"on": {}, "off": {}, "tasks": 0, "turns": 0, "runs": 0}
    for t in tasks:
        if any(n not in needs_ok for n in t["needs"]):
            continue
        s = shapes[t["category"]]
        tot["tasks"] += 1
        for arm in ("on", "off"):
            if arm == "on":
                per_round_out = block["out"] // max(s.rounds, 1)
                per_round_in = block["in"] // max(s.rounds, 1)
                u = turn_usage(prefix + block["definition"], s, per_round_out, per_round_in,
                               block["closing"] if s.rounds > 2 else 0)
            else:
                u = turn_usage(prefix, s)
            for k, v in u.items():
                tot[arm][k] = tot[arm].get(k, 0) + v * s.turns * repeats
            tot["turns"] += s.turns * repeats
            tot["runs"] += repeats
    return tot
