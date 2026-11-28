"""Spend: list prices, the live budget guard, and the estimate of a full run.

**Prices** are SynapsCLI's own table (`0a71ea0d`, `crates/agent-core/src/
pricing.rs`: `listed_prices` 118-151, `cache_read_multiplier` 161-173,
`calculate_cost_split` 214-236; verified there 2026-11-12 against
<https://platform.claude.com/docs/en/about-claude/pricing>). USD per million
tokens; cache reads at the model's multiplier of input, 5-minute cache
writes at 1.25× (the engine's default TTL), 1-hour at 2×. They are list
prices of the direct API: a run on a subscription (OAuth) is billed by the
subscription, not by these.

**The guard.** A real run takes a budget (`--budget-usd`). Every paid
request is charged as its usage comes, and written to the run's ledger
(`paid.jsonl`, one line each) before any more work starts: the daemon's
responses, and the paid probes of a provider with no token-count endpoint
(the calibration's one-token completions, the definition control's two).
The run stops starting new runs once what it spent plus the largest run so
far would pass the budget; a probe, or a turn's response, that takes the
total past it stops the run (`OverBudget`; a turn is cancelled first).

**A soft stop, not a hard cap:** a charge is known only once its response
is done, so the response that crosses the budget is already paid, and so is
whatever the engine had in flight when the turn was cancelled. That is
charged too: the stopped run drains its session to the `idle`, and every
usage the session records is charged once, whatever read it (harness.py's
"The budget"), so the ledger has every response the run saw. The budget
bounds when the run stops spending, not to the cent what it spent. The dry
run prices nothing.
"""

import json
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


class OverBudget(Exception):
    pass


class Budget:
    def __init__(self, usd, model, ledger=None):
        if base(model) is None:
            raise KeyError(f"no list price for {model!r}: a real run needs one to guard its budget")
        self.usd, self.model, self.spent, self.largest = usd, model, 0.0, 0.0
        self._run = 0.0
        self.ledger = ledger  # a path: every charge, a JSON line, written as it comes

    def charge(self, usage, what="run"):
        """Price one paid response's usage (the daemon's `session: usage`
        keys), write it to the ledger, and say whether the total is still
        within the budget."""
        c = cost(self.model, usage)
        self.spent += c
        self._run += c
        if self.ledger is not None:
            keep = {k: usage.get(k, 0) for k in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")}
            with open(self.ledger, "a") as f:
                f.write(json.dumps({"what": what, "usage": keep, "usd": round(c, 6), "spent": round(self.spent, 6)}) + "\n")
        return self.spent <= self.usd

    def paid(self, what):
        """The hook a paid probe calls with its usage: charged, and the run
        stopped (`OverBudget`) once the total passes the budget."""
        def hook(usage):
            if not self.charge(usage, what):
                raise OverBudget(f"spent {self.spent:.2f} of {self.usd:.2f} USD ({what})")
        return hook

    def run_done(self):
        self.largest = max(self.largest, self._run)
        self._run = 0.0

    def room_for_another(self):
        return self.spent + self.largest <= self.usd


# ── The estimate ───────────────────────────────────────────────────────────


@dataclass
class Shape:
    """One kind of turn, as the estimate assumes it."""
    rounds: int  # provider responses per turn
    out: int  # output tokens per round (text, thinking, calls)
    tool_in: int  # tokens each round's tool results add to the context
    user_in: int  # tokens of the user's message


def session_usage(prefix, turns, block_out=0, block_in=0, closing=0, warm=True):
    """One run's reported usage: its `turns` (Shapes) in one session, each
    starting from the history the last one left (the context carries over),
    with prompt caching as the engine sets it: each request reads from the
    cache what the last one sent, and writes what's new. `prefix` is the
    system prompt and the tool definitions. `warm`: they are in the cache
    before the session's first request (an earlier run's, within the cache's
    5 minutes); cold, the first request writes them. With blocks, every turn
    adds `block_out` output tokens and `block_in` of results, spread over its
    rounds, and `closing` rounds to a turn of more than 2."""
    ctx, cached = prefix, prefix if warm else 0
    read = write = out = requests = 0
    for s in turns:
        ctx += s.user_in
        n = s.rounds + (closing if s.rounds > 2 else 0)
        b_out, b_in = (block_out // max(n, 1), block_in // max(n, 1)) if block_out or block_in else (0, 0)
        for _ in range(n):
            read += cached
            write += ctx - cached
            out += s.out + b_out
            requests += 1
            cached = ctx
            ctx += s.out + b_out + s.tool_in + b_in
    return {"input_tokens": UNCACHED_PER_REQUEST * requests, "output_tokens": out, "cache_read_input_tokens": read,
            "cache_creation_input_tokens": write}


def turn_kinds(task, steers_said_instead=False):
    """The turns one run of `task` takes, by kind (estimate.py's shapes): a
    coding task's one turn, and one more per steer said instead (it missed
    the turn and started the next); a card task's turn that shows the card,
    then its answer's coding turn; a hand-back's proposal, then its coding
    turn."""
    starts = sum(1 for s in task["turns"] if {"say", "answer", "hand_back"} & set(s))
    steers = sum(1 for s in task["turns"] if "steer" in s) if steers_said_instead else 0
    if task["category"] == "interactive":
        return ["card"] + ["coding"] * (starts - 1)
    return [task["category"]] * starts + ["steer"] * steers


def estimate(tasks, prefix, shapes, block, repeats=3, needs_ok=(), warm=True, steers_said_instead=False):
    """Usage of a full run: every task the run can do, both arms, `repeats`
    times. `block` = what blocks add per turn (out, in, closing rounds) and
    the definition on every request. A dict of totals per arm."""
    tot = {"on": {}, "off": {}, "tasks": 0, "turns": 0, "runs": 0}
    for t in tasks:
        if any(n not in needs_ok for n in t["needs"]):
            continue
        turns = [shapes[k] for k in turn_kinds(t, steers_said_instead)]
        tot["tasks"] += 1
        for arm in ("on", "off"):
            if arm == "on":
                u = session_usage(prefix + block["definition"], turns, block["out"], block["in"], block["closing"], warm)
            else:
                u = session_usage(prefix, turns, warm=warm)
            for k, v in u.items():
                tot[arm][k] = tot[arm].get(k, 0) + v * repeats
            tot["turns"] += len(turns) * repeats
            tot["runs"] += repeats
    return tot
