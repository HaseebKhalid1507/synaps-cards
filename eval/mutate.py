#!/usr/bin/env python3
"""B5a's mutation pass: break the method on purpose, one exact edit at a
time, and check that a named test of eval's suite catches it.

Each mutation switches off one rule of the method as eval/b5 states it, in
a copy of this checkout's `eval/` (the checkout is never touched), then
runs `python3 -m unittest discover -s tests` there. The copy is laid out
as the checkout is (`eval/` and `docs/`: the suite reads §2.8), and the
pass first runs the suite on it unmutated: if that fails, nothing after
it means anything, and the pass stops. A mutation counts as caught only
if a named test failed on it; a test module that fails to import is no
test, so that run is bad, and so is one whose anchor isn't found exactly
once. The pass is good only with every mutation caught: a survivor is a
rule no test holds.

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
E, M, U, J = "b5/estimate.py", "b5/daemon.py", "run.py", "tasks/set.json"
A = "../docs/blocks.md"  # §2.8, the authority: the copy holds docs/ beside eval/

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
    ("count: no framing on a calibrated call", N, "return self.call_framing + self.fit.ratio * self.proxy(compact(call_input))",
     "return self.fit.ratio * self.proxy(compact(call_input))"),
    ("count: the proxy uncalibrated (ratio ignored)", N, "return self.call_framing + self.fit.ratio * self.proxy(compact(call_input))",
     "return self.call_framing + self.proxy(compact(call_input))"),
    ("count: a call counted from its pretty JSON, not compact", N, "return self.call_framing + self.fit.ratio * self.proxy(compact(call_input))",
     "return self.call_framing + self.fit.ratio * self.proxy(__import__('json').dumps(call_input, indent=1))"),
    ("count: the uncalibrated framing dropped", N, "return UNCALIBRATED_FRAMING + self.proxy(compact(call_input))", "return self.proxy(compact(call_input))"),
    ("count: an uncalibrated count can flip the default", N, 'class Uncalibrated(Counter):\n    kind = "uncalibrated"\n',
     'class Uncalibrated(Counter):\n    kind = "uncalibrated"\n    can_flip_default = True\n'),
    ("count: a result read once, not by every later round", N, "result_input += counter.text(item[1].text) * rounds_after", "result_input += counter.text(item[1].text)"),
    ("count: a result also counts the round that made it", N, "result_input, result_framing_input, rounds_after = 0.0, 0.0, 0",
     "result_input, result_framing_input, rounds_after = 0.0, 0.0, 1"),
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
    ("count: the endpoint's call has no framing allowance", N, "return self.text(compact(call_input)) + self.call_framing", "return self.text(compact(call_input))"),
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
    ("eligibility: missing runs are a smaller experiment (P1 1)", R, "    if gaps:\n", "    if False:\n"),
    ("eligibility: an unmeasured turn is data (P1 1, P2 9)", R, '    if bad:\n        out.append(f"{len(bad)} runs with a turn not measured',
     '    if False:\n        out.append(f"{len(bad)} runs with a turn not measured'),
    ("eligibility: D8 tasks not run don't matter (P1 1, P1 3)", R, "    if d8:\n", "    if False:\n"),
    ("eligibility: a notice probe that didn't reach the agent passes (P2 11)", R, '    if short:\n        out.append(f"the notice probe',
     '    if False:\n        out.append(f"the notice probe'),
    ("verdict: rows that pass are a pass, eligible or not (P1 1)", R, 'verdict = "not eligible" if why_not else ("passes" if rows_pass else "fails")',
     'verdict = "passes" if rows_pass else "fails"'),
    ("turn: a session's done ends the turn, no idle needed (P2 9)", T, "                turn.saw_done = True\n",
     '                turn.saw_done = True\n                turn.outcome, turn.t_end = "done", t\n'),
    ("turn: a response with no usage is measured (P2 9)", T,
     'return self.outcome == "done" and self.t_end is not None and bool(self.rounds) and not self.unmeasured',
     'return self.outcome == "done" and self.t_end is not None and bool(self.rounds)'),
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
     'if "numbers" in e and not set(float(x) for x in e["numbers"]) <= set(numbers(text) or []):'),
    ("judge: items matched loosely, extra items allowed (P2 8)", X,
     "            ok = len(got) == len(pats) and all(sum(bool(p.fullmatch(g)) for g in got) == 1 for p in pats) \\\n"
     "                and all(sum(bool(p.fullmatch(g)) for p in pats) == 1 for g in got)\n",
     "            ok = all(any(p.search(g) for g in got or []) for p in pats)\n"),
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
    # Round 2 (review #14, Shady's round 2 on 6f7e8fc)
    ("eligibility: the caller's plan is the matrix (R2-2)", R, "    want = required(tasks)\n",
     "    want = collections.Counter(plan) if plan is not None else required(tasks)\n"),
    ("eligibility: a task left out of records and plan is fine (R2-2)", R, "    if other:\n", "    if False:\n"),
    ("eligibility: a run with no judgment is data (R2-2)", R, "    if unjudged:\n", "    if False:\n"),
    ("outcome row: no judgments pass it (R2-2)", R, '"pass": bool(judged) and not worse}', '"pass": not worse}'),
    ("turn: a turn the provider never answered is measured (R2-2)", T,
     'return self.outcome == "done" and self.t_end is not None and bool(self.rounds) and not self.unmeasured',
     'return self.outcome == "done" and self.t_end is not None and not self.unmeasured'),
    ("interactions: a fallback hand-back is a hand-back (R2-1)", R, '                elif s.get("as") != "block":\n', '                elif False:\n'),
    ("interactions: a skipped answer is an answer (R2-1)", R, '            if s.get("step") == "answer":\n                if s.get("skipped"):\n',
     '            if s.get("step") == "answer":\n                if False:\n'),
    ("interactions: a sent message is a received one (R2-1)", R, '            got = [m for m in msgs if received(history, m.get("text"))]\n',
     '            got = msgs\n'),
    ("eligibility: interaction coverage ignored (R2-1)", R, "    if missed:\n        out.append(", "    if False:\n        out.append("),
    ("notice: delivered on the send record (R2-3)", R, "                if at is not None and carried and all(v < n[2] for v in earlier):\n",
     "                if True:\n"),
    ("notice: any version in the text is the desk's (R2-3)", R,
     '                carried = desk.notices_in(m.get("text")).get((n[0], "dismissed")) == n[2]\n', '                carried = True\n'),
    ("notice: a stale version delivers (R2-3)", R, "if at is not None and carried and all(v < n[2] for v in earlier):",
     "if at is not None and carried:"),
    ("received: a text part isn't the message (R2-3)", D, "        if want in {x.strip() for x in parts} or text_of(m).strip() == want:",
     "        if text_of(m).strip() == want:"),
    ("harness: a lost send isn't settled as lost (R2-3)", H,
     '        self.desk.settle(conv, history, lost=[m["ticket"] for m in lost if m.get("ticket") is not None])', "        self.desk.settle(conv, history)"),
    ("budget: the session charges no usage (R2-6)", M, "                self.on_usage(ev[\"event\"])\n", "                pass\n"),
    ("budget: a stopped run doesn't drain (R2-6)", H, '            rec["drained"] = drain(s)\n', '            rec["drained"] = True\n'),
    ("judge: the numbers found in prose (R2-5)", X, "    hit = NUMBERS.fullmatch(bare(text))", "    hit = NUMBERS.search(bare(text))"),
    ("judge: the form searched for, not whole (R2-5)", X, 'if "form" in e and not re.fullmatch(e["form"], bare(low)',
     'if "form" in e and not re.search(e["form"], bare(low)'),
    ("judge: the turn's narration is the answer (R2-5)", H,
     '    return next((r.text for r in reversed(ts[-1].rounds) if r.text.strip()), "") if ts else ""',
     '    return "".join(r.text for r in ts[-1].rounds) if ts else ""'),
    ("task: shelf-subjects's OR unsaid (R2-7)", J, " with several, a book is kept if it has any one of them (OR, not AND).", ""),
    ("count: the result's framing is block-call output (R2-4)", N,
     '        return out["framing"] if out.get("framing") is not None else self.allowance', "        return self.allowance"),
    ("count: the result's framing not charged as input (R2-4)", N, "            result_framing_input += counter.result_framing * rounds_after",
     "            result_framing_input += 0"),
    ("count: no output control can flip the default (R2-4)", K,
     '        return ["no output control: the call\'s framing on output wasn\'t measured"]', "        return []"),
    ("calibrate: the output control's framing keeps its arguments (R2-4)", K,
     '"framing": None if why else r["output"] - args,', '"framing": None if why else r["output"],'),
    ("calibrate: a control with an excluded request qualifies (R2-4, R3-1)", K, '    if out["excluded"]:\n', "    if False:\n"),
    ("calibrate: a response with a call is a sample, its first call taken (R3-1)", K, "        why = alone(r)\n",
     '        why = [] if r.get("calls") else alone(r)\n'),
    ("calibrate: a second call is fine (R3-1)", K, "    elif len(calls) > 1:\n", "    elif False:\n"),
    ("calibrate: a call to another tool is fine (R3-1)", K, '    if any(c.get("name") != name for c in calls):\n', "    if False:\n"),
    ("calibrate: arguments that aren't JSON are fine (R3-1)", K, '    if any(c.get("input") is None for c in calls):\n', "    if False:\n"),
    ("calibrate: text with the call is fine (R3-1)", K, '    if r.get("text"):\n', "    if False:\n"),
    ("calibrate: thinking or reasoning with the call is fine (R3-1)", K, '    why += sorted(set(r.get("other") or []))\n', "    why += []\n"),
    ("calibrate: a response cut off at the cap is fine (R3-1)", K, '    if not r.get("by_call"):\n', "    if False:\n"),
    ("calibrate: a response with no output count is fine (R3-1)", K,
     "    if not isinstance(o, (int, float)) or isinstance(o, bool) or not 0 < o < math.inf:\n", "    if o is None:\n"),
    ("calibrate: the forced request allows parallel calls (R3-1)", K, '"disable_parallel_tool_use": True},', '"disable_parallel_tool_use": False},'),
    ("providers: a chat request allows parallel calls (R3-1)", P, '            out["parallel_tool_calls"] = False\n', "            pass\n"),
    ("providers: Anthropic's shape keeps only the first call (R3-1)", P,
     '"calls": [{"name": c.get("name"), "input": c.get("input")} for c in content if c.get("type") == "tool_use"],',
     '"calls": [{"name": c.get("name"), "input": c.get("input")} for c in content if c.get("type") == "tool_use"][:1],'),
    ("providers: Anthropic's thinking dropped (R3-1)", P, '"other": [c.get("type") for c in content if c.get("type") not in ("tool_use", "text")],',
     '"other": [],'),
    ("providers: a chat response's text dropped (R3-1)", P, '"text": _text(content) if content else "",', '"text": "",'),
    ("providers: a chat response's reasoning dropped (R3-1)", P, '    other = (["reasoning"] if m.get("reasoning_content")', '    other = ([] if m.get("reasoning_content")'),
    ("providers: a chat response cut off at the cap is by the call (R3-1)", P, 'get("finish_reason") in ("tool_calls", "stop")}',
     'get("finish_reason") in ("tool_calls", "stop", "length")}'),
    ("providers: Anthropic's forced call isn't paid (R2-4)", P,
     "            self.on_usage({k: u.get(k) or 0 for k in", "            (lambda u: None)({k: u.get(k) or 0 for k in"),
    ("estimate: the proxy probes priced as pairs (R2-8)", E, "        n = calibrate.proxy_requests(len(calls))\n", "        n = 2 * len(calls)\n"),
    ("estimate: the D8 tasks left out of the full run (R2-8)", E, 'needs_ok=("D8",), warm=warm', "needs_ok=(), warm=warm"),
    ("report: a record's own mode unread, only the meta's (R3-2)", R,
     '    labels = collections.Counter(r.get("mode") for r in records if r.get("mode") != "real")\n', "    labels = collections.Counter()\n"),
    ("report: a record with no mode is a real run's (R3-2)", R,
     '    labels = collections.Counter(r.get("mode") for r in records if r.get("mode") != "real")\n',
     '    labels = collections.Counter(r.get("mode") for r in records if r.get("mode") not in ("real", None))\n'),
    ("report: a record of another experiment is this run's (R3-2)", R, '        others = sum(r.get("experiment") != exp for r in records)\n',
     "        others = 0\n"),
    ("report: a run naming no experiment is fine (R3-2)", R, "    if not exp:\n", "    if False:\n"),
    ("report: a pilot's configuration is fine (R3-2)", R, '    if (meta.get("config") or {}).get("pilot"):\n', "    if False:\n"),
    ("run: a record isn't stamped (R3-2)", U, "        rec.update(stamp)\n", "        pass\n"),
    ("run: the budget-stopped record isn't stamped (R3-2)", U, "                    keep(e.record)\n",
     '                    out.write(json.dumps(e.record, ensure_ascii=False) + "\\n")\n                    records.append(e.record)\n'),
    ("run: the pilot's records stamped real (R3-2)", U, 'stamp("pilot")', 'stamp("real")'),
    ("run: the pilot's meta isn't its records' experiment (R3-2)", U,
     '    meta = {"mode": st["mode"], "experiment": st["experiment"], "model": a.model,', '    meta = {"mode": st["mode"], "experiment": None, "model": a.model,'),
    ("authority: the endpoint's allowance keeps the receipt (R3-3)", A, "`d − arguments − receipt`", "`d − arguments`"),
    ("authority: the depth pilot not ratified (R3-3)", A, "**The one exception: the depth pilot**", "**The depth pilot**"),
    ("authority: the pilot can't bias the comparison (R3-3)", A, "But choosing tasks by their off-arm depth still shapes the workload, and the freeze says so.",
     "So it can't bias the comparison."),
    ("authority: a response that makes the call is a sample (R3-1)", A, "a response is a sample only if it **is** one call and nothing else",
     "a response is a sample if it makes the call"),
    ("pilot: both arms (pilot)", H, '    return [(0, t, "off") for t in tasks if t["category"] == "coding"]',
     '    return [(0, t, a) for t in tasks if t["category"] == "coding" for a in ("on", "off")]'),
    ("pilot: its report eligible (pilot)", R, '"meta": meta, "mode": "pilot", "eligible_for_b6": False,', '"meta": meta, "mode": "pilot", "eligible_for_b6": True,'),
    ("pilot: a run's approval is a pilot's (pilot)", U, '    if a.approved != f"pilot:{a.model}@{a.budget_usd:g}":',
     '    if a.approved not in (f"pilot:{a.model}@{a.budget_usd:g}", f"{a.model}@{a.budget_usd:g}"):'),
    ("record: latency per request taken from the on-arm only (P3 13)", R,
     '        lat = [round(q["t_done"] - q["t_sent"], 3) for rec in records if rec["arm"] == arm for q in rec.get("requests") or []',
     '        lat = [round(q["t_done"] - q["t_sent"], 3) for rec in records if rec["arm"] == "on" for q in rec.get("requests") or []'),
    ("turns: a one-shot's aggregate usage is a round (R4-1)", T, '            if sess == "usage" and workers:', '            if sess == "usage" and workers is None:'),
    ("turns: a worker's span never closes (R4-1)", T, '            workers.discard(s.get("subagent_id"))', '            pass'),
    ("turns: a usage with no response begun is dropped (R4-1)", T,
     '                r = next((r for r in reversed(turn.rounds) if r.usage is None), None) or _new_round(turn, t)',
     '                r = next((r for r in reversed(turn.rounds) if r.usage is None), None) or Round(index=-1)'),
    ("turns: one delegation tool left out (R4-1)", T, '"subagent_steer", "subagent_collect", "subagent_resume")', '"subagent_steer", "subagent_resume")'),
    ("turns: delegation offered unread (R4-1)", T, "    d = [n for n in offered(requests) if n in DELEGATION_TOOLS]\n", "    d = []\n"),
    ("turns: delegation calls unread (R4-1)", T, "    called = collections.Counter(c.name for c, _ in cs if c.name in DELEGATION_TOOLS)\n",
     "    called = collections.Counter()\n"),
    ("turns: a worker's events unread (R4-1)", T, 's.get("kind") == "agent" and s.get("agent") in WORKER_EVENTS:', 's.get("kind") == "agent" and s.get("agent") in ():'),
    ("turns: subagent_rows unread (R4-1)", T, '        elif ev.get("ev") == "subagent_rows" and ev.get("rows"):', '        elif False:'),
    ("turns: a worker's usage unnamed (R4-1)", T, "    if wu:\n", "    if False:\n"),
    ("turns: a worker's report in a result unread (R4-1)", T, '    return isinstance(v, dict) and "handle_id" in v and "usage" in v', "    return False"),
    ("turns: more paid requests than usages is fine (R4-1)", T, "    if n != usages:\n", "    if n < usages:\n"),
    ("turns: a usage the tap never saw is fine (R4-1)", T, "    if n != usages:\n", "    if n > usages:\n"),
    ("turns: a failed request is paid (R4-1)", T, '            and (q.get("status") is None or 200 <= q["status"] < 300)]', "            ]"),
    ("turns: a request not yet answered isn't paid (R4-1)", T, 'and (q.get("status") is None or 200 <=', 'and (q.get("status") is not None and 200 <='),
    ("turns: requests not recorded read as none (R4-1)", T, "    if requests is None:\n", "    if False:\n"),
    ("turns: a chat function's name unread (R4-1)", T,
     '            names.add((t.get("function") or {}).get("name") if t.get("type") == "function" else t.get("name"))', '            names.add(t.get("name"))'),
    ("turns: Anthropic's tool name unread (R4-1)", T,
     '            names.add((t.get("function") or {}).get("name") if t.get("type") == "function" else t.get("name"))',
     '            names.add((t.get("function") or {}).get("name"))'),
    ("daemon: the daemon boots without disabled_tools (R4-1)", M, "        self.config.write_text(self.config_base)\n", "        self.config.write_text(self.config_was)\n"),
    ("daemon: an arm's config drops disabled_tools (R4-1)", M, "        self.config.write_text(self.config_base + (", "        self.config.write_text(self.config_was + ("),
    ("daemon: a config's own disabled_tools merged (R4-1)", M, '    if any(line.split("=", 1)[0].strip() == "disabled_tools" for line in config.splitlines()):',
     "    if False:"),
    ("daemon: the line lists one tool (R4-1)", M, "f\"disabled_tools = {', '.join(DELEGATION_TOOLS)}\\n\"", "f\"disabled_tools = {DELEGATION_TOOLS[0]}\\n\""),
    ("harness: the record doesn't say what was offered (R4-1)", H, '        rec["offered"] = turns.offered(rec["requests"])', '        rec["offered"] = []'),
    ("harness: the record doesn't say why it isn't single-agent (R4-1)", H,
     '        rec["not_single_agent"] = turns.not_single_agent(rec["events"], rec["requests"])', '        rec["not_single_agent"] = []'),
    ("run: a run offered delegation doesn't stop the experiment (R4-1)", U, "            if delegation:  #", "            if False:  #"),
    ("run: the tap's status dropped (R4-1)", U, '"path": r.get("path"), "status": r.get("status"),', '"path": r.get("path"),'),
    ("report: eligibility doesn't read single-agent (R4-1)", R, "    multi = [(rec, w) for rec in records for w in [single_agent(rec)] if w]\n", "    multi = []\n"),
    ("report: the harness's finding unread (R4-1)", R, '    return list(dict.fromkeys(list(rec.get("not_single_agent") or [])', "    return list(dict.fromkeys(list([])"),
    ("report: the record not read again (R4-1)", R, '                              + turns.not_single_agent(rec.get("events") or [], rec.get("requests"))))',
     "                              ))"),
    ("report: a pilot run that delegated is measured (R4-1)", R, "all(x.measured for x in ts) and not why,", "all(x.measured for x in ts),"),
    ("report: the pilot's summary names none (R4-1)", R, '                    "not_single_agent": sorted(r["task"] for r in rows if r["not_single_agent"]),',
     '                    "not_single_agent": [],'),
    ("report: a worker's usage reported as the rounds' (R4-1)", R, '_merge([r.get("worker_usage") or {} for r in turn_rows', '_merge([r["usage"] for r in turn_rows'),
    ("report: the tools offered, the on-arm's for both (R4-1)", R, 'for rec in records if rec["arm"] == arm for n in turns.offered(', 'for rec in records if rec["arm"] == "on" for n in turns.offered('),
    ("authority: a worker's usage is a round (R4-1)", A, "So a worker's usage is never a round.", "So a worker's usage is a round."),
    ("authority: delegation left on (R4-1)", A, "starts with the engine's delegation tools disabled, so none is offered", "starts as the engine starts"),
    ("providers: reasoning_details unread (R4-2)", P, 'm.get("reasoning") or m.get("reasoning_details")', 'm.get("reasoning")'),
    ("providers: an output count in the details unread (R4-2)", P,
     '    other += [f"output for {k}" for k, v in sorted(details.items()) if k != "reasoning_tokens" and (_count(v) or 0) > 0]\n', "    other += []\n"),
    ("providers: a zero count in the details is output (R4-2)", P, 'k != "reasoning_tokens" and (_count(v) or 0) > 0]', 'k != "reasoning_tokens" and _count(v) is not None]'),
    ("providers: a content part that isn't text unread (R4-2)", P, '                  if not (isinstance(p, dict) and p.get("type") == "text")]', "                  if False]"),
    ("providers: an unread message field taken for nothing (R4-2)", P,
     '    other += [f"an unread part: {k}" for k in sorted(m) if k not in CHAT_READ and m[k] not in (None, "", [], {})]\n', "    other += []\n"),
    ("providers: an empty field is an unread part (R4-2)", P, 'k not in CHAT_READ and m[k] not in (None, "", [], {})]', "k not in CHAT_READ and m[k] is not None]"),
    ("providers: audio read as nothing (R4-2)", P, '"reasoning_details", "refusal")  # chat_shape', '"reasoning_details", "refusal", "audio")  # chat_shape'),
]


def copy():
    """A disposable copy of the checkout's `eval/` and `docs/` (module docs)."""
    tmp = Path(tempfile.mkdtemp(prefix="b5mut-"))
    shutil.copytree(EVAL, tmp / "eval", ignore=shutil.ignore_patterns("__pycache__"))
    if (EVAL.parent / "docs").is_dir():
        shutil.copytree(EVAL.parent / "docs", tmp / "docs")
    return tmp


def suite(tmp):
    return subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=tmp / "eval",
                          capture_output=True, text=True, timeout=600, env={"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1"})


def baseline():
    """The suite on an unmutated copy: `(ok, its last lines)`."""
    tmp = copy()
    try:
        r = suite(tmp)
        return r.returncode == 0, r.stderr.strip().splitlines()[-3:]
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def one(m):
    name, rel, old, new = m
    tmp = copy()
    try:
        f = tmp / "eval" / rel
        text = f.read_text()
        if text.count(old) != 1:
            return "bad", f"- {name}: BAD ANCHOR ({text.count(old)} matches in {rel})"
        f.write_text(text.replace(old, new))
        r = suite(tmp)
        failed = sorted(set(re.findall(r"^(?:FAIL|ERROR): (\S+) \((\S+)\)", r.stderr, re.M)))
        names = sorted({cls for _, cls in failed})  # `module.Class.test`
        if any(n.startswith("unittest.loader.") for n in names):
            return "bad", f"- {name}: a test module failed to import: {', '.join(n for n in names if n.startswith('unittest.loader.'))}"
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
    ok, tail = baseline()
    print(f"baseline, unmutated: {'OK' if ok else 'FAILED'}: {' / '.join(tail)}", flush=True)
    if not ok:
        raise SystemExit("the suite fails on the unmutated copy: no mutation can mean anything")
    lines, kinds = [f"baseline, unmutated: {' / '.join(tail)}"], []
    with ThreadPoolExecutor(max_workers=a.jobs) as ex:
        for kind, line in ex.map(one, todo):
            kinds.append(kind)
            lines.append(line)
            print(line, flush=True)
    caught, survived = kinds.count("caught"), kinds.count("survived")
    summary = f"{caught}/{len(todo)} caught by a named test, {survived} survived, {len(todo) - caught - survived} bad (anchors, imports) or unclassified"
    print(summary)
    if a.out:
        Path(a.out).write_text("\n".join(lines) + "\n" + summary + "\n")
    return 0 if caught == len(todo) else 1


if __name__ == "__main__":
    sys.exit(main())
