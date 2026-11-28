"""The spend estimate of a full run (run.py --estimate): the frozen set's
whole matrix, every task (the D8 hand-backs included: B6 needs them,
report.py's `required`), both arms, three repeats, on each candidate model,
priced at list (spend.py). No provider call: the set's own shape and the
assumptions below, each stated so the numbers can be argued with. They are
**scenarios, not a cap**: what bounds a real run's spend is its budget
(`--budget-usd`, spend.py's guard), which Haseeb approves. Beside them,
`margin_usd`: the long scenario and the paid probes, plus 15%, the least
budget that leaves the soft stop that much room.

**Assumptions** (per turn of each kind; `Shape` in spend.py):
- `PREFIX`: the engine's system prompt and tool definitions. The dry run's
  first request (SynapsCLI 0.12.0, the isolated home) carries 29 tools and
  a 574-byte system prompt: 7,861 tokens by bytes / 4 with blocks on, 6,896
  off (the 965 between them is the definition the control measured). The
  estimate takes 10,000, for a provider's larger system prompt and a
  denser tokenizer.
- **The cache.** Each request reads from the cache what the last one sent
  and writes what's new, the context carrying over from turn to turn of a
  run (a card's answer turn starts from the card's turn). *Warm*: the
  prefix is already cached when a run starts (the run before it, minutes
  earlier). *Cold*: every run's first request writes it. A run is a fresh
  session in a fresh workspace, so which one holds is the provider's: both
  are shown.
- coding: 10 rounds (the set asks ≥ 6), 300 output tokens and 900 tokens of
  tool results per round; short: 2 rounds; no tool: 1 round; a card task:
  a 2-round turn that shows the card, then a coding turn; a steer said
  instead (it missed the turn): one more turn of 3 rounds.
- blocks on: 400 output tokens of block calls per turn (between §2.8's
  tuning target and its gate), 180 tokens of results per turn, one closing
  round per turn of more than 2 rounds, and the definition's ~1,000 input
  tokens (§2.8's cap) on every request.
- **The depth pilot** (run.py's `--pilot`, `pilot`): every coding task,
  blocks off, once, in the same scenarios; no probes (it has no counter).
  Its budget beside it: 15% over its long scenario, in whole dollars.
- **The scenarios:** *expected* (as above, steers riding their turn), warm
  and cold; *long*: twice the rounds and twice the output per round, every
  steer said instead, cold: a model that thinks aloud or wanders. Long is a
  sensitivity case, not an upper bound: a model can do worse.
- **Paid probes** (`probes`), derived from calibrate.py's own procedure,
  so a change there moves them: the output control's forced calls
  (`calibrate.output_calls`, 16), paid with an endpoint too, each the
  call's tokens (bytes / 4) of input beside the block definition and the
  instruction, and as output plus a call's wrapper; with an endpoint
  (every candidate here: Anthropic's) the counting itself is free.
  Without one, also the proxy's calibration (`calibrate.proxy_requests`:
  one request a call and the shared baseline three times, 291 one-token
  completions, ~2,500 input tokens each, uncached) and the definition
  control's 2. Shown apart, since they are paid once per run of the
  harness, whatever the set.
"""

from . import calibrate, spend
from .calibrate import compact
from .spend import Shape

PREFIX = 10_000  # measured 7,861 (bytes / 4) on the dry run's first request: module docs
SHAPES = {
    "coding": Shape(rounds=10, out=300, tool_in=900, user_in=150),
    "short": Shape(rounds=2, out=150, tool_in=600, user_in=40),
    "no_tool": Shape(rounds=1, out=250, tool_in=0, user_in=60),
    "card": Shape(rounds=2, out=200, tool_in=300, user_in=120),
    "steer": Shape(rounds=3, out=300, tool_in=900, user_in=60),
}
LONG = {k: Shape(rounds=s.rounds * 2, out=s.out * 2, tool_in=s.tool_in, user_in=s.user_in) for k, s in SHAPES.items()}
BLOCK = {"out": 400, "in": 180, "closing": 1, "definition": 1000}
CALIBRATION_IN = 2500  # a proxy probe's input: the block definition, the probe's texts and a call
FORCE_IN = 60  # an output-control request's system prompt and instruction, beside its call
CALL_WRAPPER_OUT = 10  # a forced call's own wrapper, written (§2.7's ~8, rounded up)
MARGIN = 0.15  # the budget's room over the long scenario and the probes
SCENARIOS = {  # name: (shapes, warm, steers said instead)
    "expected_warm": (SHAPES, True, False),
    "expected_cold": (SHAPES, False, False),
    "long_cold": (LONG, False, True),
}
LABELS = {"expected_warm": "Expected, warm prefix", "expected_cold": "Expected, cold prefix",
          "long_cold": "Long scenario (2× rounds and output, steers said instead, cold)"}


def probes(calls, prefix=PREFIX, endpoint=True):
    """The paid probes of one run of the harness (module docs): their
    requests and usage, uncached."""
    forced = [(len(compact(c).encode()) + 3) // 4 for c in calibrate.output_calls(calls)]
    u = {"requests": len(forced), "input_tokens": sum(FORCE_IN + BLOCK["definition"] + k for k in forced),
         "output_tokens": sum(k + CALL_WRAPPER_OUT for k in forced)}
    if not endpoint:
        n = calibrate.proxy_requests(len(calls))
        u = {"requests": u["requests"] + n + 2, "input_tokens": u["input_tokens"] + n * CALIBRATION_IN + 2 * (prefix + BLOCK["definition"]),
             "output_tokens": u["output_tokens"] + n + 2}
    return u


def budget_at_margin(long_usd, probes_usd):
    """The least budget, in whole $5, with `MARGIN` over the long scenario
    and the probes."""
    need = (long_usd + probes_usd) * (1 + MARGIN)
    return 5 * -(-need // 5)


def pilot(tasks, prefix=PREFIX):
    """The depth pilot's spend (module docs): its scenarios per model, and
    its budget at the margin."""
    coding = [t for t in tasks if t["category"] == "coding"]
    out = {}
    for label, (shapes, warm, said) in SCENARIOS.items():
        tot = {}
        for t in coding:
            u = spend.session_usage(prefix, [shapes[k] for k in spend.turn_kinds(t, said)], warm=warm)
            for k, v in u.items():
                tot[k] = tot.get(k, 0) + v
        out[label] = {m: round(spend.cost(m, tot), 2) for m in spend.CANDIDATES}
    return {"tasks": len(coding), "usd": out,
            "budget_usd": {m: -(-out["long_cold"][m] * (1 + MARGIN) // 1) for m in spend.CANDIDATES}}


def table(tasks, prefix=PREFIX, repeats=3, calls=None):
    calls = calibrate.load()[0] if calls is None else calls
    rows = {}
    for label, (shapes, warm, said) in SCENARIOS.items():
        est = spend.estimate(tasks, prefix, shapes, BLOCK, repeats, needs_ok=("D8",), warm=warm, steers_said_instead=said)
        both = {k: est["on"].get(k, 0) + est["off"].get(k, 0) for k in est["on"]}
        rows[label] = {"tasks": est["tasks"], "turns": est["turns"], "runs": est["runs"], "tokens": both,
                       "usd": {m: round(spend.cost(m, both), 2) for m in spend.CANDIDATES}}
    paid = {"endpoint": probes(calls, prefix), "no_endpoint": probes(calls, prefix, endpoint=False)}
    probes_usd = {m: round(spend.cost(m, paid["no_endpoint"]), 2) for m in spend.CANDIDATES}
    control_usd = {m: round(spend.cost(m, paid["endpoint"]), 2) for m in spend.CANDIDATES}
    margin_usd = {m: budget_at_margin(rows["long_cold"]["usd"][m], control_usd[m]) for m in spend.CANDIDATES}
    e = rows["expected_warm"]
    md = [f"A full run, the frozen set's whole matrix: {e['tasks']} tasks (the {sum(bool(t['needs']) for t in tasks)} hand-backs "
          f"included: B6 needs them) × 2 arms × {repeats} repeats = {e['runs']} runs, {e['turns']} turns "
          f"(the long scenario: {rows['long_cold']['turns']}, its steers said instead). Prefix {prefix:,} tokens. "
          "Scenarios, not a cap: the budget bounds a run (estimate.py).",
          "",
          "| Model | " + " | ".join(LABELS.values()) + f" | Paid probes: the output control ({paid['endpoint']['requests']}) "
          f"| Paid probes, with no count endpoint ({paid['no_endpoint']['requests']}) | Long and the control, +{MARGIN:.0%} |",
          "|---|" + "---|" * (len(LABELS) + 3)]
    for m in spend.CANDIDATES:
        md.append(f"| `{m}` | " + " | ".join(f"${rows[k]['usd'][m]:,.2f}" for k in SCENARIOS)
                  + f" | ${control_usd[m]:,.2f} | ${probes_usd[m]:,.2f} | ${margin_usd[m]:,.0f} |")
    md += ["", "Tokens (expected, warm): " + ", ".join(f"{k} {v:,}" for k, v in sorted(e["tokens"].items())) + ".",
           "", "Every candidate is an Anthropic model, counted with the token-count endpoint: free, so only the output "
           "control's forced calls are paid. The column with no count endpoint is what the same run through an "
           "OpenAI-compatible gateway would add: the proxy's calibration, the definition control and the output control."]
    pp = pilot(tasks, prefix)
    md += ["", f"The depth pilot (`--pilot`): the {pp['tasks']} coding tasks, blocks off, once, no probes.", "",
           "| Model | " + " | ".join(LABELS.values()) + f" | Its budget, +{MARGIN:.0%} |", "|---|" + "---|" * (len(LABELS) + 1)]
    for m in spend.CANDIDATES:
        md.append(f"| `{m}` | " + " | ".join(f"${pp['usd'][k][m]:,.2f}" for k in SCENARIOS) + f" | ${pp['budget_usd'][m]:,.0f} |")
    return {"rows": rows, "probes": paid, "probes_usd": probes_usd, "control_usd": control_usd, "margin_usd": margin_usd,
            "pilot": pp, "prefix": prefix, "markdown": "\n".join(md)}
