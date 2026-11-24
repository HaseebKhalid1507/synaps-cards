#!/usr/bin/env python3
"""Smoke-test the built extension in a real Synaps (design §9.2, S2).

    python3 scripts/smoke.py --synaps PATH [--work DIR] [--keep]

Everything runs in a throwaway Synaps home (`SYNAPS_BASE_DIR`, a short
`SYNAPS_RUNTIME_DIR`, and `HOME` for the Synaps processes, all under one
temporary directory), so nothing of the user's is read or written, and no
running daemon is touched.

1. Install, as §1.1 says: `git clone` this checkout into
   `<home>/plugins/cards`, then `bash scripts/setup.sh` there.
2. A daemon in that home: `synaps daemon reload-extension cards --json` must
   load the plugin, with its tool `cards:ui`.
3. A turn: `synaps rpc --model local/stub`, its `LOCAL_ENDPOINT` a scripted
   OpenAI-compatible model served here. The model calls `cards_ui` six
   times (a card, a wait card, an invalid one, a close, a third card, a
   fourth that the rate limit refuses), then starts a worker (`subagent`,
   on the same model) that tries `cards_ui` too; then the user answers the
   first card with a §3.3 message. A tee in front of the built binary
   records the frames the host really sends it.

Each check is printed; the exit code is 0 only when all pass. The report,
with what the model saw and the host's frames, is `<work>/report.json`.
"""

import argparse
import json
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import oracle  # noqa: E402

FIX = ROOT / "fixtures"


def fixture(name):
    return json.loads((FIX / "valid" / f"{name}.json").read_text())


A1, A2, A3 = fixture("a1_plan_with_choices"), fixture("a2_approve_or_revise_wait"), fixture("a3_small_form")
GOLDEN = {c["fixture"]: c["result"] for c in json.loads((FIX / "results.json").read_text())["cases"] if "fixture" in c}
# What the scripted model calls, one per round, by how many tool results it has seen.
SCRIPT = [
    ("a1", A1["input"]),
    ("a2 (wait)", A2["input"]),
    ("invalid", {"question": "Q", "options": ["only"]}),
    ("close", {"close": "c4f9x2a"}),
    ("a3", A3["input"]),
    ("a1 again (the 4th card this minute)", A1["input"]),
]
WORKER_TASK = "SMOKE-WORKER: ask the user with cards_ui."
SUBAGENT = {"system_prompt": "You are the cards smoke test's worker.", "name": "smoke-worker", "task": WORKER_TASK, "timeout": 90}

checks = []


def check(ok, what, detail=""):
    checks.append({"ok": bool(ok), "what": what, "detail": detail})
    print(("PASS " if ok else "FAIL ") + what + (f": {detail}" if detail and not ok else ""), flush=True)
    return ok


# ── The scripted model ───────────────────────────────────────────────────


class Model(BaseHTTPRequestHandler):
    seen = []  # every chat request's body
    worker_seen = []  # every worker request's body
    tool_name = None

    def log_message(self, *args):
        pass

    def do_GET(self):
        body = json.dumps({"object": "list", "data": [{"id": "stub", "object": "model"}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        names = [t.get("function", {}).get("name") for t in req.get("tools", [])]
        cards = next((n for n in names if n and "cards" in n), None)
        messages = req.get("messages", [])
        done = sum(1 for m in messages if m.get("role") == "tool")
        last = messages[-1] if messages else {}
        text = last.get("content") if isinstance(last.get("content"), str) else json.dumps(last.get("content"))
        if any(m.get("role") == "user" and WORKER_TASK in str(m.get("content")) for m in messages):
            # The worker: one card attempt, then done.
            Model.worker_seen.append(req)
            if done == 0 and cards:
                chunks = [self.call("call_w0", cards, A1["input"])]
            else:
                chunks = [self.text("Worker done.")]
        else:
            Model.seen.append(req)
            Model.tool_name = cards
            if last.get("role") == "user" and text and text.startswith("[card "):
                chunks = [self.text("Thanks, I have the answer.")]
            elif done < len(SCRIPT) and cards:
                chunks = [self.call(f"call_{done}", cards, SCRIPT[done][1])]
            elif done == len(SCRIPT) and "subagent" in names:
                chunks = [self.call(f"call_{done}", "subagent", SUBAGENT)]
            else:
                chunks = [self.text("Smoke turn done.")]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for chunk in chunks:
            for c in chunk:
                self.wfile.write(b"data: " + json.dumps(c).encode() + b"\n\n")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    @staticmethod
    def chunk(delta, finish=None):
        return {"id": "chatcmpl-smoke", "object": "chat.completion.chunk", "created": 0, "model": "stub",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}

    def call(self, call_id, name, arguments):
        return [self.chunk({"role": "assistant", "content": None, "tool_calls": [
                    {"index": 0, "id": call_id, "type": "function",
                     "function": {"name": name, "arguments": json.dumps(arguments, ensure_ascii=False)}}]}),
                self.chunk({}, "tool_calls")]

    def text(self, content):
        return [self.chunk({"role": "assistant", "content": content}), self.chunk({}, "stop")]


# ── Processes ────────────────────────────────────────────────────────────


def lines_of(stream):
    q = queue.Queue()

    def pump():
        for line in stream:
            q.put(line)
        q.put(None)

    threading.Thread(target=pump, daemon=True).start()
    return q


def until(q, pred, timeout, events):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            line = q.get(timeout=max(0.05, end - time.monotonic()))
        except queue.Empty:
            break
        if line is None:
            raise RuntimeError("synaps rpc closed its stdout")
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        events.append(ev)
        if pred(ev):
            return ev
    raise RuntimeError(f"timed out after {timeout}s; last events: {json.dumps(events[-3:])[:2000]}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--synaps", required=True, help="the synaps binary to test against (built from dev)")
    ap.add_argument("--work", help="the throwaway directory (default: a new temporary one)")
    ap.add_argument("--keep", action="store_true", help="keep the throwaway directory")
    a = ap.parse_args()
    synaps = str(Path(a.synaps).resolve())
    work = Path(a.work or tempfile.mkdtemp(prefix="cards-smoke-"))
    work.mkdir(parents=True, exist_ok=True)
    home = work / "home"
    base = home / ".synaps-cli"
    run = Path(tempfile.mkdtemp(prefix="cs-", dir="/tmp"))  # the socket path must stay short
    cwd = work / "cwd"
    for d in (base, cwd):
        d.mkdir(parents=True, exist_ok=True)
    report = {"synaps": synaps, "work": str(work)}
    report["synaps_version"] = subprocess.run([synaps, "--version"], capture_output=True, text=True).stdout.strip()
    print(f"synaps: {report['synaps_version']} ({synaps})\nwork: {work}", flush=True)

    # 1. Install (§1.1): clone this checkout, then the plugin's setup script, as a user runs it.
    plugin = base / "plugins" / "cards"
    head = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    subprocess.run(["git", "clone", "-q", str(ROOT), str(plugin)], check=True)
    subprocess.run(["git", "-C", str(plugin), "checkout", "-q", head], check=True)
    report["head"] = head
    t = time.monotonic()
    setup = subprocess.run(["bash", "scripts/setup.sh"], cwd=plugin, capture_output=True, text=True)
    report["setup"] = {"exit": setup.returncode, "secs": round(time.monotonic() - t, 1), "tail": setup.stdout[-400:] + setup.stderr[-1200:]}
    if not check(setup.returncode == 0 and os.access(plugin / "target/release/cards-ext", os.X_OK),
                 "install: scripts/setup.sh builds target/release/cards-ext", report["setup"]["tail"]):
        return finish(report, work, a.keep, run)

    env = {"HOME": str(home), "PATH": os.environ["PATH"], "USER": os.environ.get("USER", "smoke"), "LANG": "C.UTF-8",
           "TERM": "dumb", "SYNAPS_BASE_DIR": str(base), "SYNAPS_RUNTIME_DIR": str(run), "SYNAPS_EXTENSIONS_TRACE": "1",
           "RUST_LOG": "warn"}

    # 2. A daemon in the throwaway home, then §1.1's third command.
    dlog = open(work / "daemon.log", "w")
    daemon = subprocess.Popen([synaps, "daemon", "--foreground", "--idle-exit", "120"], env=env, cwd=cwd,
                              stdin=subprocess.DEVNULL, stdout=dlog, stderr=subprocess.STDOUT)
    try:
        for _ in range(300):
            if any(run.glob("*.sock")) or daemon.poll() is not None:
                break
            time.sleep(0.1)
        r = subprocess.run([synaps, "daemon", "reload-extension", "cards", "--json"], env=env, cwd=cwd,
                           capture_output=True, text=True, timeout=120)
        try:
            out = json.loads(r.stdout)
        except json.JSONDecodeError:
            out = {"stdout": r.stdout, "stderr": r.stderr}
        report["reload_extension"] = {"exit": r.returncode, "out": out}
        tools = (out.get("report") or {}).get("tools") or []
        check(r.returncode == 0 and out.get("ok") is True and any("ui" in str(x) for x in tools),
              "daemon: reload-extension cards loads it, with its tool", json.dumps(report["reload_extension"])[:1500])
        stop = subprocess.run([synaps, "daemon", "stop"], env=env, cwd=cwd, capture_output=True, text=True, timeout=60)
        report["daemon_stop"] = stop.returncode
    finally:
        try:
            daemon.wait(timeout=20)
        except subprocess.TimeoutExpired:
            daemon.terminate()
            daemon.wait(timeout=10)
        dlog.close()

    # 3. A turn. The built binary gets a tee in front, to record the host's frames.
    real = plugin / "target/release/cards-ext.real"
    shutil.move(plugin / "target/release/cards-ext", real)
    wire = work / "wire.bin"
    wrapper = plugin / "target/release/cards-ext"
    wrapper.write_text(f"#!/bin/sh\ntee -a '{wire}' | '{real}'\n")
    wrapper.chmod(0o755)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Model)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env["LOCAL_ENDPOINT"] = f"http://127.0.0.1:{server.server_address[1]}/v1"
    rlog = open(work / "rpc.stderr.log", "w")
    proc = subprocess.Popen([synaps, "rpc", "--model", "local/stub"], env=env, cwd=cwd, text=True,
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=rlog)
    q = lines_of(proc.stdout)
    events = []
    try:
        ready = until(q, lambda e: e.get("type") == "ready", 120, events)
        report["ready"] = ready
        send = lambda c: (proc.stdin.write(json.dumps(c) + "\n"), proc.stdin.flush())
        send({"type": "prompt", "id": "p1", "message": "Smoke: ask me with cards."})
        until(q, lambda e: e.get("type") == "response" and e.get("id") == "p1", 180, events)
        results = [e["event"] for e in events if e.get("type") == "message_update" and e.get("event", {}).get("type") == "toolcall_result"]
        report["results"] = [r.get("result") for r in results]
        send({"type": "get_messages", "id": "m1"})
        history = until(q, lambda e: e.get("type") == "response" and e.get("id") == "m1", 60, events)
        report["history"] = history
        cards = [oracle_card(r) for r in report["results"]]
        answer = None
        if cards and cards[0]:
            answer = oracle.message(cards[0], A1["doc"], "answer", {"choice": "Refactor the parser"})
            send({"type": "follow_up", "id": "p2", "message": answer})
            until(q, lambda e: e.get("type") == "response" and e.get("id") == "p2", 120, events)
        report["answer"] = answer
        send({"type": "shutdown"})
        proc.wait(timeout=30)
    except Exception as e:  # noqa: BLE001
        check(False, "rpc: the turn ran", str(e))
        proc.kill()
    finally:
        rlog.close()
        server.shutdown()
    report["rpc_exit"] = proc.returncode
    report["events_tail"] = events[-5:]
    report["tool_name"] = Model.tool_name
    report["model_requests"] = len(Model.seen)
    report["worker_requests"] = len(Model.worker_seen)
    judge(report, wire)
    return finish(report, work, a.keep, run)


def oracle_card(result):
    if isinstance(result, str) and result.startswith("Card c") and " is showing (cards/1 #" in result:
        return result[5:12]
    return None


def frames(raw):
    out, i = [], 0
    while True:
        j = raw.find(b"\r\n\r\n", i)
        if j < 0:
            return out
        head = raw[i:j].decode()
        n = int(next(l.split(":", 1)[1] for l in head.split("\r\n") if l.lower().startswith("content-length")))
        out.append(json.loads(raw[j + 4:j + 4 + n]))
        i = j + 4 + n


def judge(report, wire):
    # The tool as the model sees it.
    tools = [t for r in Model.seen for t in r.get("tools", []) if t.get("function", {}).get("name") == report["tool_name"]]
    check(report["tool_name"] == "cards_ui", "the model sees cards_ui", str(report["tool_name"]))
    if tools:
        fn = tools[0]["function"]
        report["tool_as_sent"] = {"description_chars": len(fn.get("description", "")),
                                  "parameters_bytes": len(json.dumps(fn.get("parameters"), separators=(",", ":")))}
        check(fn.get("parameters") == json.loads((ROOT / "crates/cards-spec/schema.json").read_text()),
              "its parameters are schema.json")
    results = report.get("results", [])
    check(len(results) >= len(SCRIPT), f"{len(SCRIPT)} cards_ui results came back", json.dumps(results)[:800])
    report["subagent_result"] = results[len(SCRIPT)] if len(results) > len(SCRIPT) else None
    results = results[:len(SCRIPT)]
    while len(results) < len(SCRIPT):
        results.append(None)
    # What the model saw, call by call.
    for (what, inp), got, (name, golden) in zip(SCRIPT, results, [("a1", GOLDEN["a1_plan_with_choices"]),
                                                                   ("a2", GOLDEN["a2_approve_or_revise_wait"]),
                                                                   (None, None), (None, None),
                                                                   ("a3", GOLDEN["a3_small_form"]), (None, None)]):
        if golden:
            card = oracle_card(got)
            want = golden.replace({"a1": A1, "a2": A2, "a3": A3}[name]["card"], card or "?")
            check(got == want, f"{what}: the result is the golden one, with its new id", f"got {got!r}")
    check(isinstance(results[2], str) and "options: at least 2 (got 1); one option is not a choice" in results[2]
          and " is showing " not in results[2], "invalid: the model reads the validation line, no envelope", repr(results[2]))
    check(results[3] == "Card c4f9x2a closed (cards/1).", "close: its line", repr(results[3]))
    check(isinstance(results[5], str) and "cards_ui: 3 cards in the last minute; ask in plain text" in results[5],
          "the 4th card this minute: the rate line", repr(results[5]))
    report["refusal_as_seen"] = results[2]
    # The transcript the desk reads: the stored call and result bind.
    hist = json.dumps(report.get("history", {}), ensure_ascii=False)
    check(oracle.envelope(oracle_card(results[0]) or "?", A1["doc"], False) in hist,
          "history: the stored result's line 1 is the envelope of a1's doc", "")
    # The answer reaches the model as an ordinary user message.
    answer = report.get("answer")
    got = [m for r in Model.seen for m in r.get("messages", []) if m.get("role") == "user" and m.get("content") == answer]
    check(answer and got, "the answer message reached the model, two lines as sent", repr(answer))
    # The frames the host really sent the extension.
    sent = frames(wire.read_bytes()) if wire.exists() else []
    init = next((f for f in sent if f.get("method") == "initialize"), None)
    calls = [f for f in sent if f.get("method") == "tool.call"]
    report["host_initialize"] = init
    report["host_tool_call_keys"] = [sorted(c.get("params", {}).keys()) for c in calls]
    report["host_tool_call_first"] = {k: v for k, v in (calls[0]["params"] if calls else {}).items() if k != "input"}
    check(init and init.get("params", {}).get("extension_protocol_version") == 1, "host: initialize, protocol 1", json.dumps(init)[:400])
    chat_calls = [c for c in calls if isinstance(c["params"].get("session_id"), str)]
    check(len(chat_calls) == len(SCRIPT) and all(c["params"].get("name") == "ui" for c in calls),
          "host: one tool.call per chat call, named ui", json.dumps(report["host_tool_call_keys"]))
    # The worker: what dev sends for it, and what it was told.
    worker_tools = sorted({t.get("function", {}).get("name") for r in Model.worker_seen for t in r.get("tools", [])})
    report["worker_tools"] = worker_tools
    worker_calls = [c for c in calls if not isinstance(c["params"].get("session_id"), str)]
    report["host_worker_tool_call"] = [{k: v for k, v in c["params"].items() if k != "input"} for c in worker_calls]
    worker_saw = [m.get("content") for r in Model.worker_seen for m in r.get("messages", []) if m.get("role") == "tool"]
    report["worker_saw"] = worker_saw
    if not Model.worker_seen:
        check(False, "worker: the chat started one on the stub model", str(report.get("subagent_result"))[:600])
    elif report["tool_name"] not in worker_tools:
        check(True, "worker: it is not even offered cards_ui", json.dumps(worker_tools))
    else:
        check(any(isinstance(s, str) and "cards_ui: only the main chat can show cards" in s for s in worker_saw),
              "worker: its cards_ui call is refused with the worker line", json.dumps(worker_saw)[:600])
        w = worker_calls[0]["params"] if len(worker_calls) == 1 else {}
        check(len(worker_calls) == 1 and w.get("session_id") is None and ("caller" not in w or "parent" in w["caller"]),
              "host: the worker's call has no session, and a caller with a parent or none",
              json.dumps(report["host_worker_tool_call"]))
    check(calls and isinstance(calls[0]["params"].get("session_id"), str), "host: a chat's call carries its session_id",
          json.dumps(report["host_tool_call_first"]))
    check(calls and calls[0]["params"]["input"] == A1["input"], "host: the input is the model's, untouched")
    check(report.get("rpc_exit") == 0, "rpc: exits 0 after shutdown", str(report.get("rpc_exit")))


def finish(report, work, keep, run):
    report["checks"] = checks
    (work / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False))
    failed = [c for c in checks if not c["ok"]]
    print(f"\n{len(checks) - len(failed)}/{len(checks)} checks passed; report: {work / 'report.json'}", flush=True)
    shutil.rmtree(run, ignore_errors=True)
    if not keep and not failed:
        shutil.rmtree(work / "home", ignore_errors=True)
    return 1 if failed or not checks else 0


if __name__ == "__main__":
    sys.exit(main())
