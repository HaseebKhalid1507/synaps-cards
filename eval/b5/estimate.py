"""The spend estimate of a full run (run.py --estimate): every runnable
task of the frozen set, both arms, three repeats, on each candidate model,
priced at list (spend.py). No provider call: the set's own shape and the
assumptions below, each stated so the budget can be argued with.

**Assumptions** (per turn of each category; `Shape` in spend.py):
- `PREFIX`: the engine's system prompt and tool definitions. The dry run's
  first request (SynapsCLI 0.12.0, the isolated home) carries 29 tools and
  a 574-byte system prompt: 7,861 tokens by bytes / 4 with blocks on, 6,896
  off (the 965 between them is the definition the control measured). The
  estimate takes 10,000, for a provider's larger system prompt and a
  denser tokenizer. It is cached after a session's first request, so it is
  cache-read every round.
- coding: 10 rounds (the set asks ≥ 6), 300 output tokens and 900 tokens of
  tool results per round; short: 2 rounds; no tool: 1 round; a card task:
  a 2-round turn that shows the card, then a coding turn.
- blocks on: 400 output tokens of block calls per turn (between §2.8's
  tuning target and its gate), 30 tokens of results per call over 6 calls,
  one closing round per turn of more than 2 rounds, and the definition's
  ~1,000 input tokens (§2.8's cap) on every request.
- `HIGH`: the same with twice the rounds and twice the output per round,
  for a model that thinks aloud or wanders: the budget to approve.
- The calibration: with an endpoint (Anthropic), free. Without one, 288
  pairs of one-token completions, ~2,500 input tokens each, uncached.
"""

from . import spend
from .spend import Shape

PREFIX = 10_000  # measured 7,861 (bytes / 4) on the dry run's first request: module docs
SHAPES = {
    "coding": Shape(rounds=10, turns=1, out=300, tool_in=900, user_in=150),
    "short": Shape(rounds=2, turns=1, out=150, tool_in=600, user_in=40),
    "no_tool": Shape(rounds=1, turns=1, out=250, tool_in=0, user_in=60),
    "interactive": Shape(rounds=6, turns=2, out=300, tool_in=900, user_in=120),
}
HIGH = {k: Shape(rounds=s.rounds * 2, turns=s.turns, out=s.out * 2, tool_in=s.tool_in, user_in=s.user_in) for k, s in SHAPES.items()}
BLOCK = {"out": 400, "in": 180, "closing": 1, "definition": 1000}
CALIBRATION_PAIRS = 288
CALIBRATION_IN = 2500


def table(tasks, prefix=PREFIX, repeats=3):
    rows = {}
    for label, shapes in (("expected", SHAPES), ("high", HIGH)):
        est = spend.estimate(tasks, prefix, shapes, BLOCK, repeats)
        both = {k: est["on"].get(k, 0) + est["off"].get(k, 0) for k in est["on"]}
        rows[label] = {"tasks": est["tasks"], "turns": est["turns"], "runs": est["runs"], "tokens": both,
                       "usd": {m: round(spend.cost(m, both), 2) for m in spend.CANDIDATES}}
    calib = {m: round(spend.cost(m, {"input_tokens": CALIBRATION_PAIRS * 2 * CALIBRATION_IN, "output_tokens": CALIBRATION_PAIRS * 2}), 2)
             for m in spend.CANDIDATES}
    e, h = rows["expected"], rows["high"]
    md = [f"A full run: {e['tasks']} runnable tasks × 2 arms × {repeats} repeats = {e['runs']} runs, {e['turns']} turns "
          f"(the {sum(bool(t['needs']) for t in tasks)} hand-back tasks need D8, not run). Prefix {prefix:,} tokens.",
          "",
          "| Model | Expected | High (the budget) | Output tokens (exp.) | Cache-read tokens (exp.) |",
          "|---|---|---|---|---|"]
    for m in spend.CANDIDATES:
        md.append(f"| `{m}` | ${e['usd'][m]:,.2f} | ${h['usd'][m]:,.2f} | {e['tokens']['output_tokens']:,} | "
                  f"{e['tokens']['cache_read_input_tokens']:,} |")
    md += ["", "Counting is free on every candidate: each is an Anthropic model, counted with the token-count endpoint. "
           "(The same set through an OpenAI-compatible gateway, with no endpoint, would add the calibration's "
           f"{CALIBRATION_PAIRS} probe pairs: `calibration_usd` in `--estimate --json`.)"]
    return {"rows": rows, "calibration_usd": calib, "prefix": prefix, "markdown": "\n".join(md)}
