#!/usr/bin/env python3
"""B5a's mutation pass: break the method on purpose, one exact edit at a
time, and check that a named test of eval's suite catches it.

Each mutation switches off one rule of the method as eval/b5 states it, in
a copy of this checkout's `eval/` (the checkout is never touched), then
runs `python3 -m unittest discover -s tests` there. A mutation counts as
caught only if a named test failed on it; one whose anchor isn't found
exactly once is a bad anchor. The pass is good only with every mutation
caught: a survivor is a rule no test holds.

    python3 eval/mutate.py [--jobs N] [--out FILE] [--anchors] [FILTER]

`FILTER` runs only the mutations whose name contains it; `--anchors` only
checks that every anchor is found once (fast, no test runs). The results
of the pass at a commit are in the PR that commits it.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

EVAL = Path(__file__).resolve().parent
C, T, N, K, R = "b5/classify.py", "b5/turns.py", "b5/count.py", "b5/calibrate.py", "b5/report.py"
H, S, D, P, X = "b5/harness.py", "b5/spend.py", "b5/desk.py", "b5/providers.py", "b5/taskset.py"

MUTATIONS = [
    # The turn and the round (turns.py)
    ("turn: queued_auto (the user's missed steer) isn't user-initiated", T, 'USER_TRIGGERS = ("user", "queued_auto")', 'USER_TRIGGERS = ("user",)'),
    ("turn: event_auto counts as user-initiated", T, 'USER_TRIGGERS = ("user", "queued_auto")', 'USER_TRIGGERS = ("user", "queued_auto", "event_auto")'),
    ("round: a reset keeps what the round drew", T, '                    r.calls, r.text, r.resets = [], "", r.resets + 1', '                    r.resets = r.resets + 1'),
    ("round: counted by response_start, a usage with none is no round", T,
     "r = next((r for r in reversed(turn.rounds) if r.usage is None), None) or _new_round(turn, t)",
     "r = next((r for r in reversed(turn.rounds) if r.usage is None), None) or Round(index=-1)"),
    ("call: streamed arguments ignored (only tool_use)", T, '            elif llm == "tool_use_delta":\n                call = calls.get(s.get("tool_id"))',
     '            elif False:\n                call = calls.get(s.get("tool_id"))'),
    ("call: tool_use after a start adds a second call", T, "                call = calls.get(tid)\n                if call is None:",
     "                call = None\n                if call is None:"),
    ("steer: a delivered steer isn't in the timeline", T, '            turn.timeline.append(("user", s.get("message", ""), True))', "            pass"),
    # The classifier (classify.py)
    ("classifier: k rounds close, not k - 1", C, "closing_rounds=max(len(rs) - 1, 0),", "closing_rounds=len(rs),"),
    ("classifier: the closing input includes R1", C, "closing_input=sum(r.input_side for r in later),", "closing_input=sum(r.input_side for r in rs),"),
    ("classifier: the closing input is the uncached part only", C, "closing_input=sum(r.input_side for r in later),",
     'closing_input=sum((r.usage or {}).get("input_tokens", 0) for r in later),'),
    ("classifier: a block result anchors too", C, 'if item[0] == "user" or (item[0] == "result" and item[1].name != block_tool):',
     'if item[0] == "user" or item[0] == "result":'),
    ("classifier: the user's message doesn't anchor", C, 'if item[0] == "user" or (item[0] == "result" and item[1].name != block_tool):',
     'if item[0] == "result" and item[1].name != block_tool:'),
    ("classifier: only the opening message anchors (a steer doesn't)", C, 'if item[0] == "user" or (item[0] == "result" and item[1].name != block_tool):',
     'if (item[0] == "user" and not item[2]) or (item[0] == "result" and item[1].name != block_tool):'),
    ("classifier: a card is block-like, not the turn's work", C, 'if item[0] == "user" or (item[0] == "result" and item[1].name != block_tool):',
     'if item[0] == "user" or (item[0] == "result" and item[1].name not in (block_tool, "cards_ui")):'),
    ("classifier: the first anchor, not the last", C, "            at = i\n", "            at = i if at < 0 else at\n"),
    ("classifier: an unmeasured turn counts as complete", C, "complete=bool(rs) and turn.measured,", "complete=bool(rs),"),
    # Counting (count.py)
    ("count: no framing on a calibrated call", N, "return self.fit.framing + self.fit.ratio * self.proxy(compact(call_input))",
     "return self.fit.ratio * self.proxy(compact(call_input))"),
    ("count: the proxy uncalibrated (ratio ignored)", N, "return self.fit.framing + self.fit.ratio * self.proxy(compact(call_input))",
     "return self.fit.framing + self.proxy(compact(call_input))"),
    ("count: a call counted from its pretty JSON, not compact", N, "return self.fit.framing + self.fit.ratio * self.proxy(compact(call_input))",
     "return self.fit.framing + self.fit.ratio * self.proxy(__import__('json').dumps(call_input, indent=1))"),
    ("count: the uncalibrated framing dropped", N, "return UNCALIBRATED_FRAMING + self.proxy(compact(call_input))", "return self.proxy(compact(call_input))"),
    ("count: an uncalibrated count can flip the default", N, 'class Uncalibrated(Counter):\n    kind = "uncalibrated"\n',
     'class Uncalibrated(Counter):\n    kind = "uncalibrated"\n    can_flip_default = True\n'),
    ("count: a result read once, not by every later round", N, "result_input += counter.text(item[1].text) * rounds_after", "result_input += counter.text(item[1].text)"),
    ("count: a result also counts the round that made it", N, "result_input, rounds_after = 0.0, 0", "result_input, rounds_after = 0.0, 1"),
    ("count: a result is read at least once, even with no round after it", N, "result_input += counter.text(item[1].text) * rounds_after",
     "result_input += counter.text(item[1].text) * max(rounds_after, 1)"),
    ("count: per round sums every round, callless too", N, "per_round = [sum(per_call[c.tool_id] for c in r.block_calls()) for r in turn.rounds if r.block_calls()]",
     "per_round = [sum(per_call[c.tool_id] for c in r.block_calls()) for r in turn.rounds]"),
    ("definition: counted over the whole first request, not its first message", N, '"messages": prefix["messages"][:1]}', '"messages": prefix["messages"]}'),
    ("definition: a prefix with no cards_block isn't refused", N, "    if len(off[\"tools\"]) == len(on[\"tools\"]):\n        raise", "    if False:\n        raise"),
    # The calibration (calibrate.py)
    ("calibration: fewer than 200 pairs accepted", K, "    if len(points) < CALIBRATION_MIN:\n        raise", "    if len(points) < 2:\n        raise"),
    ("calibration: the framing is the whole intercept (the receipt left in)", K, "framing=a - b * receipt_proxy,", "framing=a,"),
    ("calibration: the leave-one-out spread not computed", K, "loo = [ols(points[:i] + points[i + 1:]) for i in range(len(points))]", "loo = [(a, b)]"),
    ("probe: the pair differs by more than the call (the result left out)", K, '"tool_use_id": PROBE_ID', '"tool_use_id": PROBE_ID + "x"'),
    # The gate (report.py)
    ("gate: callless turns pull the per-turn median down", R, 'with_calls = [r["attributed"]["block_output"] for r in on if r["attributed"] and r["attributed"]["block_calls"]]',
     'with_calls = [r["attributed"]["block_output"] for r in on if r["attributed"]]'),
    ("gate: closing share over 50%", R, 'share <= GATE["closing_rounds"]["share"]', "share <= 0.5"),
    ("gate: two closing rounds in a turn allowed", R, "max(closing) <= 1 and share", "max(closing) <= 2 and share"),
    ("gate: outcome compares pooled pass counts the other way", R, "        if off_p > on_p:", "        if on_p > off_p:"),
    # The set's shape (report.py)
    ("shape: read with blocks on", R, '        if r["arm"] == "off":\n            rows_of_run', '        if r["arm"] == "on":\n            rows_of_run'),
    ("shape: coding at 5 rounds", R, "        return rounds >= 6", "        return rounds >= 5"),
    ("shape: short of 3 rounds", R, "        return 1 <= rounds <= 2", "        return 1 <= rounds <= 3"),
    ("shape: a task's longest turn, not its turns' sum", R, '    rounds = sum(r["rounds"] for r in rows)', '    rounds = max((r["rounds"] for r in rows), default=0)'),
    ("shape: a no-tool run with a tool call", R, '        return bool(rows) and all(r["tool_calls"] == 0 for r in rows)', "        return bool(rows)"),
    ("shape: a no-tool run with no turn", R, '        return bool(rows) and all(r["tool_calls"] == 0 for r in rows)', '        return all(r["tool_calls"] == 0 for r in rows)'),
    ("shape: a skipped answer counts", R, '        return any(s.get("step") in ("answer", "hand_back") and not s.get("skipped") for s in steps)',
     '        return any(s.get("step") in ("answer", "hand_back") for s in steps)'),
    ("shape: a hand-back doesn't count", R, '        return any(s.get("step") in ("answer", "hand_back") and not s.get("skipped") for s in steps)',
     '        return any(s.get("step") == "answer" and not s.get("skipped") for s in steps)'),
    ("shape: one category is enough", R, '        holds = holds and len(held) >= need', '        holds = holds or len(held) >= need'),
    # Round 2 (review #14), and the rules above that it moved
    ("count: the endpoint's call has no framing allowance", N, "return self.text(compact(call_input)) + self.framing", "return self.text(compact(call_input))"),
    ("count: unseen ops ignore when the dismissal landed", N, 'unseen = [o for o in ops(fold_lines, set(inputs)) if o["dismissed_before"]]',
     'unseen = [o for o in ops(fold_lines, set(inputs))]'),
    ("count: another turn's ops count as this turn's unseen patches", N,
     '        if ln.get("tool_id") not in tool_ids or ln.get("author") != "agent" or ln.get("did") not in OP_DIDS:',
     '        if ln.get("author") != "agent" or ln.get("did") not in OP_DIDS:'),
    ("ops: a path diagnostic is an op of its own (P1 2)", N, '            o["paths"].append(ln.get("why") or "?")\n            continue',
     '            o["paths"].append(ln.get("why") or "?")'),
    ("ops: one log line, one op (P1 2)", N, '        o = by.setdefault((ln["tool_id"], ln.get("event"), ln.get("op")), {',
     '        o = by.setdefault((ln["tool_id"], ln.get("event"), ln.get("op"), len(by)), {'),
    ("ops: an op partly skipped counts as skipped (P1 2)", N, 'whole = bool(outs) and all(x == "skipped" for x in outs)',
     'whole = bool(outs) and any(x == "skipped" for x in outs)'),
    ("calibration: a pair's difference against itself (no base request)", K, "        d = count(w) - bases[0]\n", "        d = count(w)\n"),
    ("calibration: a fit of low r² qualifies (P2 6)", K, '    if f.r2 < q["r2_min"]:', '    if False:'),
    ("gate: one repeat is enough", R, '"pass": pooled[k]["pass"] and len(held) >= 2}', '"pass": pooled[k]["pass"] and len(held) >= 1}'),
    ("gate: pooled alone", R, '"pass": pooled[k]["pass"] and len(held) >= 2}', '"pass": pooled[k]["pass"]}'),
    ("shape: one off-run of 3 is enough", R, "held = sorted(t for t, oks in runs.items() if sum(oks) >= 2)",
     "held = sorted(t for t, oks in runs.items() if sum(oks) >= 1)"),
    ("shape: every off-run must play it", R, "held = sorted(t for t, oks in runs.items() if sum(oks) >= 2)",
     "held = sorted(t for t, oks in runs.items() if sum(oks) >= 3)"),
    ("eligibility: the shape ignored (P1 1)", R, '    if not shape["holds"]:\n        out.append', '    if False:\n        out.append'),
    ("eligibility: a dry run is data (P1 1)", R, '    if meta.get("mode") != "real":', '    if False:'),
    ("eligibility: any counter can flip the default (P1 1)", R, '    if not c.get("can_flip_default"):', '    if False:'),
    ("eligibility: fewer repeats are enough (P1 1)", R, '    if meta.get("repeats") != len(REPEATS):', '    if False:'),
    ("eligibility: missing runs are a smaller experiment (P1 1)", R, "        if missing:\n", "        if False:\n"),
    ("eligibility: an unmeasured turn is data (P1 1, P2 9)", R, '    if bad:\n        out.append(f"{len(bad)} runs with a turn not measured',
     '    if False:\n        out.append(f"{len(bad)} runs with a turn not measured'),
    ("eligibility: tasks not run don't matter (P1 1, P1 3)", R, "    if not_run:\n", "    if False:\n"),
    ("eligibility: a notice probe that didn't reach the agent passes (P2 11)", R, '    if short:\n        out.append(f"the notice probe',
     '    if False:\n        out.append(f"the notice probe'),
    ("verdict: rows that pass are a pass, eligible or not (P1 1)", R, 'verdict = "not eligible" if why_not else ("passes" if rows_pass else "fails")',
     'verdict = "passes" if rows_pass else "fails"'),
    ("turn: a session's done ends the turn, no idle needed (P2 9)", T, "                turn.saw_done = True\n",
     '                turn.saw_done = True\n                turn.outcome, turn.t_end = "done", t\n'),
    ("turn: a response with no usage is measured (P2 9)", T, 'return self.outcome == "done" and self.t_end is not None and not self.unmeasured',
     'return self.outcome == "done" and self.t_end is not None'),
    ("net output: per turn, not per planned turn of a run (P2 5)", R,
     'diffs[task] = (statistics.mean(v[0] / v[2] for v in per["on"].values())\n                           - statistics.mean(v[0] / v[2] for v in per["off"].values()))',
     'diffs[task] = (statistics.mean(v[0] / v[1] for v in per["on"].values())\n                           - statistics.mean(v[0] / v[1] for v in per["off"].values()))'),
    ("budget: a stopped run's record is lost (P2 10)", "run.py", "                    keep(e.record)\n", "                    pass\n"),
    ("budget: a paid probe past the budget doesn't stop the run (P2 10)", S,
     '            if not self.charge(usage, what):\n                raise OverBudget', '            if not self.charge(usage, what) and False:\n                raise OverBudget'),
    ("dismissal: a pending block is clicked (P2 11)", H,
     'ok = [b for b in board.get("blocks") or [] if not b.get("dismissed") and not b.get("pending") and (kind is None or b.get("kind") == kind)]',
     'ok = [b for b in board.get("blocks") or [] if not b.get("dismissed") and (kind is None or b.get("kind") == kind)]'),
    ("card: an answer may name no label (P2 7)", X, '            elif keys[0] == "answer" and not (isinstance(s["answer"].get("pick"), list)',
     '            elif keys[0] == "answer" and False and not (isinstance(s["answer"].get("pick"), list)'),
    ("judge: numbers in any order and amount (P2 8)", X, 'if "numbers" in e and numbers(text) != [float(x) for x in e["numbers"]]:',
     'if "numbers" in e and not set(float(x) for x in e["numbers"]) <= set(numbers(text)):'),
    ("judge: items matched loosely, extra items allowed (P2 8)", X,
     "            ok = len(got) == len(pats) and all(sum(bool(p.fullmatch(g)) for g in got) == 1 for p in pats) \\\n",
     "            ok = all(any(p.search(g) for g in got) for p in pats) or \\\n"),
    ("hand-back: off, the opening asks for a block too (P1 3)", H,
     'starter["say_off"] if self.arm == "off" and "say_off" in starter else starter["say"]', 'starter["say"]'),
    ("hand-back: off, the desk is asked for a block (P1 3)", H, '            if self.arm == "on":\n                block = live_block(',
     '            if True:\n                block = live_block('),
    ("hand-back: the oldest block of the kind, not the latest (P1 3)", H, 'hb["block"]["kind"], last=True)', 'hb["block"]["kind"], last=False)'),
    ("long tasks: no spares (P1 4)", X, "        if have < n + SPARES.get(cat, 0):", "        if have < n:"),
    ("long tasks: a one-line repair is a long task (P1 4)", X, "            if written < LONG_WORK_LINES:", "            if False:"),
    ("long tasks: one under 6 rounds isn't reported apart (P1 4)", R,
     'out[cat]["apart"] = [{"task": t, "rounds": [rounds.get((t, rp)) for rp in REPEATS]} for t in out[cat]["short_of"]]',
     'out[cat]["apart"] = []'),
    ("long tasks: one under 6 rounds stays in the long bucket (P1 4)", R,
     '        return ("long" if task in long else "coding_under_6") if c == "coding" else c', '        return "long" if c == "coding" else c'),
    ("estimate: always warm (P3 12)", S, "    ctx, cached = prefix, prefix if warm else 0", "    ctx, cached = prefix, prefix"),
    ("estimate: each turn from the prefix alone (P3 12)", S, "    for s in turns:\n        ctx += s.user_in",
     "    for s in turns:\n        ctx, cached = prefix + s.user_in, prefix"),
    ("estimate: no steer is said instead (P3 12)", S, 'steers = sum(1 for s in task["turns"] if "steer" in s) if steers_said_instead else 0', "steers = 0"),
    ("block-only: a round with another call too is block-only", T, "return bool(self.calls) and all(c.name == BLOCK_TOOL for c in self.calls)",
     "return any(c.name == BLOCK_TOOL for c in self.calls)"),
    ("block-only: not reported", R, '            "block_only_rounds": sum(r.block_only for r in t.rounds),', '            "block_only_rounds": 0,'),
    ("record: the desk's handshake dropped (P3 13)", D, "        self.handshake = r\n", "        pass\n"),
    ("record: latency per request taken from the on-arm only (P3 13)", R,
     '        lat = [round(q["t_done"] - q["t_sent"], 3) for rec in records if rec["arm"] == arm for q in rec.get("requests") or []',
     '        lat = [round(q["t_done"] - q["t_sent"], 3) for rec in records if rec["arm"] == "on" for q in rec.get("requests") or []'),
]


def one(m):
    name, rel, old, new = m
    tmp = Path(tempfile.mkdtemp(prefix="b5mut-"))
    try:
        shutil.copytree(EVAL, tmp / "eval", ignore=shutil.ignore_patterns("__pycache__"))
        f = tmp / "eval" / rel
        text = f.read_text()
        if text.count(old) != 1:
            return "bad", f"- {name}: BAD ANCHOR ({text.count(old)} matches in {rel})"
        f.write_text(text.replace(old, new))
        r = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=tmp / "eval",
                           capture_output=True, text=True, timeout=600, env={"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1"})
        failed = sorted(set(re.findall(r"^(?:FAIL|ERROR): (\S+) \((\S+)\)", r.stderr, re.M)))
        names = sorted({cls for _, cls in failed})  # `module.Class.test`
        if r.returncode != 0 and names:
            return "caught", f"- {name}: caught by {len(names)}: {', '.join(names)}"
        if r.returncode == 0:
            return "survived", f"- {name}: SURVIVED (no test failed)"
        return "bad", f"- {name}: exit {r.returncode} with no FAIL line: {r.stderr[-300:]!r}"
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def anchors(todo):
    bad = [f"- {name}: BAD ANCHOR ({(EVAL / rel).read_text().count(old)} matches in {rel})"
           for name, rel, old, _ in todo if (EVAL / rel).read_text().count(old) != 1]
    return bad


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("filter", nargs="?")
    ap.add_argument("--jobs", type=int, default=int(os.environ.get("B5A_JOBS", "1")))
    ap.add_argument("--out", help="also write the results here")
    ap.add_argument("--anchors", action="store_true", help="only check every anchor is found once")
    a = ap.parse_args(argv)
    names = [m[0] for m in MUTATIONS]
    if len(names) != len(set(names)):
        raise SystemExit("two mutations share a name")
    todo = [m for m in MUTATIONS if not a.filter or a.filter in m[0]]
    if a.anchors:
        bad = anchors(todo)
        print("\n".join(bad + [f"{len(todo) - len(bad)}/{len(todo)} anchors found once"]))
        return 1 if bad else 0
    lines, kinds = [], []
    with ThreadPoolExecutor(max_workers=a.jobs) as ex:
        for kind, line in ex.map(one, todo):
            kinds.append(kind)
            lines.append(line)
            print(line, flush=True)
    caught, survived = kinds.count("caught"), kinds.count("survived")
    summary = f"{caught}/{len(todo)} caught by a named test, {survived} survived, {len(todo) - caught - survived} bad anchors or unclassified"
    print(summary)
    if a.out:
        Path(a.out).write_text("\n".join(lines) + "\n" + summary + "\n")
    return 0 if caught == len(todo) else 1


if __name__ == "__main__":
    sys.exit(main())
