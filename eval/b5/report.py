"""The report (docs/blocks.md §2.8): run records in, the gate's rows and the
rows reported with no gate out. Every number says its kind: `reported`
(the provider's per-response usage), `attributed` (count.py, by the
counter named), `classified` (classify.py), `counted` (the fold report and
the results) or `judged` (taskset.py).

**What counts as a turn here:** user-initiated turns only (turns.py);
event-started turns are listed apart, outside every denominator.

**A gate row** passes only if it passes on the pooled runs **and** in at
least 2 of the 3 repeats (§2.8). Where §2.8 leaves the population open,
the reading here is the stricter one, and each is stated:
- *block-call output per turn* is over the on-turns that made at least one
  block call (a turn with none adds a zero that would pull the median
  down); how many made none is reported beside it;
- *per round* is over the rounds that carry a block call;
- *closing rounds* are over every complete on-turn;
- *output on − off* is per task: the mean of its on-turns' reported output
  less the mean of its off-turns', then the median over tasks; a repeat's
  is the same over that repeat's runs;
- *repairs and re-puts* are per on-turn; *skipped ops* over every agent op
  the board logged with blocks on;
- *task outcome*: pooled, a task fails the row if it passed more of its
  off-runs than its on-runs; in a repeat, if it passed off and failed on.

**Refusals and repairs:** a `cards_block` result that isn't a receipt
(`ok …`) is a refusal; a repair is a refusal whose next block call in the
same turn was accepted.
"""

import json
import statistics

from . import classify, count, turns

GATE = {
    "block_output_per_turn": {"kind": "attributed", "median": 500, "p90": 900, "target": 400},
    "block_output_per_round": {"kind": "attributed", "median": 65, "target": 50},
    "closing_rounds": {"kind": "classified", "max_per_turn": 1, "share": 0.40, "target": 0.25},
    "definition": {"kind": "fixed-prefix control", "max": 1000, "target": 800},
    "output_on_minus_off": {"kind": "reported", "median": 300, "target": 150},
    "repairs_and_reputs": {"kind": "counted", "per_turn": 0.1},
    "skipped_ops": {"kind": "counted", "share": 0.05, "target": 0.02},
    "task_outcome": {"kind": "judged"},
}


def p90(xs):
    """The 90th percentile, nearest rank (no interpolation: a gate reads an
    observed value)."""
    xs = sorted(xs)
    return xs[max(0, -(-9 * len(xs) // 10) - 1)] if xs else None


def median(xs):
    return statistics.median(xs) if xs else None


def refusals(turn):
    """(refusals, repairs) of one turn (module docs)."""
    results = {}
    for item in turn.timeline:
        if item[0] == "result" and item[1].name == turns.BLOCK_TOOL:
            results[item[1].tool_id] = item[1].text
    calls = [c for r in turn.rounds for c in r.block_calls()]
    refused = [i for i, c in enumerate(calls) if c.tool_id in results and not results[c.tool_id].startswith("ok ")]
    repairs = sum(1 for i in refused if i + 1 < len(calls) and results.get(calls[i + 1].tool_id, "").startswith("ok "))
    return len(refused), repairs


def per_turn(rec, counter):
    """The rows of one run's turns: a list of dicts, and its event-started turns."""
    ts = turns.turns(rec["events"])
    lines = rec["fold"]["lines"]
    rows, event_started = [], []
    for t in ts:
        if not t.user_initiated:
            event_started.append({"trigger": t.trigger, "rounds": len(t.rounds)})
            continue
        c = classify.closing(t)
        a = count.attribute(t, counter, lines) if rec["arm"] == "on" else None
        ref, rep = refusals(t)
        ids = {x.tool_id for x in t.block_calls()}
        mine = [ln for ln in lines if ln.get("tool_id") in ids and ln.get("author") == "agent" and ln.get("did") in count.OP_DIDS]
        rows.append({
            "task": rec["task"], "arm": rec["arm"], "repeat": rec["repeat"], "trigger": t.trigger, "outcome": t.outcome,
            "rounds": len(t.rounds),
            "output": sum(r.output for r in t.rounds),
            "input_side": sum(r.input_side for r in t.rounds),
            "usage": {k: sum((r.usage or {}).get(k, 0) for r in t.rounds)
                      for k in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")},
            "closing": {"k": c.k, "rounds": c.closing_rounds, "input": c.closing_input, "complete": c.complete},
            "attributed": a,
            "refusals": ref, "repairs": rep,
            "ops": len(mine),
            "skipped": sum(ln["outcome"] == "skipped" for ln in mine),
            "skipped_by_reason": _by(mine, lambda ln: ln["outcome"] == "skipped"),
            "skipped_paths_by_reason": _by(mine, lambda ln: ln["outcome"] == "skipped_path"),
            "reputs": sum(ln["outcome"] == "exists" for ln in mine),
            "latency": [round(r.t_end - r.t_start, 3) for r in t.rounds if r.t_end is not None and r.t_start is not None],
        })
    return rows, event_started


def _by(lines, keep):
    out = {}
    for ln in lines:
        if keep(ln):
            out[ln.get("why") or "?"] = out.get(ln.get("why") or "?", 0) + 1
    return out


def rows_of(turn_rows, judged, definition):
    """§2.8's gate rows over one population (pooled, or one repeat)."""
    on = [r for r in turn_rows if r["arm"] == "on"]
    out = {}
    with_calls = [r["attributed"]["block_output"] for r in on if r["attributed"] and r["attributed"]["block_calls"]]
    g = GATE["block_output_per_turn"]
    out["block_output_per_turn"] = {"median": median(with_calls), "p90": p90(with_calls), "turns": len(with_calls),
                                    "turns_without_calls": len(on) - len(with_calls)}
    out["block_output_per_turn"]["pass"] = bool(with_calls) and median(with_calls) <= g["median"] and p90(with_calls) <= g["p90"]
    per_round = [x for r in on if r["attributed"] for x in r["attributed"]["block_output_per_round"]]
    out["block_output_per_round"] = {"median": median(per_round), "rounds": len(per_round),
                                     "pass": bool(per_round) and median(per_round) <= GATE["block_output_per_round"]["median"]}
    done = [r for r in on if r["closing"]["complete"]]
    closing = [r["closing"]["rounds"] for r in done]
    share = (sum(1 for x in closing if x >= 1) / len(closing)) if closing else None
    out["closing_rounds"] = {"max": max(closing, default=None), "share": share, "turns": len(closing),
                             "pass": bool(closing) and max(closing) <= 1 and share <= GATE["closing_rounds"]["share"]}
    out["definition"] = {"tokens": definition, "pass": definition is not None and definition <= GATE["definition"]["max"]}
    diffs = {}
    for task in sorted({r["task"] for r in turn_rows}):
        o = [r["output"] for r in turn_rows if r["task"] == task and r["arm"] == "on"]
        f = [r["output"] for r in turn_rows if r["task"] == task and r["arm"] == "off"]
        if o and f:
            diffs[task] = statistics.mean(o) - statistics.mean(f)
    out["output_on_minus_off"] = {"median": median(list(diffs.values())), "tasks": len(diffs), "per_task": diffs,
                                  "pass": bool(diffs) and median(list(diffs.values())) <= GATE["output_on_minus_off"]["median"]}
    fixes = sum(r["repairs"] + r["reputs"] for r in on)
    out["repairs_and_reputs"] = {"repairs": sum(r["repairs"] for r in on), "reputs": sum(r["reputs"] for r in on),
                                 "turns": len(on), "pass": bool(on) and fixes <= GATE["repairs_and_reputs"]["per_turn"] * len(on)}
    ops = sum(r["ops"] for r in on)
    skipped = sum(r["skipped"] for r in on)
    out["skipped_ops"] = {"skipped": skipped, "ops": ops, "share": skipped / ops if ops else None,
                          "pass": ops > 0 and skipped <= GATE["skipped_ops"]["share"] * ops}
    worse = []
    for task in sorted({j["task"] for j in judged}):
        on_p = sum(j["pass"] for j in judged if j["task"] == task and j["arm"] == "on")
        off_p = sum(j["pass"] for j in judged if j["task"] == task and j["arm"] == "off")
        if off_p > on_p:
            worse.append(task)
    out["task_outcome"] = {"worse_with_blocks": worse, "pass": not worse}
    return out


def build(records, counter, definition, not_run, meta):
    """The whole report (module docs)."""
    turn_rows, event_started, judged = [], [], []
    for rec in records:
        rows, ev = per_turn(rec, counter)
        turn_rows += rows
        event_started += [dict(e, task=rec["task"], arm=rec["arm"], repeat=rec["repeat"]) for e in ev]
        judged.append({"task": rec["task"], "arm": rec["arm"], "repeat": rec["repeat"], "pass": rec["judge"]["pass"]})
    repeats = sorted({r["repeat"] for r in records})
    pooled = rows_of(turn_rows, judged, definition)
    by_repeat = {r: rows_of([x for x in turn_rows if x["repeat"] == r], [j for j in judged if j["repeat"] == r], definition)
                 for r in repeats}
    gate = {}
    for k in GATE:
        held = [r for r in repeats if by_repeat[r][k]["pass"]]
        need = min(2, len(repeats))
        gate[k] = {"kind": GATE[k]["kind"], "pooled": pooled[k], "repeats_passing": held,
                   "pass": pooled[k]["pass"] and len(held) >= need}
    on = [r for r in turn_rows if r["arm"] == "on"]
    also = {
        "closing_input": sum(r["closing"]["input"] for r in on),
        "result_input": sum(r["attributed"]["result_input"] for r in on if r["attributed"]),
        "unseen_ops": sum(r["attributed"]["unseen_ops"] for r in on if r["attributed"]),
        "unseen_tokens": sum(r["attributed"]["unseen_tokens"] for r in on if r["attributed"]),
        "refusals": sum(r["refusals"] for r in on),
        "repairs": sum(r["repairs"] for r in on),
        "reputs": sum(r["reputs"] for r in on),
        "skipped_by_reason": _merge(r["skipped_by_reason"] for r in on),
        "skipped_paths_by_reason": _merge(r["skipped_paths_by_reason"] for r in on),
        "notices_delivered": sum(1 for rec in records for ln in rec["fold"]["lines"] if ln.get("did") == "notice"),
        "latency_per_round": {arm: {"median": median(lat), "p90": p90(lat), "rounds": len(lat)}
                              for arm in ("on", "off") for lat in [[x for r in turn_rows if r["arm"] == arm for x in r["latency"]]]},
        "reported_per_arm": {arm: _merge([r["usage"] for r in turn_rows if r["arm"] == arm]) for arm in ("on", "off")},
    }
    return {
        "meta": meta | {"counter": counter.describe()},
        "turns": len(turn_rows), "event_started": event_started,
        "not_run": [{"task": t["id"], "needs": t["needs"], "why": f"needs {', '.join(t['needs'])}, not run"} for t in not_run],
        "gate": gate, "gate_pass": all(g["pass"] for g in gate.values()),
        "also": also,
        "outcomes": judged,
        "turn_rows": turn_rows,
    }


def _merge(dicts):
    out = {}
    for d in dicts:
        for k, v in d.items():
            out[k] = out.get(k, 0) + v
    return out


def markdown(rep):
    """The report's summary, for the PR and for Haseeb."""
    m = rep["meta"]
    lines = [f"# B5 report: {m.get('mode')} run, {m.get('model')}",
             "",
             f"Head `{m.get('head', '?')[:7]}`, task set `{m.get('taskset', '?')[:12]}`, {rep['turns']} user-initiated turns, counter **{m['counter']['kind']}**"
             + (" (never data)" if m.get("mode") == "dry" else "") + ".",
             ""]
    lines += ["| Row | Kind | Pooled | Repeats passing | Pass |", "|---|---|---|---|---|"]
    for k, g in rep["gate"].items():
        p = {x: y for x, y in g["pooled"].items() if x not in ("pass", "per_task")}
        lines.append(f"| {k} | {g['kind']} | `{json.dumps(p, sort_keys=True)}` | {g['repeats_passing']} | {'yes' if g['pass'] else '**no**'} |")
    lines += ["", f"Also reported: `{json.dumps({k: v for k, v in rep['also'].items() if k != 'reported_per_arm'}, sort_keys=True)}`", ""]
    if rep["not_run"]:
        lines.append("Not run: " + ", ".join(f"`{t['task']}` ({t['why']})" for t in rep["not_run"]) + ".")
    if rep["event_started"]:
        lines.append(f"Event-started turns, outside every denominator: {len(rep['event_started'])}.")
    return "\n".join(lines) + "\n"
