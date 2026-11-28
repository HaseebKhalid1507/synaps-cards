"""The desk seam (docs/blocks.md §3.5, §8.2's D6 and D8): what the user's
desk does to the messages the eval sends. The eval drives the daemon as a
desk would, but the dismiss notices and the hand-backs are the desk's own
(decision, B5a): **a real run takes them from synaps-desk**, through this
seam, never from text the harness types.

**The protocol, `b5-desk/1`:** one JSON object per line each way, over a
process's stdin and stdout (`--desk CMD`), one reply per request, in order.
Every request names the `conversation` (the session's journal id) and
carries the session's `history` as the daemon holds it
(`SessionQuery::Messages`), so the desk folds the same board it would draw.

- `{"op": "hello", "protocol": "b5-desk/1"}` →
  `{"ok": true, "protocol": "b5-desk/1", "impl": "…", "commit": "…"}`.
  A real run takes a desk whose `impl` names `D6b` (and `D8` for a
  hand-back); it refuses any other.
- `{"op": "dismiss", "conversation", "id", "history"}` and the same with
  `"show"`: the user's click on block `id` (D6b's `Desires::dismiss`/`show`).
  → `{"ok": true}` or `{"ok": false, "refused": "…"}`.
- `{"op": "send", "conversation", "carry": "submit" | "steer" | "answer",
  "body", "history"}`: the user sends `body` (D6b's `Desires::ride`). →
  `{"text": what goes on the wire, "notices": [[id, "dismissed" | "shown",
  version], …], "ticket": …}`. The harness sends `text` exactly. A card
  answer carries nothing, and nothing rides an empty body (§3.5).
- `{"op": "settle", "conversation", "history", "lost": [ticket, …]}`: a
  settled history (D6b's `Desires::settle`, `lost`). → `{"ok": true,
  "feedback": […]}`.
- `{"op": "hand_back", "conversation", "block": {"id": …}, "edit": {…},
  "say": "…", "history"}` (D8): take block `id` (the harness picks it: the
  agent's latest live block of the task's kind), edit it as `edit` says,
  hand it back with `say`. → `{"text": the hand-back message, with its
  notices}` or `{"ok": false, "refused": "…"}`. `edit` is the task's
  (tasks/set.json), never the agent's proposal's: `{"append_step":
  "label"}` (a plan's new last step), `{"append_text": "…"}` (a note's
  text, appended) or `{"keep": "all"}` (a diff's every hunk kept).

**The stand-in** (`StandIn`) speaks the same protocol in process, for the
dry run only: it mints `v<N>` as one more than any version in the history
and its own desires, attaches at most 8 ids per line, dismissals first,
never on an empty body or a card answer, and settles nothing. Its hand-back
(for the dry run's `--d8-stand-in`) is `say` and a line naming the block
and the edit, a stand-in for D8's message. Its `impl` says so, and a real
run refuses it. It proves the harness's side of the seam; it is never
data.
"""

import json
import re
import subprocess

PROTOCOL = "b5-desk/1"
STAND_IN = "dry-run stand-in (not D6b)"
NOTICE = re.compile(r"^\[block ((?:[a-z][a-z0-9_-]{0,23} v[1-9][0-9]{0,8})(?:, [a-z][a-z0-9_-]{0,23} v[1-9][0-9]{0,8}){0,7}) (dismissed|shown)\]$")


class Desk:
    impl = "?"
    handshake = None  # the adapter's `hello` reply, kept for the run's record (impl, commit, protocol)

    def request(self, req):
        raise NotImplementedError

    def hello(self):
        r = self.request({"op": "hello", "protocol": PROTOCOL})
        if r.get("protocol") != PROTOCOL:
            raise RuntimeError(f"the desk speaks {r.get('protocol')!r}, not {PROTOCOL}")
        self.impl = r.get("impl", "?")
        self.handshake = r
        return r

    def dismiss(self, conversation, block, history):
        return self.request({"op": "dismiss", "conversation": conversation, "id": block, "history": history})

    def send(self, conversation, carry, body, history):
        return self.request({"op": "send", "conversation": conversation, "carry": carry, "body": body, "history": history})

    def settle(self, conversation, history, lost=()):
        return self.request({"op": "settle", "conversation": conversation, "history": history, "lost": list(lost)})

    def hand_back(self, conversation, spec, history):
        return self.request({"op": "hand_back", "conversation": conversation, "history": history, **spec})

    def close(self):
        pass


def for_real_run(desk, needs=("D6b",)):
    """Refuse a desk a real run can't take (module docs)."""
    desk.hello()
    missing = [n for n in needs if n not in desk.impl]
    if desk.impl == STAND_IN or missing:
        raise RuntimeError(f"a real run takes synaps-desk's {', '.join(needs)} through the seam; this desk is {desk.impl!r}")
    return desk


class Process(Desk):
    """A desk adapter process (`--desk CMD`) speaking `b5-desk/1`."""

    def __init__(self, cmd):
        self.p = subprocess.Popen(cmd, shell=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

    def request(self, req):
        self.p.stdin.write(json.dumps(req, ensure_ascii=False) + "\n")
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        if not line:
            raise RuntimeError(f"the desk adapter closed (exit {self.p.poll()})")
        return json.loads(line)

    def close(self):
        if self.p.poll() is None:
            self.p.stdin.close()
            try:
                self.p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.p.kill()


class StandIn(Desk):
    """The dry run's desk (module docs). Never data."""

    def __init__(self):
        self.desires = {}  # conversation → {id: (state, version, sent)}

    def request(self, req):
        op = req["op"]
        if op == "hello":
            return {"ok": True, "protocol": PROTOCOL, "impl": STAND_IN, "commit": None}
        conv = self.desires.setdefault(req.get("conversation"), {})
        if op in ("dismiss", "show"):
            v = 1 + max([_highest(req["history"])] + [d[1] for d in conv.values()])
            conv[req["id"]] = ("dismissed" if op == "dismiss" else "shown", v, False)
            return {"ok": True}
        if op == "send":
            body = req["body"]
            if req["carry"] == "answer" or not body.strip():
                return {"text": body, "notices": [], "ticket": None}
            ride = sorted(((i, s, v) for i, (s, v, sent) in conv.items() if not sent), key=lambda x: (x[1] != "dismissed", x[2]))
            lines = []
            for kind in ("dismissed", "shown"):
                ids = [x for x in ride if x[1] == kind][:8]
                if ids:
                    lines.append(f"[block {', '.join(f'{i} v{v}' for i, _, v in ids)} {kind}]")
                    for i, s, v in ids:
                        conv[i] = (s, v, True)
            rode = [[i, s, v] for i, s, v in ride if conv[i][2]]
            return {"text": body + "".join("\n" + ln for ln in lines), "notices": rode, "ticket": len(rode) or None}
        if op == "settle":
            return {"ok": True, "feedback": []}
        if op == "hand_back":
            return {"text": f"{req['say']}\n[stand-in hand-back of block {req['block']['id']}: {json.dumps(req['edit'], ensure_ascii=False)}]",
                    "notices": []}
        return {"ok": False, "refused": f"the stand-in has no {op}"}


def _highest(history):
    """The highest notice version in `history`'s user messages."""
    top = 0
    for m in history or []:
        if m.get("role") != "user":
            continue
        content = m.get("content")
        text = content if isinstance(content, str) else "".join(p.get("text", "") for p in content or [] if isinstance(p, dict) and p.get("type") == "text")
        for line in text.split("\n"):
            hit = NOTICE.match(line)
            if hit:
                top = max([top] + [int(x.rsplit(" v", 1)[1]) for x in hit.group(1).split(", ")])
    return top
