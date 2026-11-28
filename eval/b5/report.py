"""The report (docs/blocks.md §2.8): run records in, the gate's rows and the
rows reported with no gate out. Every number says its kind: `reported`
(the provider's per-response usage), `attributed` (count.py, by the
counter named), `classified` (classify.py), `counted` (the fold report and
the results) or `judged` (taskset.py).

**What counts as a turn here:** user-initiated turns only (turns.py);
event-started turns are listed apart, outside every denominator.

**A gate row** passes only if it passes on the pooled runs **and** in at
least 2 of the 3 repeats (§2.8): always 2 of repeats 1, 2 and 3, a repeat
with no runs failing every row. Where §2.8 leaves the population open,
the reading here is the stricter one, and each is stated:
- *block-call output per turn* is over the on-turns that made at least one
  block call (a turn with none adds a zero that would pull the median
  down); how many made none is reported beside it;
- *per round* is over the rounds that carry a block call;
- *closing rounds* are over every complete on-turn;
- *output on − off* (§2.8: "per task, the difference of the repeats'
  means") is per run first. A run's output per turn is the reported output
  of all its user-initiated turns, summed, over the task's **planned**
  turns (its user's starters: `say`, `answer`, `hand_back`). So a turn
  the engine split (a steer said instead, or one queued past the drain)
  moves output between turns, not out of the task, and can't pass for a
  saving. A task's arm is the mean of its runs, one per repeat, each
  weighed the same; the row is the median over tasks of on − off; a
  repeat's is the same over that repeat's runs. Each task's run totals and
  turn counts are reported beside it, and every repeat whose two arms took
  a different number of turns is listed (`unmatched_turns`);
- *repairs and re-puts* are per on-turn; *skipped ops* over every agent op
  of the on-turns' calls, each op once however many lines the board logged
  for it (count.py's `ops`); a path that didn't apply is a diagnostic in
  its own table, never an op;
- *task outcome*: pooled, a task fails the row if it passed more of its
  off-runs than its on-runs; in a repeat, if it passed off and failed on.

**Refusals and repairs:** a `cards_block` result that isn't a receipt
(`ok …`) is a refusal; a repair is a refusal whose next block call in the
same turn was accepted.

**The set's shape** (§2.8: "≥ 20 coding tasks of ≥ 6 rounds", "≥ 10
short tasks of 1–2 rounds", "≥ 5 turns with no tool call", "≥ 5 tasks in
which the user answers a card or hands back an edit") is how the model
played the set, so it is read from the run, with blocks off (blocks add
rounds of their own). One off-run plays its task's category if:
- *coding*: its user-initiated turns took ≥ 6 rounds in all;
- *short*: 1 or 2;
- *no tool*: it had a turn, and no turn of it made a tool call;
- *interactive*: the user's answer or hand-back was sent, not skipped
  (no card showing, or none to answer).

A task holds its shape if at least 2 of its 3 off-runs (repeats 1–3)
played it; a run that isn't there didn't.
The shape holds if every category has at least §2.8's minimum of tasks
holding theirs (`taskset.CATEGORIES`), and **the gate passes only if the
shape holds**: a run that didn't play §2.8's set didn't measure it. Every
task short of its shape is named.

**Long tasks** (frozen here, before any data): a coding task counts as long
only if its blocks-off runs reached 6 rounds, as above; the set keeps
spares for the ones that don't (taskset.py's `SPARES`). The rest are
reported apart, each with its off-runs' rounds by repeat
(`set_shape.coding.apart`), never in the long count. The gate's rows are
§2.8's, over every turn; `also.by_bucket` gives the same rows over each
bucket (the long tasks, coding under 6 rounds, short, no tool,
interactive), reported with no gate.

**The verdict** keeps what the rows say apart from what they license:
- `rows_pass`: every gate row passes (pooled and 2 of 3). Descriptive.
- *Notice probes*: each task with a dismissal is its probe of the unseen
  patches (§3.5). An on-run delivered it if the desk accepted the click and
  a message the agent then got carried the notice. The desk refusing one is
  a seam failure; a probe delivered in fewer than 2 of its task's 3 on-runs
  wasn't exercised.
- `eligible_for_b6`: the run is the experiment §2.8 prescribes, so its rows
  can flip the default. Every condition is checked and each one missing is
  named (`not_eligible_because`): a real run (a dry run is never data); a
  counter that can flip the default (the endpoint, or a fitted proxy:
  count.py); the frozen task set (its digest); exactly three repeats; the
  plan's runs, every runnable task × both arms × repeats 1–3, each there
  once, none stopped (an interrupted run is incomplete, not a smaller
  experiment, and so is one the budget stopped); every user-initiated turn
  of every run measured (turns.py:
  its `idle`, every response's usage); no task left out (B6 needs the D8
  hand-backs' both arms, which B5a doesn't run: §8.1's B5b); the set's
  shape; no dismissal refused, and every notice probe delivered in 2 of 3.
- `verdict`: `not eligible` unless it is eligible, whatever the rows say;
  then `passes` or `fails`. `gate_pass` is the verdict `passes`.
"""

import collections
import json
import statistics

from . import classify, count, taskset, turns

REPEATS = (1, 2, 3)  # §2.8: "three times each"

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
# What one off-run of each category plays (module docs, "The set's shape").
SHAPE = {
    "coding": "≥ 6 rounds",
    "short": "1–2 rounds",
    "no_tool": "no tool call",
    "interactive": "the user's answer or hand-back sent",
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
        mine = count.ops(lines, {x.tool_id for x in t.block_calls()})
        rows.append({
            "task": rec["task"], "arm": rec["arm"], "repeat": rec["repeat"], "trigger": t.trigger, "outcome": t.outcome,
            "measured": t.measured, "unmeasured_rounds": len(t.unmeasured), "planned_turns": rec.get("planned_turns", 1),
            "rounds": len(t.rounds),
            "tool_calls": sum(len(r.calls) for r in t.rounds),
            "output": sum(r.output for r in t.rounds),
            "input_side": sum(r.input_side for r in t.rounds),
            "usage": {k: sum((r.usage or {}).get(k, 0) for r in t.rounds)
                      for k in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")},
            "closing": {"k": c.k, "rounds": c.closing_rounds, "input": c.closing_input, "complete": c.complete},
            "attributed": a,
            "refusals": ref, "repairs": rep,
            "ops": len(mine),
            "skipped": sum(o["outcome"] == "skipped" for o in mine),
            "partly_skipped": sum(o["partly"] for o in mine),
            "skipped_by_reason": _by([o["why"] for o in mine if o["outcome"] == "skipped"]),
            "skipped_paths_by_reason": _by([w for o in mine for w in o["paths"]]),
            "reputs": sum(o["outcome"] == "exists" for o in mine),
            "latency": [round(r.t_end - r.t_start, 3) for r in t.rounds if r.t_end is not None and r.t_start is not None],
        })
    return rows, event_started


def _by(whys):
    out = {}
    for w in whys:
        out[w or "?"] = out.get(w or "?", 0) + 1
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
    runs = {}  # (task, arm, repeat) → [its output, its user-initiated turns, the task's planned turns]
    for r in turn_rows:
        x = runs.setdefault((r["task"], r["arm"], r["repeat"]), [0, 0, r.get("planned_turns", 1)])
        x[0] += r["output"]
        x[1] += 1
    diffs, totals = {}, {}
    for task in sorted({k[0] for k in runs}):
        per = {arm: {rp: v for (t, a, rp), v in runs.items() if t == task and a == arm} for arm in ("on", "off")}
        totals[task] = {arm: {"run_output": [per[arm][rp][0] for rp in sorted(per[arm])],
                              "turns": [per[arm][rp][1] for rp in sorted(per[arm])]} for arm in per}
        if per["on"] and per["off"]:
            diffs[task] = (statistics.mean(v[0] / v[2] for v in per["on"].values())
                           - statistics.mean(v[0] / v[2] for v in per["off"].values()))
    unmatched = sorted([t, rp] for (t, a, rp), v in runs.items() if a == "on" and (t, "off", rp) in runs and runs[(t, "off", rp)][1] != v[1])
    out["output_on_minus_off"] = {"median": median(list(diffs.values())), "tasks": len(diffs), "per_task": diffs,
                                  "run_totals": totals, "unmatched_turns": unmatched,
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


def played(category, rows, steps):
    """Whether one off-run, its turn rows and its harness steps, played its
    category's shape (module docs)."""
    rounds = sum(r["rounds"] for r in rows)
    if category == "coding":
        return rounds >= 6
    if category == "short":
        return 1 <= rounds <= 2
    if category == "no_tool":
        return bool(rows) and all(r["tool_calls"] == 0 for r in rows)
    if category == "interactive":
        return any(s.get("step") in ("answer", "hand_back") and not s.get("skipped") for s in steps)
    return False


def set_shape(records, turn_rows):
    """The set's shape as the run played it (module docs)."""
    rows_of_run = {}
    for r in turn_rows:
        if r["arm"] == "off":
            rows_of_run.setdefault((r["task"], r["repeat"]), []).append(r)
    out, holds = {}, True
    for cat, says in SHAPE.items():
        runs = {}
        for rec in records:
            if rec.get("category") == cat and rec["arm"] == "off":
                ok = played(cat, rows_of_run.get((rec["task"], rec["repeat"]), []), rec.get("steps", []))
                runs.setdefault(rec["task"], []).append(ok)
        held = sorted(t for t, oks in runs.items() if sum(oks) >= 2)
        need = taskset.CATEGORIES[cat]
        out[cat] = {"plays": says, "tasks": len(runs), "held": len(held), "need": need,
                    "short_of": sorted(set(runs) - set(held)), "held_tasks": held}
        if cat == "coding":
            rounds = {(t, rp): sum(r["rounds"] for r in rows) for (t, rp), rows in rows_of_run.items()}
            out[cat]["apart"] = [{"task": t, "rounds": [rounds.get((t, rp)) for rp in REPEATS]} for t in out[cat]["short_of"]]
        holds = holds and len(held) >= need
    out["holds"] = holds
    return out


def eligibility(records, turn_rows, counter, not_run, meta, plan, shape):
    """Why the run can't flip the default: empty when it can (module docs,
    "The verdict")."""
    out = []
    if meta.get("mode") != "real":
        out.append(f"a {meta.get('mode')} run, never data")
    c = counter.describe()
    if not c.get("can_flip_default"):
        out.append(f"the counter is {c.get('kind')}, which can't flip the default" + (f": {c['why_not']}" if c.get("why_not") else ""))
    frozen = taskset.FROZEN.read_text().split()[0]
    if meta.get("taskset") != frozen:
        out.append(f"task set {str(meta.get('taskset'))[:12]}, not the frozen {frozen[:12]}")
    if meta.get("repeats") != len(REPEATS):
        out.append(f"{meta.get('repeats')} repeats, not §2.8's {len(REPEATS)}")
    if plan is None:
        out.append("no plan to check the runs against")
    else:
        want = collections.Counter(plan)
        have = collections.Counter((r["task"], r["arm"], r["repeat"]) for r in records)
        missing, extra = want - have, have - want
        if missing:
            out.append(f"{sum(missing.values())} of the plan's {sum(want.values())} runs missing (an interrupted run is incomplete)")
        if extra:
            out.append(f"{sum(extra.values())} runs the plan doesn't have, or twice")
    if meta.get("stopped"):
        out.append(f"the run stopped ({meta['stopped']})")
    stopped = [r for r in records if r.get("stopped")]
    if stopped:
        out.append(f"{len(stopped)} runs stopped: " + ", ".join(sorted({r['stopped'] for r in stopped})))
    measured = collections.defaultdict(list)
    for r in turn_rows:
        measured[(r["task"], r["arm"], r["repeat"])].append(r.get("measured", False))
    bad = [rec for rec in records if not measured.get((rec["task"], rec["arm"], rec["repeat"]))
           or not all(measured[(rec["task"], rec["arm"], rec["repeat"])])]
    if bad:
        out.append(f"{len(bad)} runs with a turn not measured (no idle, an error, aborted, or a response with no usage)")
    if not_run:
        out.append(f"{len(not_run)} tasks not run ({', '.join(t['id'] for t in not_run)}): B6 needs them, both arms (B5b)")
    if not shape["holds"]:
        out.append("the set's shape doesn't hold")
    probes = notice_probes(records)
    refused = sum(p["refused"] for p in probes.values())
    if refused:
        out.append(f"the desk refused {refused} dismissals")
    dismissing = {t["id"] for t in taskset.load() if any("dismiss" in s for s in t["turns"])}
    short = sorted(t for t in {task for task, _, _ in plan or []} & dismissing if probes.get(t, {}).get("delivered", 0) < 2)
    if short:
        out.append(f"the notice probe didn't reach the agent in 2 of 3 on-runs: {', '.join(short)}")
    return out


def notice_probes(records):
    """Per task with a dismissal, its on-runs' probes (module docs): how many
    on-runs, how many delivered the notice, refused or skipped it."""
    out = {}
    for rec in records:
        steps = [s for s in rec.get("steps", []) if s.get("step") == "dismiss"]
        if rec["arm"] != "on" or not steps:
            continue
        p = out.setdefault(rec["task"], {"runs": 0, "delivered": 0, "refused": 0, "skipped": 0})
        p["runs"] += 1
        p["refused"] += sum(s.get("outcome") == "refused" for s in steps)
        p["skipped"] += sum(s.get("outcome") == "skipped" or "skipped" in s for s in steps)
        ids = {s["id"] for s in steps if s.get("outcome") == "accepted"}
        rode = {n[0] for m in rec.get("sent", []) for n in m.get("notices", []) if len(n) >= 2 and n[1] == "dismissed"}
        p["delivered"] += bool(ids & rode)
    return out


def build(records, counter, definition, not_run, meta, plan=None):
    """The whole report (module docs). `plan` is the run's prescribed runs,
    `(task, arm, repeat)` (harness.plan, three repeats), for eligibility."""
    turn_rows, event_started, judged = [], [], []
    for rec in records:
        rows, ev = per_turn(rec, counter)
        turn_rows += rows
        event_started += [dict(e, task=rec["task"], arm=rec["arm"], repeat=rec["repeat"]) for e in ev]
        if (rec.get("judge") or {}).get("pass") is not None:
            judged.append({"task": rec["task"], "arm": rec["arm"], "repeat": rec["repeat"], "pass": rec["judge"]["pass"]})
    pooled = rows_of(turn_rows, judged, definition)
    ran = {rec["repeat"] for rec in records}
    by_repeat = {r: rows_of([x for x in turn_rows if x["repeat"] == r], [j for j in judged if j["repeat"] == r], definition)
                 if r in ran else {k: {"pass": False, "runs": 0} for k in GATE} for r in REPEATS}
    gate = {}
    for k in GATE:
        held = [r for r in REPEATS if by_repeat[r][k]["pass"]]
        gate[k] = {"kind": GATE[k]["kind"], "pooled": pooled[k], "repeats_passing": held,
                   "pass": pooled[k]["pass"] and len(held) >= 2}
    on = [r for r in turn_rows if r["arm"] == "on"]
    also = {
        "closing_input": sum(r["closing"]["input"] for r in on),
        "result_input": sum(r["attributed"]["result_input"] for r in on if r["attributed"]),
        "unseen_ops": sum(r["attributed"]["unseen_ops"] for r in on if r["attributed"]),
        "unseen_tokens": sum(r["attributed"]["unseen_tokens"] for r in on if r["attributed"]),
        "refusals": sum(r["refusals"] for r in on),
        "repairs": sum(r["repairs"] for r in on),
        "reputs": sum(r["reputs"] for r in on),
        "partly_skipped_ops": sum(r.get("partly_skipped", 0) for r in on),
        "skipped_by_reason": _merge(r["skipped_by_reason"] for r in on),
        "skipped_paths_by_reason": _merge(r["skipped_paths_by_reason"] for r in on),
        "notices_delivered": sum(1 for rec in records for ln in rec["fold"]["lines"] if ln.get("did") == "notice"),
        "notice_probes": notice_probes(records),
        "interaction_failures": [{"task": rec["task"], "arm": rec["arm"], "repeat": rec["repeat"], "why": s["skipped"]}
                                 for rec in records for s in rec.get("steps", []) if s.get("interaction")],
        "latency_per_round": {arm: {"median": median(lat), "p90": p90(lat), "rounds": len(lat)}
                              for arm in ("on", "off") for lat in [[x for r in turn_rows if r["arm"] == arm for x in r["latency"]]]},
        "reported_per_arm": {arm: _merge([r["usage"] for r in turn_rows if r["arm"] == arm]) for arm in ("on", "off")},
    }
    shape = set_shape(records, turn_rows)
    also["by_bucket"] = by_bucket(records, turn_rows, judged, definition, shape)
    rows_pass = all(g["pass"] for g in gate.values())
    why_not = eligibility(records, turn_rows, counter, not_run, meta, plan, shape)
    verdict = "not eligible" if why_not else ("passes" if rows_pass else "fails")
    return {
        "meta": meta | {"counter": counter.describe()},
        "turns": len(turn_rows), "event_started": event_started,
        "not_run": [{"task": t["id"], "needs": t["needs"], "why": f"needs {', '.join(t['needs'])}, not run"} for t in not_run],
        "gate": gate, "set_shape": shape, "rows_pass": rows_pass,
        "eligible_for_b6": not why_not, "not_eligible_because": why_not, "verdict": verdict, "gate_pass": verdict == "passes",
        "also": also,
        "outcomes": judged,
        "turn_rows": turn_rows,
    }


BUCKETS = ("long", "coding_under_6", "short", "no_tool", "interactive")


def by_bucket(records, turn_rows, judged, definition, shape):
    """The gate's rows over each bucket (module docs, "Long tasks"):
    reported, with no gate."""
    cat = {rec["task"]: rec.get("category") for rec in records}
    long = set(shape["coding"]["held_tasks"])
    def bucket(task):
        c = cat.get(task)
        return ("long" if task in long else "coding_under_6") if c == "coding" else c
    out = {}
    for b in BUCKETS:
        tasks = sorted(t for t in cat if bucket(t) == b)
        rows = rows_of([r for r in turn_rows if bucket(r["task"]) == b], [j for j in judged if bucket(j["task"]) == b], definition)
        out[b] = {"tasks": tasks, "rows": {k: {x: y for x, y in v.items() if x not in ("pass", "per_task", "run_totals")}
                                           for k, v in rows.items() if k != "definition"}}
    return out


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
        p = {x: y for x, y in g["pooled"].items() if x not in ("pass", "per_task", "run_totals")}
        lines.append(f"| {k} | {g['kind']} | `{json.dumps(p, sort_keys=True)}` | {g['repeats_passing']} | {'yes' if g['pass'] else '**no**'} |")
    s = rep["set_shape"]
    cats = ", ".join(f"{c} {s[c]['held']} of {s[c]['tasks']} ({s[c]['plays']}; ≥ {s[c]['need']})"
                     + (f", short: {', '.join(f'`{t}`' for t in s[c]['short_of'])}" if s[c]["short_of"] else "")
                     for c in SHAPE)
    apart = s["coding"].get("apart") or []
    lines += ["", f"The set's shape, blocks off: {'holds' if s['holds'] else '**does not hold**'}: {cats}."]
    if apart:
        lines.append("Coding tasks under 6 rounds blocks off, reported apart, not long: "
                     + ", ".join(f"`{a['task']}` ({', '.join('-' if x is None else str(x) for x in a['rounds'])})" for a in apart) + ".")
    lines += [f"Gate rows: {'every row passes' if rep['rows_pass'] else '**not every row passes**'} (pooled and in 2 of 3 repeats).",
              "Eligible for B6: " + ("yes" if rep["eligible_for_b6"] else "**no**: " + "; ".join(rep["not_eligible_because"])) + ".",
              f"Gate: {rep['verdict'] if rep['verdict'] == 'passes' else '**' + rep['verdict'] + '**'}."]
    lines += ["", f"Also reported: `{json.dumps({k: v for k, v in rep['also'].items() if k != 'reported_per_arm'}, sort_keys=True)}`", ""]
    if rep["not_run"]:
        lines.append("Not run: " + ", ".join(f"`{t['task']}` ({t['why']})" for t in rep["not_run"]) + ".")
    if rep["event_started"]:
        lines.append(f"Event-started turns, outside every denominator: {len(rep['event_started'])}.")
    return "\n".join(lines) + "\n"
