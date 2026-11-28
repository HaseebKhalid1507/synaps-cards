"""The spend estimate of a full run (run.py --estimate): every runnable
task of the frozen set, both arms, three repeats, on each candidate model,
priced at list (spend.py). No provider call: the set's own shape and the
assumptions below, each stated so the numbers can be argued with. They are
**scenarios, not a cap**: what bounds a real run's spend is its budget
(`--budget-usd`, spend.py's guard), which Haseeb approves.

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
- **The scenarios:** *expected* (as above, steers riding their turn), warm
  and cold; *long*: twice the rounds and twice the output per round, every
  steer said instead, cold: a model that thinks aloud or wanders. Long is a
  sensitivity case, not an upper bound: a model can do worse.
- **Paid probes:** with an endpoint (every candidate here: Anthropic's), the
  counting is free. Without one, the calibration's 288 pairs of one-token
  completions, ~2,500 input tokens each, uncached, and the definition
  control's 2 probes: shown apart, since they are paid once per run of the
  harness, whatever the set.
"""

from . import spend
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
CALIBRATION_PAIRS = 288
CALIBRATION_IN = 2500
SCENARIOS = {  # name: (shapes, warm, steers said instead)
    "expected_warm": (SHAPES, True, False),
    "expected_cold": (SHAPES, False, False),
    "long_cold": (LONG, False, True),
}
LABELS = {"expected_warm": "Expected, warm prefix", "expected_cold": "Expected, cold prefix",
          "long_cold": "Long scenario (2× rounds and output, steers said instead, cold)"}


def table(tasks, prefix=PREFIX, repeats=3):
    rows = {}
    for label, (shapes, warm, said) in SCENARIOS.items():
        est = spend.estimate(tasks, prefix, shapes, BLOCK, repeats, warm=warm, steers_said_instead=said)
        both = {k: est["on"].get(k, 0) + est["off"].get(k, 0) for k in est["on"]}
        rows[label] = {"tasks": est["tasks"], "turns": est["turns"], "runs": est["runs"], "tokens": both,
                       "usd": {m: round(spend.cost(m, both), 2) for m in spend.CANDIDATES}}
    probes = {"input_tokens": CALIBRATION_PAIRS * 2 * CALIBRATION_IN + 2 * (prefix + BLOCK["definition"]),
              "output_tokens": CALIBRATION_PAIRS * 2 + 2}
    probes_usd = {m: round(spend.cost(m, probes), 2) for m in spend.CANDIDATES}
    e = rows["expected_warm"]
    md = [f"A full run: {e['tasks']} runnable tasks × 2 arms × {repeats} repeats = {e['runs']} runs, {e['turns']} turns "
          f"(the long scenario: {rows['long_cold']['turns']}, its steers said instead; "
          f"the {sum(bool(t['needs']) for t in tasks)} hand-back tasks need D8, not run). Prefix {prefix:,} tokens. "
          "Scenarios, not a cap: the budget bounds a run (estimate.py).",
          "",
          "| Model | " + " | ".join(LABELS.values()) + " | Paid probes, with no count endpoint |",
          "|---|" + "---|" * (len(LABELS) + 1)]
    for m in spend.CANDIDATES:
        md.append(f"| `{m}` | " + " | ".join(f"${rows[k]['usd'][m]:,.2f}" for k in SCENARIOS) + f" | ${probes_usd[m]:,.2f} |")
    md += ["", "Tokens (expected, warm): " + ", ".join(f"{k} {v:,}" for k, v in sorted(e["tokens"].items())) + ".",
           "", "Counting is free on every candidate: each is an Anthropic model, counted with the token-count endpoint, so "
           "the probes column is $0 for them. It is what the same run through an OpenAI-compatible gateway, with no "
           "endpoint, would add."]
    return {"rows": rows, "probes_usd": probes_usd, "prefix": prefix, "markdown": "\n".join(md)}
