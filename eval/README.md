# B5's harness (docs/blocks.md §2.8)

The method B5 measures blocks by, fixed **before any run**: the frozen task
set, the turn and the round, the closing-round classifier, the counting, the
runs and the report. B5a commits it; B5b reuses it unchanged. Each part's
exact rules are in its module's docs, and its tests pin them.

| Part | Where | Tests |
|---|---|---|
| The frozen task set: 20 coding tasks of 6+ rounds, 10 short, 5 with no tool call, 8 where the user answers a card (5) or hands back an edit (3, "needs D8, not run" until B5b) | `tasks/set.json`, the workspaces, checks and reference solutions; `b5/taskset.py`; its digest in `tasks/set.sha256` | `tests/test_taskset.py`: the shape is §2.8's, the digest holds, every check fails at the start and passes with its reference solution |
| A turn (`TurnStarted` to `Idle`, steers included, user-initiated only) and a round (one provider response: one usage) | `b5/turns.py` | `tests/test_classify.py` |
| The closing-round classifier | `b5/classify.py` | `tests/test_classify.py` |
| Counting: the token-count endpoint where there is one, else the proxy calibrated against reported usage (≥ 200 probe pairs), the framing allowance per call, result input, the definition's fixed-prefix control | `b5/count.py`, `b5/calibrate.py`, `b5/proxy.py`, `calibration.jsonl` (288 valid calls, 34–8,104 bytes of compact JSON) | `tests/test_count.py` |
| The runs: blocks on/off × 3 repeats, interleaved, against an isolated daemon (as `scripts/smoke.py` isolates one) | `b5/harness.py`, `b5/daemon.py`, `run.py` | `tests/test_harness.py`; the dry run |
| The desk seam: D6b's dismiss notices (and D8's hand-backs) come from synaps-desk over `b5-desk/1`, never typed in | `b5/desk.py` | `tests/test_harness.py` (`Seam`) |
| Providers, the tap, spend and the budget guard | `b5/providers.py`, `b5/spend.py`, `b5/estimate.py` | `tests/test_harness.py` |
| The report: the gate's rows (pooled **and** 2 of 3 repeats), the rows reported with no gate, and the set's shape as the run played it (coding ≥ 6 rounds, short 1–2, no tool call, the card answered; blocks off). The gate passes only if the shape holds | `b5/report.py` | `tests/test_harness.py` (`Gate`, `SetShape`) |

## Running it

```sh
# The tests (standard library only; CI runs them).
cd eval && python3 -m unittest discover -s tests

# The dry run: free. The scripted model (b5/scripted.py) plays every runnable
# task in both arms; the stand-in desk sends the notices. Never data.
python3 eval/run.py --dry-run --synaps PATH/TO/synaps --work DIR

# The spend estimate of a full run, per candidate model. No provider call.
python3 eval/run.py --estimate

# A real run spends money: it needs the approval written out, synaps-desk's
# adapter and the credential in the environment (run.py's docs).
ANTHROPIC_API_KEY=… python3 eval/run.py --provider anthropic --model claude-… \
    --budget-usd 60 --approved claude-…@60 --desk 'synaps-desk-b5 …' --synaps PATH --work DIR
```

The run installs this checkout as a user installs the plugin (a clone at
HEAD, then `scripts/setup.sh`) into a throwaway Synaps home with its own
runtime directory and daemon, and builds `cards-eval` (the fold report, the
receipts and card answers, all cards-spec's own) in the same clone. Nothing
of the user's home, plugins or daemons is read or touched.

## What a real run still needs

- **synaps-desk's `b5-desk/1` adapter** (`b5/desk.py` documents the protocol):
  D6b's `Desires` for the dismiss notices, D8 for the hand-backs. A real run
  refuses any desk whose `impl` doesn't name D6b, and the stand-in always.
- **Haseeb's approval** of the model and the budget (`--approved`).
- For a provider with no token-count endpoint: `tiktoken`, for the proxy.

## Changing the set

The set is frozen (§2.8: "Tuning the description never changes them"). A
deliberate change, made before B5b's first run and said in its PR, updates
`tasks/set.sha256`; `tests/test_taskset.py` fails until it does.
