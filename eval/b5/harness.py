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
- after each turn the desk settles on the session's history.

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
the ledger as it comes; a turn is cancelled when the total passes the
budget, and no run starts that could pass it. **A run the budget stopped**
still gives its record, marked `stopped`, with everything it had (events,
what was sent, the history and its fold when the session could give them;
no judge), so the report is of every run that spent: the stop is a soft one
(spend.py), and the report says the experiment is incomplete.
"""

import json
import subprocess
import time
from pathlib import Path

from . import taskset, turns
from .spend import OverBudget
from .turns import BLOCK_TOOL

TURN_TIMEOUT = 900
DRY_TURN_TIMEOUT = 120
QUEUED_GRACE = 1.5  # seconds after an idle in which a queued steer starts its turn


def plan(tasks, repeats=3):
    """[(repeat, task, arm)] in run order, and the tasks not run (module docs)."""
    runnable = [t for t in tasks if not t["needs"]]
    order = []
    for r in range(repeats):
        for i, t in enumerate(runnable):
            arms = ("on", "off") if (i + r) % 2 == 0 else ("off", "on")
            order.extend((r, t, a) for a in arms)
    return order, [t for t in tasks if t["needs"]]


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
        rec["judge"] = None if stop else taskset.judge(task, ws, text=_last_text(rec["events"]))
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
        seen = len(s.events)
        if "say" in starter:
            self.send(s, conv, "submit", starter["say"], rec)
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
            out = self.desk.hand_back(conv, starter["hand_back"], s.messages())
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
                    if self.budget and not self.budget.charge(ev["event"], "run"):
                        s.cancel()
                        raise OverBudget(f"spent {self.budget.spent:.2f} of {self.budget.usd:.2f} USD")
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
        self.desk.settle(conv, s.messages())
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
            rec["steps"].append({"step": "dismiss", "round": rounds, "id": block, "desk": r,
                                 "outcome": "accepted" if r.get("ok") else "refused"})
        else:
            rec["steps"].append({"step": "steer", "round": rounds})
            self.send(s, conv, "steer", f["steer"], rec, steer=True)


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


def live_block(board):
    """The block a dismissal clicks (module docs): the first of `board`'s
    live blocks (a `cards-eval fold` report) not dismissed and not pending."""
    for b in board.get("blocks") or []:
        if not b.get("dismissed") and not b.get("pending"):
            return b["id"]
    return None


def _last_text(events):
    """The text of the last turn's responses."""
    out, last = [], []
    for _, ev in events:
        if ev.get("ev") == "turn_started":
            out = []
        s = ev.get("event") or {}
        if s.get("llm") == "text":
            out.append(s.get("text", ""))
        if ev.get("ev") == "idle":
            last = out
    return "".join(last or out)
