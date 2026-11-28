#!/usr/bin/env python3
"""B5's runs (docs/blocks.md §2.8): the frozen task set, blocks on and off,
three repeats, interleaved, against the eval's own isolated daemon
(b5/daemon.py), then the report (b5/report.py).

    eval/run.py --dry-run --synaps PATH [--work DIR] [--tasks ID,…] [--repeats N] [--keep] [--d8-stand-in]
    eval/run.py --provider anthropic --model claude-… --budget-usd 40 --approved claude-…@40 \\
                --desk 'CMD' --synaps PATH [--work DIR]
    eval/run.py --estimate [--json]

**The dry run** is free: the provider is the scripted model (b5/scripted.py)
served here, the counter is calibrated against it with its own tokenizer
(`bytes4`), and the desk is the stand-in (b5/desk.py). It proves the
harness end to end: install, the reloads, every runnable task in both arms
(with `--d8-stand-in`, the hand-back tasks too, against the stand-in's D8),
the steers, the card answers, the dismiss notices, the fold report, the
judge, the calibration, the definition control and the report. Its numbers
are a stand-in's and say so; they are never data.

**A real run** spends money, so it takes all of these, or it doesn't start:
- `--approved MODEL@USD`, equal to `--model` and `--budget-usd`: the
  approval, written out (Haseeb approves the model and the budget);
- `--desk CMD`: synaps-desk's adapter for `b5-desk/1`, with D6b's dismiss
  notices (and D8, for the hand-back tasks; without it they are "not run");
- the provider's credential in the environment (`ANTHROPIC_API_KEY`), never
  on the command line.
Its daemon's provider is reached through the tap (b5/providers.py), so the
first requests and the latencies are recorded as sent. Every paid request
is charged against the budget and written to `paid.jsonl` as it comes, the
probes of a provider with no token-count endpoint included (spend.py). A
run the budget stops (a soft stop: spend.py) still writes its report, of
every run that spent, the stopped one marked: an incomplete experiment,
never eligible (report.py).

**The estimate** (`--estimate`) prints the spend of a full run per
candidate model (b5/spend.py), from the set's own shape: no provider call.

**The run's record** (`report.json`'s `meta`) is what B5b needs to say it ran
the same harness: the head and whether its tree was clean, the task set's
digest, the synaps binary's path, version and SHA-256, the desk adapter's
handshake (its `impl` and `commit`) and executable, and the nonsecret
configuration (`config`: provider, upstream base URL, model, the proxy
and its tokenizer's version when there is no endpoint, Python, the
platform). Credentials are never in it.

Everything a run makes is under `--work`: `records.jsonl`, `report.json`,
`report.md`, `calibration.json`, `paid.jsonl` (a real run's), the daemon's
log and each run's workspace.
"""

import argparse
import json
import os
import platform
import shlex
import shutil
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from b5 import calibrate, count, daemon, desk, harness, providers, proxy, report, scripted, spend, taskset  # noqa: E402
from b5.estimate import table  # noqa: E402


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument("--dry-run", action="store_true", help="the scripted model and the stand-in desk: free, never data")
    mode.add_argument("--provider", choices=("anthropic", "openai_compatible"), help="a real run (spends money)")
    mode.add_argument("--estimate", action="store_true", help="the spend estimate of a full run, per candidate model")
    ap.add_argument("--synaps", help="the synaps binary (built from SynapsCLI dev)")
    ap.add_argument("--work", help="where the run's files go (default: a new temporary directory)")
    ap.add_argument("--keep", action="store_true", help="keep the run's workspaces")
    ap.add_argument("--tasks", help="only these task ids, comma-separated (a dry run's subset; a real run runs the set)")
    ap.add_argument("--repeats", type=int, default=3)
    ap.add_argument("--model")
    ap.add_argument("--base-url", help="openai_compatible: the provider's base URL")
    ap.add_argument("--proxy", default="cl100k", help="the proxy tokenizer, without an endpoint: cl100k or o200k")
    ap.add_argument("--budget-usd", type=float)
    ap.add_argument("--approved", help="MODEL@USD: the approval of this model and budget, written out")
    ap.add_argument("--desk", help="synaps-desk's b5-desk/1 adapter command")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--d8-stand-in", action="store_true",
                    help="a dry run's D8: the stand-in desk hands back, so the hand-back tasks run in both arms (never data)")
    a = ap.parse_args(argv)

    if a.estimate:
        t = table(taskset.load())
        print(json.dumps(t, indent=1) if a.json else t["markdown"])
        return 0
    if not a.synaps:
        ap.error("--synaps is required for a run")
    tasks = taskset.load()
    problems = taskset.problems(tasks)
    if problems:
        ap.error("the task set isn't §2.8's: " + "; ".join(problems))
    frozen = taskset.FROZEN.read_text().split()[0]
    if taskset.digest() != frozen:
        ap.error("the task set differs from its frozen digest (tasks/set.sha256)")
    if a.provider:
        return real(a, tasks, frozen)
    return dry(a, tasks, frozen)


def dry(a, tasks, frozen):
    if a.tasks:
        keep = set(a.tasks.split(","))
        unknown = keep - {t["id"] for t in tasks}
        if unknown:
            raise SystemExit(f"no such task: {', '.join(sorted(unknown))}")
        tasks = [t for t in tasks if t["id"] in keep]
    if a.d8_stand_in:  # the hand-back tasks, both arms, against the stand-in's D8 (desk.py)
        tasks = [dict(t, needs=[n for n in t["needs"] if n != "D8"]) for t in tasks]
    work = Path(a.work or tempfile.mkdtemp(prefix="b5-dry-")).resolve()
    work.mkdir(parents=True, exist_ok=True)
    model = scripted.Scripted()
    iso = daemon.Isolated(a.synaps, work, env={"LOCAL_ENDPOINT": model.url})
    d = desk.StandIn()
    d.hello()
    started = time.time()
    try:
        iso.install()
        iso.start()
        # The calibration, against the scripted model with its own tokenizer.
        calls, _ = calibrate.load()
        tools = _block_tools(iso)
        prov = providers.OpenAICompatible("stub", model.url)
        fit, pairs = calibrate.calibrate(calls, tools, lambda b: prov.count(b), proxy.bytes4, "bytes4")
        out = calibrate.output_control(calls, tools, prov.force, lambda s: fit.ratio * proxy.bytes4(s))
        fit.models = tuple(sorted(prov.models))
        counter = count.Scripted(fit, proxy.bytes4, out)
        (work / "calibration.json").write_text(json.dumps({"config": calibrate.config(prov, tools), "counter": counter.describe(),
                                                           "fit": fit.summary(), **pairs, "output_control": out}, indent=1))
        model.requests.clear()
        records = []
        definition = run_all(a, tasks, iso, "local/stub", d, work, records, model=model, dry=True,
                             count_request=lambda body: prov.count(body))
    finally:
        rec = iso.stop()
        model.close()
    return finish(a, work, records, counter, definition, tasks, frozen, rec, started, "dry", "local/stub (scripted)",
                  config(a, prov, d, endpoint=False))


def real(a, tasks, frozen):
    if a.repeats != len(report.REPEATS):
        raise SystemExit(f"a real run is §2.8's {len(report.REPEATS)} repeats, not --repeats {a.repeats}")
    if not (a.model and a.budget_usd and a.approved and a.desk):
        raise SystemExit("a real run takes --model, --budget-usd, --approved MODEL@USD and --desk (run.py's docs)")
    if a.approved != f"{a.model}@{a.budget_usd:g}":
        raise SystemExit(f"--approved {a.approved!r} isn't this run's {a.model}@{a.budget_usd:g}")
    if a.tasks:
        raise SystemExit("a real run runs the whole frozen set")
    if a.d8_stand_in:
        raise SystemExit("--d8-stand-in is the dry run's: a real run takes D8 from synaps-desk")
    work = Path(a.work or tempfile.mkdtemp(prefix="b5-run-")).resolve()
    work.mkdir(parents=True, exist_ok=True)
    budget = spend.Budget(a.budget_usd, a.model, ledger=work / "paid.jsonl")
    the_desk = desk.for_real_run(desk.Process(a.desk))
    has_d8 = "D8" in the_desk.impl
    for t in tasks:
        if has_d8 and t["needs"] == ["D8"]:
            t["needs"] = []
    if a.provider == "anthropic":
        key = os.environ.get("ANTHROPIC_API_KEY")
        if not key:
            raise SystemExit("ANTHROPIC_API_KEY isn't set")
        prov = providers.Anthropic(a.model, key, on_usage=budget.paid("probe"))
        tap = providers.Tap(providers.ANTHROPIC)
        env = {"ANTHROPIC_API_KEY": key, "SYNAPS_ANTHROPIC_BASE_URL": tap.url}
        model_id = a.model
    else:
        if not a.base_url:
            raise SystemExit("openai_compatible takes --base-url")
        prov = providers.OpenAICompatible(a.model, a.base_url, os.environ.get("OPENAI_API_KEY"), on_usage=budget.paid("probe"))
        tap = providers.Tap(a.base_url)
        env = {"LOCAL_ENDPOINT": tap.url}
        model_id = f"local/{a.model}"
    iso = daemon.Isolated(a.synaps, work, env=env)
    started = time.time()
    records, definition, stopped = [], None, None
    counter = count.NoCounter("the run stopped before its calibration finished")
    try:
        iso.install()
        iso.start()
        tools = _block_tools(iso)
        calls, _ = calibrate.load()
        # The output control (calibrate.py): a call's framing on output; paid
        # with an endpoint too, so each request is charged as a probe.
        if prov.has_endpoint:
            framing, raw = calibrate.endpoint(calls, tools, prov.count)
            counter = count.Endpoint(prov.count, tools, framing, raw)
            counter.output = calibrate.output_control(calls, tools, prov.force, counter.text)
            counter.models = tuple(sorted(prov.models))
            body = {"framing": framing, **raw, "output_control": counter.output}
        else:
            px = proxy.get(a.proxy)
            fit, raw = calibrate.calibrate(calls, tools, prov.count, px, a.proxy)
            out = calibrate.output_control(calls, tools, prov.force, lambda s: fit.ratio * px(s))
            fit.models = tuple(sorted(prov.models))
            counter = count.Calibrated(fit, px, out)
            body = {"fit": fit.summary(), **raw, "output_control": out}
        (work / "calibration.json").write_text(json.dumps({"config": calibrate.config(prov, tools), "counter": counter.describe(),
                                                           **body}, indent=1))
        tap.take()
        definition = run_all(a, tasks, iso, model_id, the_desk, work, records, tap=tap, budget=budget,
                             count_request=prov.count)
    except spend.OverBudget as e:
        stopped = f"over budget: {e}"
        print(f"stopped: {e}", file=sys.stderr)
    finally:
        rec = iso.stop()
        tap.close()
        the_desk.close()
    rec["spent_usd"], rec["stopped"] = round(budget.spent, 4), stopped
    return finish(a, work, records, counter, definition, tasks, frozen, rec, started, "real", a.model,
                  config(a, prov, the_desk, endpoint=prov.has_endpoint))


def config(a, prov, the_desk, endpoint):
    """The run's nonsecret configuration (module docs, "The run's record")."""
    out = {"provider": "scripted" if a.dry_run else a.provider, "base_url": getattr(prov, "base", None),
           "model": getattr(prov, "model", None), "repeats": a.repeats, "python": platform.python_version(),
           "platform": platform.platform(), "desk": {"handshake": the_desk.handshake}}
    if a.desk and not a.dry_run:
        exe = shlex.split(a.desk)[0]
        path = shutil.which(exe) or exe
        out["desk"]["executable"] = daemon.binary_digest(path) if os.path.isfile(path) else {"path": exe}
    if not endpoint and not a.dry_run:
        out["proxy"] = a.proxy
        try:
            import tiktoken
            out["tiktoken"] = tiktoken.__version__
        except ImportError:
            out["tiktoken"] = None
    return out


def _block_tools(iso):
    """The tools every probe carries (calibrate.py): `cards_block` as the
    extension declares it (`cards-ext`'s `tool.rs` `initialize`: its
    `description-block.md` and cards-spec's `schema-blocks.json`), from the
    installed checkout. Both requests of a pair carry it, so it cancels out
    of every difference; the definition itself is measured on the engine's
    own first request (count.definition)."""
    plugin = Path(iso.base, "plugins", "cards")
    return [{"name": "cards_block",
             "description": (plugin / "crates/cards-ext/src/description-block.md").read_text().rstrip(),
             "input_schema": json.loads((plugin / "crates/cards-spec/schema-blocks.json").read_text())}]


def run_all(a, tasks, iso, model_id, the_desk, work, records, model=None, tap=None, budget=None, dry=False, count_request=None):
    """Every run of the plan, each record appended to `records` and to
    `records.jsonl` as it finishes, a run the budget stopped included (it
    raises on, its record kept): the definition control's count."""
    order, _ = harness.plan(tasks, a.repeats)
    if tap is not None:
        def take():
            return [{"t_sent": r["t_sent"], "t_done": r.get("t_done"), "request": r["request"]} for r in tap.take()]
        to_neutral = providers.from_anthropic if a.provider == "anthropic" else providers.from_chat
    else:
        def take():
            out = list(model.requests)
            model.requests.clear()
            return [{"request": r} for r in out]
        to_neutral = providers.from_chat
    runner = harness.Runner(iso, model_id, the_desk, work, scripted=model, budget=budget, requests=take, dry=dry)
    out = open(work / "records.jsonl", "w")
    firsts = {}

    def keep(rec):
        out.write(json.dumps(rec, ensure_ascii=False) + "\n")
        out.flush()
        records.append(rec)

    try:
        for n, (r, task, arm) in enumerate(order):
            if budget and not budget.room_for_another():
                raise spend.OverBudget(f"no room for run {n} of {len(order)}: spent {budget.spent:.2f} of {budget.usd:.2f} USD, "
                                       f"the largest run {budget.largest:.2f}")
            try:
                rec = runner.run(n, r, task, arm)
            except spend.OverBudget as e:
                if getattr(e, "record", None) is not None:
                    keep(e.record)
                raise
            if budget:
                budget.run_done()
            first = next((q["request"] for q in rec["requests"] if q.get("request") and (q["request"].get("messages"))), None)
            if first is not None:
                firsts.setdefault(arm, to_neutral(first))
            keep(rec)
            print(f"{n + 1}/{len(order)} {task['id']} {arm} r{r + 1}: {'pass' if rec['judge']['pass'] else 'FAIL'}", flush=True)
    finally:
        out.close()
    definition = None
    if "on" in firsts:
        definition = count.definition(count_request, firsts["on"])
        if "off" in firsts:
            (work / "definition.json").write_text(json.dumps({"tokens": definition,
                                                              "else_differs": count.prefix_diff(firsts["on"], firsts["off"])}))
    return definition


def finish(a, work, records, counter, definition, tasks, frozen, daemon_rec, started, mode, model, cfg=None):
    # Eligibility checks the runs against the prescribed plan: three repeats, whatever --repeats was.
    order, not_run = harness.plan(tasks, len(report.REPEATS))
    meta = {"mode": mode, "model": model, "head": daemon_rec.get("head"), "taskset": frozen, "repeats": a.repeats,
            "synaps": daemon_rec.get("synaps_version"), "synaps_sha256": daemon_rec.get("synaps_sha256"),
            "head_dirty": daemon_rec.get("head_dirty"), "daemon": daemon_rec, "config": cfg or {},
            "secs": round(time.time() - started, 1), "stopped": daemon_rec.get("stopped")}
    rep = report.build(records, counter, definition, not_run, meta, plan=[(t["id"], arm, r + 1) for r, t, arm in order])
    (work / "report.json").write_text(json.dumps(rep, indent=1, ensure_ascii=False))
    (work / "report.md").write_text(report.markdown(rep))
    print(report.markdown(rep))
    ok = bool(records) and daemon_rec.get("daemon_exit") is not None and not daemon_rec.get("runtime_dir_left")
    print(f"work: {work}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
