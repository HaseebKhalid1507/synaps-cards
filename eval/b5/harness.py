"""The runs (docs/blocks.md §2.8, "Runs"): each task with blocks on and with
blocks off, on the same model and prompt, three times each, interleaved.

**The plan** (`plan`): for each repeat, every runnable task in the set's
order, its two arms back to back, the arm that goes first alternating by
task and by repeat (so neither arm always runs with a warmer cache or a
later clock). A task that `needs` D8 is in the report as "needs D8, not
run"; B5b runs it unchanged.

**One run** (`Runner.run`): a fresh copy of the task's workspace, seeded;
the daemon's blocks set for the arm (§1.1's config and reload, only when the
arm changes); one owned session of the eval's daemon, in that workspace, on
the run's model. Then the task's user side, step by step (taskset.py):
- each `say`, `answer` and `hand_back` starts a turn once the session is
  idle; the `steer` and `dismiss` steps after it belong to that turn and
  fire once it has had `after_round` rounds (provider responses);
- everything the user sends goes through the desk (desk.py), so a notice
  rides it exactly as the desk would send it, and a card answer is
  cards-spec's (`cards-eval answer`);
- a turn ends at the session's `idle`; a steer the engine queued past the
  turn's end starts its own turn, and the harness follows it;
- after each turn the desk settles on the session's history; a message
  sent that turn that the history doesn't have is lost (desk.py's
  `settle`): its ticket goes in `lost`, and its record says `lost`.

**A dismissal** clicks the first block on the board as the desk draws it
then: the session's history folded by cards-spec (`cards-eval fold`), its
first live block, not dismissed already and not pending. Never a block the
agent only tried to put: a refused put has no block, and a removed one is
gone. Each is recorded as it went: `accepted` or `refused` by the desk, or
`skipped` with why (no live block). The report counts the notices that
then rode a message to the agent (report.py's notice probes). **A card
answer** answers the last card the agent showed that's still showing: not
one it closed since.

Then the run's record: the session's events, its history, the fold report
(`cards-eval fold`), what the user sent, every prompt the daemon raised
(each denied: nothing in the eval asks), the requests the provider got,
and the judged outcome (taskset.py).

**The budget** (real runs, spend.py): every usage is priced and written to
the ledger exactly once, as the session records it (daemon.py's
`on_usage`), whatever read it: the turn's loop, a history query, or the
drain after a cancel or a timeout (review #14 round 2, R2-6). A turn is
cancelled when the total passes the budget, and no run starts that could
pass it. **A run the budget stopped** stops new work, then drains: it
reads the session until its `idle` (at most `DRAIN` seconds), so the
usage of what was already in flight is charged too, then takes the
history. Its record is marked `stopped`, with everything it had (events,
what was sent, the history and its fold when the session could give them;
no judge), and `drained`: true at the `idle`, or false, with
`in_flight_unknown`, when the session gave none in time (a response
still in flight may be paid and never seen). So the report is of every
run that spent: the stop is a soft one (spend.py), and the report says
the experiment is incomplete.
"""

import json
import subprocess
import time
from pathlib import Path

from . import desk, taskset, turns
from .spend import OverBudget
from .turns import BLOCK_TOOL

TURN_TIMEOUT = 900
DRY_TURN_TIMEOUT = 120
QUEUED_GRACE = 1.5  # seconds after an idle in which a queued steer starts its turn
DRAIN = 60  # seconds a stopped run waits for its session's idle (module docs, "The budget")


def plan(tasks, repeats=3):
    """[(repeat, task, arm)] in run order, and the tasks not run (module docs)."""
    runnable = [t for t in tasks if not t["needs"]]
    order = []
    for r in range(repeats):
        for i, t in enumerate(runnable):
            arms = ("on", "off") if (i + r) % 2 == 0 else ("off", "on")
            order.extend((r, t, a) for a in arms)
    return order, [t for t in tasks if t["needs"]]


def pilot_plan(tasks):
    """The depth pilot's runs (run.py's `--pilot`): every coding task of the
    set, blocks off, once, in the set's order."""
    return [(0, t, "off") for t in tasks if t["category"] == "coding"]


def groups(turns):
    """The task's steps as turns: each a starter and the steps that fire in it."""
    out = []
    for s in turns:
        if "steer" in s or "dismiss" in s:
            out[-1].append(s)
        else:
            out.append([s])
    return out


def helper(cards_eval, cmd, payload):
    """One request to `cards-eval <cmd>`."""
    line = payload if isinstance(payload, str) else json.dumps(payload, ensure_ascii=False)
    r = subprocess.run([cards_eval, cmd], input=line + "\n", capture_output=True, text=True, timeout=120)
    if r.returncode != 0:
        raise RuntimeError(f"cards-eval {cmd}: {r.stderr[-600:]}")
    return json.loads(r.stdout.strip().splitlines()[-1] if cmd != "fold" else r.stdout)


class Runner:
    def __init__(self, iso, model, desk, work, scripted=None, budget=None, requests=None, dry=False):
        self.iso, self.model, self.desk, self.work = iso, model, desk, Path(work)
        self.scripted, self.budget, self.dry = scripted, budget, dry
        self.requests = requests  # () → the provider requests since the last call
        self.arm = None
        self.timeout = DRY_TURN_TIMEOUT if dry else TURN_TIMEOUT
        self.over = None  # why the budget stopped the run, once it has

    def charge(self, usage):
        """A session's `on_usage` (module docs, "The budget"): every usage
        charged once; past the budget, the stop is noted, never raised
        mid-read, so a drain keeps charging."""
        if self.budget and not self.budget.charge(usage, "run") and self.over is None:
            self.over = f"spent {self.budget.spent:.2f} of {self.budget.usd:.2f} USD"

    def set_arm(self, arm):
        if arm != self.arm:
            self.tools = self.iso.blocks(arm == "on")
            self.arm = arm

    def run(self, n, repeat, task, arm):
        ws = taskset.prepare(task, self.work / "runs" / f"{n:03d}-{task['id']}-{arm}-r{repeat + 1}" / "ws")
        self.set_arm(arm)
        if self.scripted:
            self.scripted.current = {"task": task, "workspace": str(ws), "on": arm == "on"}
        if self.requests:
            self.requests()
        rec = {"n": n, "task": task["id"], "category": task["category"], "arm": arm, "repeat": repeat + 1,
               "planned_turns": len(groups(task["turns"])), "tools": self.tools, "workspace": str(ws), "sent": [], "steps": [], "t0": time.time()}
        s = self.iso.session(ws, self.model, f"b5-{task['id']}-{arm}-r{repeat + 1}")
        s.on_usage = self.charge
        conv = s.meta.get("journal_id") or s.id
        rec["session"] = {k: s.meta.get(k) for k in ("id", "journal_id", "host_pid", "model")}
        stop = None
        try:
            for g in groups(task["turns"]):
                left = self.turn(s, conv, g[0], g[1:], rec)
                for steer in left:  # the turn ended first: said instead, as the next turn's message
                    rec["steps"].append({"step": "steer", "said_instead": True})
                    self.turn(s, conv, {"say": steer["steer"]}, [], rec)
            rec["history"] = s.messages()
        except OverBudget as e:
            stop, rec["stopped"] = e, "over budget"
            rec["drained"] = drain(s)
            if not rec["drained"]:
                rec["in_flight_unknown"] = True
            try:
                rec["history"] = s.messages()
            except Exception as why:  # the session may be gone: the record keeps what it had
                rec["history"], rec["history_error"] = None, str(why)
        finally:
            rec["events"] = s.events
            rec["prompts"] = s.prompts
            s.close()
            self.iso.sessions.remove(s)
        rec["fold"] = (helper(self.iso.cards_eval, "fold", json.dumps(rec["history"], ensure_ascii=False))
                       if rec.get("history") is not None else {"lines": []})
        rec["requests"] = self.requests() if self.requests else []
        rec["judge"] = None if stop else taskset.judge(task, ws, text=final_text(rec["events"]))
        rec["t1"] = time.time()
        if stop is not None:
            stop.record = rec
            raise stop
        return rec

    def send(self, s, conv, carry, body, rec, steer=False):
        out = self.desk.send(conv, carry, body, s.messages())
        text = out["text"]
        (s.steer if steer else s.submit)(text)
        rec["sent"].append({"carry": carry, "text": text, "notices": out.get("notices", []), "ticket": out.get("ticket"),
                            "at_event": len(s.events)})

    def turn(self, s, conv, starter, fires, rec):
        """One turn (module docs): its unfired steers, when it ended first.
        It reads only the events after what it sends: an earlier turn's
        `idle` isn't this one's end."""
        seen, sent_before = len(s.events), len(rec["sent"])
        if "say" in starter:
            self.send(s, conv, "submit", starter["say_off"] if self.arm == "off" and "say_off" in starter else starter["say"], rec)
        elif "answer" in starter:
            card = _last_card(s.events)
            if card is None:
                rec["steps"].append({"step": "answer", "skipped": "no card showing"})
                return []
            want = starter["answer"]
            a = helper(self.iso.cards_eval, "answer", {"input": card[0], "result": card[1], "pick": want["pick"], **({"fill": want["fill"]} if "fill" in want else {})})
            if "error" in a:
                rec["steps"].append({"step": "answer", "skipped": a["error"], "interaction": bool(a.get("interaction"))})
                return []
            rec["steps"].append({"step": "answer", "card": a.get("card"), "values": a.get("values")})
            self.send(s, conv, "answer", a["text"], rec)
        elif "hand_back" in starter:
            hb = starter["hand_back"]
            history = s.messages()
            block = None
            if self.arm == "on":
                block = live_block(helper(self.iso.cards_eval, "fold", json.dumps(history, ensure_ascii=False)), hb["block"]["kind"], last=True)
            if block is None:
                step = {"step": "hand_back", "as": "message"}
                if self.arm == "on":
                    step["said_instead"] = f"no live {hb['block']['kind']} block"
                rec["steps"].append(step)
                self.send(s, conv, "submit", hb["message"], rec)
            else:
                out = self.desk.hand_back(conv, {"block": {"id": block}, "edit": hb["edit"], "say": hb["say"]}, history)
                if "text" not in out:
                    rec["steps"].append({"step": "hand_back", "skipped": f"the desk refused: {out.get('refused')}", "block": block})
                    return []
                rec["steps"].append({"step": "hand_back", "as": "block", "block": block})
                s.submit(out["text"])
                rec["sent"].append({"carry": "hand_back", "text": out["text"], "notices": out.get("notices", []), "at_event": len(s.events)})
        pending = list(fires)
        rounds, started = 0, False
        end = time.monotonic() + self.timeout
        while True:
            if time.monotonic() > end:
                s.cancel()
                rec["steps"].append({"step": "timeout", "after": self.timeout})
                s.until(lambda f: (f.get("event") or {}).get("ev") == "idle", 60)
                break
            s.next(0.5)
            idle = False
            while seen < len(s.events):
                _, ev = s.events[seen]
                seen += 1
                kind = ev.get("ev")
                started |= kind == "turn_started"
                if kind == "stream" and (ev.get("event") or {}).get("session") == "usage":
                    rounds += 1
                    if self.over:  # charged as it was recorded (`charge`)
                        s.cancel()
                        raise OverBudget(self.over)
                    for f in [f for f in pending if f["after_round"] <= rounds]:
                        pending.remove(f)
                        self.fire(s, conv, f, rec, rounds)
                idle |= kind == "idle" and started
            if idle:
                # A steer queued past the turn's last drain starts its own turn.
                grace = time.monotonic() + QUEUED_GRACE
                while time.monotonic() < grace and seen == len(s.events):
                    s.next(0.25)
                if seen < len(s.events) and any(e.get("ev") == "turn_started" for _, e in s.events[seen:]):
                    rounds = 0
                    continue
                break
        history = s.messages()
        if self.over:  # passed during a timeout's drain or the history's read
            raise OverBudget(self.over)
        lost = [m for m in rec["sent"][sent_before:] if not desk.received(history, m["text"])]
        for m in lost:
            m["lost"] = True
        self.desk.settle(conv, history, lost=[m["ticket"] for m in lost if m.get("ticket") is not None])
        left = [f for f in pending if "steer" in f]
        for f in pending:
            if "dismiss" in f:  # the turn ended first: the click still happens, and rides the next message
                self.fire(s, conv, f, rec, rounds)
        return left

    def fire(self, s, conv, f, rec, rounds):
        if "dismiss" in f:
            history = s.messages()
            block = live_block(helper(self.iso.cards_eval, "fold", json.dumps(history, ensure_ascii=False)))
            if block is None:
                rec["steps"].append({"step": "dismiss", "round": rounds, "outcome": "skipped", "skipped": "no live block"})
                return
            r = self.desk.dismiss(conv, block, history)
            rec["steps"].append({"step": "dismiss", "round": rounds, "id": block, "desk": r, "at_event": len(s.events),
                                 "outcome": "accepted" if r.get("ok") else "refused"})
        else:
            rec["steps"].append({"step": "steer", "round": rounds})
            self.send(s, conv, "steer", f["steer"], rec, steer=True)


def drain(s, timeout=None):
    """Read a cancelled session until its `idle` (module docs, "The
    budget"): every usage on the way is charged as it is recorded. Whether
    the session is idle: at once if its last turn has ended already (the
    engine sends a response's usage before its turn's `idle`)."""
    if ended(s.events):
        return True
    try:
        s.until(lambda f: (f.get("event") or {}).get("ev") == "idle", DRAIN if timeout is None else timeout)
    except Exception:  # timed out, or the session is gone: the wire says how far it got
        pass
    return ended(s.events)


def ended(events):
    """Whether the last turn of `events` has had its `idle` (or none began)."""
    for _, e in reversed(events):
        if e.get("ev") == "idle":
            return True
        if e.get("ev") == "turn_started":
            return False
    return True


def _last_card(events):
    """The last card the agent showed that it hasn't closed since: (its
    cards_ui input, its result)."""
    last = None
    for c, result in turns.calls(events):
        if c.name != "cards_ui" or not result:
            continue
        if result.startswith("Card c") and " is showing" in result:
            last = (c.input, result)
        elif last and isinstance(c.input, dict) and c.input.get("close") and last[1].startswith(f"Card {c.input['close']} ") \
                and not result.startswith("Tool execution failed"):
            last = None
    return last


def live_block(board, kind=None, last=False):
    """The block a dismissal clicks (module docs): the first of `board`'s
    live blocks (a `cards-eval fold` report) not dismissed and not pending;
    of `kind` if one is named; the latest if `last` (a hand-back's)."""
    ok = [b for b in board.get("blocks") or [] if not b.get("dismissed") and not b.get("pending") and (kind is None or b.get("kind") == kind)]
    return (ok[-1] if last else ok[0])["id"] if ok else None


def final_text(events):
    """The answer an `expect` judges (taskset.py): of the last turn's
    responses (turns.py's rounds), the last one with text. The text of the
    responses before it is narration ("I'll run the tests."), not the
    answer."""
    ts = turns.turns(events)
    return next((r.text for r in reversed(ts[-1].rounds) if r.text.strip()), "") if ts else ""
