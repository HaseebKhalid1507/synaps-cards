"""The dry run's model: an OpenAI-compatible server, as smoke.py serves one,
that plays each task from the frozen set the same way every time, so the
harness can run end to end for free. Never data: its numbers are a
stand-in's, and the report says so.

**What it plays.** The harness names the run (`Scripted.current`: the task,
its workspace, the arm). Each turn of the task (its opening `say`, then the
turn a card answer starts) is a fixed list of rounds, each a list of tool
calls, then a final text; a request gets the first round whose calls aren't
in the history yet, so steers and notices change nothing. A coding turn
reads, runs the tests, applies the task's reference solution with the
engine's own `edit` and `write` tools, runs the tests again, and answers.
With blocks on it also keeps a plan: a `put` riding the first read, ticks
riding real calls, and one closing tick alone in its own round. A few
tasks add what the report must catch: a refused call and its repair
(`ledger-parens`), a `put` on a live id (`shelf-due`), and in the tasks
with a dismissal, the ticks that come after the notice (unseen patches).

**Its usage** is the stand-in tokenizer's: ⌈bytes / 4⌉ of the request's
messages and tools (input), of what it streams (output, plus 4 per call),
and its cache: the bytes this request shares as a prefix with the last one
of the run. A one-token completion (`max_tokens: 1`, not streamed) is a
probe: it answers with its usage only, so the calibration can run against
it.
"""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from . import taskset

CARDS = {
    "ledger-card-negative": {"question": "How should money.format print negative amounts?", "options": ["-12.50", "(12.50)"]},
    "shelf-card-period": {"question": "How long should a loan be?", "options": ["14 days", "21 days", "28 days"]},
    "ledger-card-header": {"question": "Which header holds the amount?", "options": ["amount", "value", "total"]},
    "shelf-card-prefix": {"question": "How should search match?", "options": ["Whole words only", "Word prefixes too"]},
    "ledger-card-zero": {"question": "Leave zero-balance accounts out of the trial balance?", "options": ["Yes, hide them", "No, keep them"]},
}
SHORT_READ = {"ledger": "ledger/accounts.py", "shelf": "shelf/loans.py"}


def _text(content):
    """A chat message's text: a string, or its text parts joined."""
    if isinstance(content, str):
        return content
    return "".join(p.get("text", "") for p in content or [] if isinstance(p, dict) and p.get("type") == "text")


def tokens(s):
    return (len(s.encode()) + 3) // 4


def plan_ops(steps):
    return {"ops": [{"op": "put", "id": "plan", "kind": "plan", "title": "Work through it",
                     "body": {"steps": [{"id": s, "label": s.capitalize()} for s in steps]}}]}


def tick(**states):
    return {"ops": [{"op": "patch", "id": "plan", "state": states}]}


def coding_rounds(task, ws, on, first):
    """The rounds of a coding turn (module docs)."""
    edits = json.loads((taskset.TASKS / task["solution"]).read_text())
    files = list(dict.fromkeys(e["path"] for e in edits))
    test = ("bash", {"command": "python3 -m unittest -q 2>&1 | tail -3"})
    rounds = [[("read", {"path": str(Path(ws) / files[0])})], [test]]
    for e in edits:
        p = str(Path(ws) / e["path"])
        rounds.append([("write", {"path": p, "content": e["content"]}) if "content" in e else
                       ("edit", {"path": p, "old_string": e["old"], "new_string": e["new"]})])
    rounds.append([test])
    if not on:
        return rounds
    rounds[0].append(("cards_block", plan_ops(["look", "fix", "test"]) if first else tick(look="doing")))
    rounds[1].append(("cards_block", tick(look="done", fix="doing")))
    if task["id"] == "ledger-parens":  # a refusal, then its repair
        rounds[1][-1] = ("cards_block", tick(look="finished"))
        rounds[2].append(("cards_block", tick(look="done", fix="doing")))
    if task["id"] == "shelf-due":  # a put on a live id
        rounds[2].append(("cards_block", plan_ops(["look", "fix", "test"])))
    rounds[-1].append(("cards_block", tick(fix="done", test="doing")))
    rounds.append([("cards_block", tick(test="done"))])  # the closing call, alone
    return rounds


def turn_rounds(task, ws, on, seg):
    """Turn `seg` of `task` (0: its opening, 1: after a card answer)."""
    cat = task["category"]
    if cat == "no_tool":
        return []
    if cat == "short":
        if "tests" in task["id"]:
            return [[("bash", {"command": "python3 -m unittest 2>&1 | tail -3"})]]
        return [[("read", {"path": str(Path(ws) / SHORT_READ[task["workspace"]])})]]
    if task["id"] in CARDS and seg == 0:
        return [[("cards_ui", CARDS[task["id"]])]]
    return coding_rounds(task, ws, on, first=(seg == 0 or task["id"] in CARDS))


def final_text(task, seg):
    c = task["check"]
    if "reference" in c:
        return c["reference"]
    if task["id"] in CARDS and seg == 0:
        return "Waiting for your answer."
    return f"Done: {task['id']}, the tests pass."


class Scripted:
    """The server (module docs). `current` names the run being played."""

    def __init__(self):
        self.current = None  # {"task": …, "workspace": …, "on": bool}
        self.requests, self.last = [], {}
        self.lock = threading.Lock()
        model = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                self._json({"object": "list", "data": [{"id": "stub", "object": "model"}]})

            def do_POST(self):
                req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                with model.lock:
                    model.requests.append(req)
                if req.get("max_tokens") == 1 and not req.get("stream"):
                    self._json({"id": "probe", "object": "chat.completion", "choices": [
                        {"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "length"}],
                        "usage": {"prompt_tokens": model.input_tokens(req), "completion_tokens": 1}})
                    return
                chunks, out = model.respond(req)
                prompt = model.input_tokens(req)
                cached = model.cached(req)
                usage = {"prompt_tokens": prompt, "completion_tokens": out, "prompt_tokens_details": {"cached_tokens": cached}}
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                for c in chunks + [{"id": "chatcmpl-b5", "object": "chat.completion.chunk", "choices": [], "usage": usage}]:
                    self.wfile.write(b"data: " + json.dumps(c).encode() + b"\n\n")
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()

            def _json(self, v):
                body = json.dumps(v).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server.server_address[1]}/v1"

    @staticmethod
    def _body(req):
        return json.dumps(req.get("messages", []), ensure_ascii=False) + json.dumps(req.get("tools", []), ensure_ascii=False)

    def input_tokens(self, req):
        return tokens(self._body(req))

    def cached(self, req):
        body = self._body(req)
        prev, self.last["body"] = self.last.get("body", ""), body
        n = 0
        for a, b in zip(prev, body):
            if a != b:
                break
            n += 1
        return min(tokens(body[:n]), self.input_tokens(req))

    def respond(self, req):
        cur = self.current
        names = [t.get("function", {}).get("name") for t in req.get("tools", [])]
        on = "cards_block" in names
        msgs = req.get("messages", [])
        seg = sum(1 for m in msgs if m.get("role") == "user" and _text(m.get("content")).startswith("[card "))
        made = {c.get("id") for m in msgs if m.get("role") == "assistant" for c in m.get("tool_calls") or []}
        if cur is None:
            return self.text("No run is being played."), tokens("No run is being played.")
        rounds = turn_rounds(cur["task"], cur["workspace"], on, seg)
        for r, calls in enumerate(rounds):
            ids = [f"call_s{seg}_r{r}_{k}" for k in range(len(calls))]
            if not set(ids) <= made:
                present = [(i, c) for i, c in zip(ids, calls) if c[0] in names]
                if not present:
                    continue
                out = sum(tokens(json.dumps(c[1], ensure_ascii=False)) + 4 for _, c in present)
                return self.calls(present), out
        text = final_text(cur["task"], seg)
        return self.text(text), tokens(text)

    @staticmethod
    def chunk(delta, finish=None):
        return {"id": "chatcmpl-b5", "object": "chat.completion.chunk", "created": 0, "model": "stub",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}

    def calls(self, calls):
        out = []
        for k, (cid, (name, args)) in enumerate(calls):
            delta = {"tool_calls": [{"index": k, "id": cid, "type": "function",
                                     "function": {"name": name, "arguments": json.dumps(args, ensure_ascii=False)}}]}
            if k == 0:
                delta = {"role": "assistant", "content": None, **delta}
            out.append(self.chunk(delta))
        return out + [self.chunk({}, "tool_calls")]

    def text(self, content):
        return [self.chunk({"role": "assistant", "content": content}), self.chunk({}, "stop")]

    def close(self):
        self.server.shutdown()
