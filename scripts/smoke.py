#!/usr/bin/env python3
"""Smoke-test the built extension in a real Synaps (design §9.2, S2; blocks B3).

    python3 scripts/smoke.py --synaps PATH [--work DIR] [--keep]

Everything runs in a throwaway Synaps home (`SYNAPS_BASE_DIR`, a short
`SYNAPS_RUNTIME_DIR`, and `HOME` for the Synaps processes, all under one
temporary directory), so nothing of the user's is read or written, and no
running daemon is touched: the daemon here is its own, on its own socket,
and the script stops only a daemon whose socket it saw appear in its own
runtime directory.

1. Install, as §1.1 says: `git clone` this checkout into
   `<home>/plugins/cards`, then `bash scripts/setup.sh` there.
2. A daemon in that home: `synaps daemon reload-extension cards --json` must
   load the plugin with its one tool `cards:ui` (blocks off, the manifest's
   default), then, with `extension.cards.blocks = true` in the home's
   config, with `cards:ui` and `cards:block` (`docs/blocks.md` §1.1).
3. A turn with blocks off: `synaps rpc --model local/stub`, its
   `LOCAL_ENDPOINT` a scripted OpenAI-compatible model served here, which
   only records the tools it is offered: `cards_ui`, and no `cards_block`.
4. A turn with blocks on (`SYNAPS_EXTENSION_CARDS_BLOCKS=true`). The model
   calls `cards_ui` six times (a card, a wait card, an invalid one, a
   close, a third card, a fourth that the rate limit refuses), then
   `cards_block` (§1.3's put; §2.1's tick riding a `read` in one message; an
   8 KiB call; 16 ops; an invalid call; 16-op calls until §7's 120 ops a
   minute refuses one), then starts a worker (`subagent`, on the same
   model) that tries both tools; then the user answers the first card with
   a §3.3 message. A tee in front of the built binary records the frames the
   host really sends it.

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
import oracle_blocks  # noqa: E402

FIX = ROOT / "fixtures"


def fixture(name):
    return json.loads((FIX / "valid" / f"{name}.json").read_text())


def block_case(corpus, about):
    return next(c for c in json.loads((FIX / "blocks" / f"{corpus}.json").read_text())["cases"] if c["about"] == about)


A1, A2, A3 = fixture("a1_plan_with_choices"), fixture("a2_approve_or_revise_wait"), fixture("a3_small_form")
GOLDEN = {c["fixture"]: c["result"] for c in json.loads((FIX / "results.json").read_text())["cases"] if "fixture" in c}
B_PUT = block_case("valid", "put: §1.3's example, with §1.5's plan sketch as its body (less its stop: the agent never names it)")
B_TICK = block_case("valid", "§2.1's two-step tick, state sugar: the same normalized op as the next case")
B_8K = block_case("limits", "a call's bytes (8192): at the bound")
B_16 = block_case("limits", "ops in a call (16): at the bound")
B_BAD = block_case("invalid", "§1.6's first refusal")
NOTES = "notes.txt"  # in the turn's cwd: what the tick's `read` reads
OPS_PER_MINUTE = 120
# What the scripted model calls, one round per assistant message with tool
# calls; each round is (what, [(tool, input), …]), the tool `ui`, `block` or
# `read`. A round with an expected golden checks the model saw it.
CARD_ROUNDS = [
    ("a1", [("ui", A1["input"])]),
    ("a2 (wait)", [("ui", A2["input"])]),
    ("invalid", [("ui", {"question": "Q", "options": ["only"]})]),
    ("close", [("ui", {"close": "c4f9x2a"})]),
    ("a3", [("ui", A3["input"])]),
    ("a1 again (the 4th card this minute)", [("ui", A1["input"])]),
]
BLOCK_ROUNDS = [
    ("block: §1.3's put", [("block", B_PUT["input"])]),
    ("block: §2.1's tick, riding a read", [("read", None), ("block", B_TICK["input"])]),
    ("block: an 8 KiB call", [("block", B_8K["input"])]),
    ("block: 16 ops", [("block", B_16["input"])]),
    ("block: §1.6's first refusal", [("block", B_BAD["input"])]),
]
# Then 16-op calls until the minute's ops are spent: the refused call is the last.
_used = sum(len(i["ops"]) for _, calls in BLOCK_ROUNDS for t, i in calls if t == "block" and i is not B_BAD["input"])
_fill = (OPS_PER_MINUTE - _used) // 16
BLOCK_ROUNDS += [(f"block: 16 ops, filling the minute ({k + 1}/{_fill})", [("block", B_16["input"])]) for k in range(_fill)]
BLOCK_ROUNDS += [("block: 16 ops past 120 a minute", [("block", B_16["input"])])]
RATE_LINE = f"cards_block: {_used + 16 * (_fill + 1)} ops in the last minute, over {OPS_PER_MINUTE}; patch less"
SCRIPT = CARD_ROUNDS + BLOCK_ROUNDS
WORKER_TASK = "SMOKE-WORKER: ask the user with cards_ui, and keep a block with cards_block."
SUBAGENT = {"system_prompt": "You are the cards smoke test's worker.", "name": "smoke-worker", "task": WORKER_TASK, "timeout": 90}
OFF_PROMPT = "SMOKE-OFF: which tools do you have?"

checks = []


def check(ok, what, detail=""):
    checks.append({"ok": bool(ok), "what": what, "detail": detail})
    print(("PASS " if ok else "FAIL ") + what + (f": {detail}" if detail and not ok else ""), flush=True)
    return ok


# ── The scripted model ───────────────────────────────────────────────────


class Model(BaseHTTPRequestHandler):
    seen = []  # every chat request's body
    worker_seen = []  # every worker request's body
    off_seen = []  # every request of the blocks-off turn
    tool_name = None
    read_path = None  # the absolute path the tick's `read` reads

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
        tool = {"ui": "cards_ui" if "cards_ui" in names else None, "block": "cards_block" if "cards_block" in names else None,
                "read": "read" if "read" in names else None}
        messages = req.get("messages", [])
        done = sum(1 for m in messages if m.get("role") == "assistant" and m.get("tool_calls"))
        last = messages[-1] if messages else {}
        text = last.get("content") if isinstance(last.get("content"), str) else json.dumps(last.get("content"))
        users = [str(m.get("content")) for m in messages if m.get("role") == "user"]
        if any(OFF_PROMPT in u for u in users):
            Model.off_seen.append(req)
            chunks = [self.text("Blocks are off here.")]
        elif any(WORKER_TASK in u for u in users):
            # The worker: a card attempt and a block attempt, each if offered, then done.
            Model.worker_seen.append(req)
            tries = [(t, i) for t, i in [("ui", A1["input"]), ("block", B_PUT["input"])] if tool[t]]
            if done < len(tries):
                chunks = [self.calls(f"call_w{done}", [(tool[tries[done][0]], tries[done][1])])]
            else:
                chunks = [self.text("Worker done.")]
        else:
            Model.seen.append(req)
            Model.tool_name = tool["ui"]
            if last.get("role") == "user" and text and text.startswith("[card "):
                chunks = [self.text("Thanks, I have the answer.")]
            elif done < len(SCRIPT) and all(tool[t] for t, _ in SCRIPT[done][1]):
                calls = [(tool[t], {"path": Model.read_path} if t == "read" else i) for t, i in SCRIPT[done][1]]
                chunks = [self.calls(f"call_{done}", calls)]
            elif done == len(SCRIPT) and "subagent" in names:
                chunks = [self.calls(f"call_{done}", [("subagent", SUBAGENT)])]
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

    def calls(self, call_id, calls):
        """One assistant message calling each (name, arguments) in order, a
        chunk per call as an OpenAI-compatible stream sends them."""
        out = []
        for k, (name, arguments) in enumerate(calls):
            delta = {"tool_calls": [{"index": k, "id": f"{call_id}_{k}" if len(calls) > 1 else call_id, "type": "function",
                                     "function": {"name": name, "arguments": json.dumps(arguments, ensure_ascii=False)}}]}
            if k == 0:
                delta = {"role": "assistant", "content": None, **delta}
            out.append(self.chunk(delta))
        return out + [self.chunk({}, "tool_calls")]

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

    # 2. A daemon in the throwaway home, then §1.1's third command: blocks
    # off (the manifest's default), then on through the home's config.
    config = base / "config"
    config_was = config.read_text() if config.exists() else None
    dlog = open(work / "daemon.log", "w")
    daemon = subprocess.Popen([synaps, "daemon", "--foreground", "--idle-exit", "120"], env=env, cwd=cwd,
                              stdin=subprocess.DEVNULL, stdout=dlog, stderr=subprocess.STDOUT)
    try:
        for _ in range(300):
            if any(run.glob("*.sock")) or daemon.poll() is not None:
                break
            time.sleep(0.1)
        sockets = sorted(str(p) for p in run.glob("*.sock"))
        report["daemon_sockets"] = sockets
        # Only ever this daemon: no socket in our own runtime dir, no client command.
        if check(sockets and daemon.poll() is None, "daemon: its own, on a socket in the throwaway runtime dir", str(sockets)):
            for gate, want in [(None, ["cards:ui"]), ("true", ["cards:ui", "cards:block"])]:
                if gate is not None:
                    config.write_text((config_was or "") + f"\nextension.cards.blocks = {gate}\n")
                r = subprocess.run([synaps, "daemon", "reload-extension", "cards", "--json"], env=env, cwd=cwd,
                                   capture_output=True, text=True, timeout=120)
                try:
                    out = json.loads(r.stdout)
                except json.JSONDecodeError:
                    out = {"stdout": r.stdout, "stderr": r.stderr}
                key = "reload_extension" if gate is None else "reload_extension_blocks_on"
                report[key] = {"exit": r.returncode, "out": out}
                tools = (out.get("report") or {}).get("tools") or []
                check(r.returncode == 0 and out.get("ok") is True and sorted(tools) == sorted(want),
                      f"daemon: reload-extension cards loads it, blocks {'off' if gate is None else 'on'}: {', '.join(want)}",
                      json.dumps(report[key])[:1500])
            if config_was is None:
                config.unlink()
            else:
                config.write_text(config_was)
            stop = subprocess.run([synaps, "daemon", "stop"], env=env, cwd=cwd, capture_output=True, text=True, timeout=60)
            report["daemon_stop"] = stop.returncode
    finally:
        try:
            daemon.wait(timeout=20)
        except subprocess.TimeoutExpired:
            daemon.terminate()
            daemon.wait(timeout=10)
        dlog.close()

    # 3 and 4. The turns. The built binary gets a tee in front, to record the host's frames.
    real = plugin / "target/release/cards-ext.real"
    shutil.move(plugin / "target/release/cards-ext", real)
    wrapper = plugin / "target/release/cards-ext"
    tee = shutil.which("tee") or "/usr/bin/tee"  # absolute: the host may start it with a reduced PATH
    (cwd / NOTES).write_text("The smoke test's notes, read beside a block call.\n")
    Model.read_path = str(cwd / NOTES)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Model)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env["LOCAL_ENDPOINT"] = f"http://127.0.0.1:{server.server_address[1]}/v1"
    try:
        off_wire = work / "wire-off.bin"
        wrapper.write_text(f"#!/bin/sh\n'{tee}' -a '{off_wire}' | '{real}'\n")
        wrapper.chmod(0o755)
        off = {}
        turn(synaps, env, cwd, work / "rpc-off.stderr.log", off, blocks=False)
        report["off"] = off
        judge_off(report, off_wire)

        wire = work / "wire.bin"
        wrapper.write_text(f"#!/bin/sh\n'{tee}' -a '{wire}' | '{real}'\n")
        env_on = {**env, "SYNAPS_EXTENSION_CARDS_BLOCKS": "true"}
        turn(synaps, env_on, cwd, work / "rpc.stderr.log", report, blocks=True)
    finally:
        server.shutdown()
    report["tool_name"] = Model.tool_name
    report["model_requests"] = len(Model.seen)
    report["worker_requests"] = len(Model.worker_seen)
    judge(report, wire)
    return finish(report, work, a.keep, run)


def turn(synaps, env, cwd, stderr, report, blocks):
    """One `synaps rpc` session: with blocks on, the scripted turn and the
    card's answer; off, one prompt the model answers in text."""
    rlog = open(stderr, "w")
    proc = subprocess.Popen([synaps, "rpc", "--model", "local/stub"], env=env, cwd=cwd, text=True,
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=rlog)
    q = lines_of(proc.stdout)
    events = []
    what = "rpc" if blocks else "rpc (blocks off)"
    try:
        ready = until(q, lambda e: e.get("type") == "ready", 120, events)
        report["ready"] = ready
        send = lambda c: (proc.stdin.write(json.dumps(c) + "\n"), proc.stdin.flush())
        if not blocks:
            send({"type": "prompt", "id": "p0", "message": OFF_PROMPT})
            until(q, lambda e: e.get("type") == "response" and e.get("id") == "p0", 120, events)
        else:
            send({"type": "prompt", "id": "p1", "message": "Smoke: ask me with cards, and keep blocks."})
            until(q, lambda e: e.get("type") == "response" and e.get("id") == "p1", 180, events)
            results = [e["event"] for e in events if e.get("type") == "message_update" and e.get("event", {}).get("type") == "toolcall_result"]
            report["results"] = [r.get("result") for r in results]
            report["result_events"] = [{k: v for k, v in r.items() if k != "result"} for r in results]
            send({"type": "get_messages", "id": "m1"})
            history = until(q, lambda e: e.get("type") == "response" and e.get("id") == "m1", 60, events)
            report["history"] = history
            cards = [oracle_card(r) for r in report["results"]]
            answer = None
            if cards and cards[0]:
                answer = oracle.message(cards[0], A1["doc"], "answer", {"choice": "Refactor the parser"})
                send({"type": "follow_up", "id": "p2", "message": answer})
                until(q, lambda e: e.get("type") == "response" and e.get("id") == "p2", 120, events)
                # The history once the answer's turn is done: the fold's real-host fixture (S3).
                send({"type": "get_messages", "id": "m2"})
                report["history_after"] = until(q, lambda e: e.get("type") == "response" and e.get("id") == "m2", 60, events)
            report["answer"] = answer
        send({"type": "shutdown"})
        proc.wait(timeout=30)
    except Exception as e:  # noqa: BLE001
        check(False, f"{what}: the turn ran", str(e))
        proc.kill()
        proc.wait(timeout=10)
    finally:
        rlog.close()
    report["rpc_exit"] = proc.returncode
    report["events_tail"] = events[-5:]


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


def call_id(r, k=0):
    """The id the scripted model gives call `k` of round `r`."""
    return f"call_{r}" if len(SCRIPT[r][1]) == 1 else f"call_{r}_{k}"


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(c.get("text", "") for c in content if isinstance(c, dict))
    return None


def tool_pairs(history):
    """The stored history's tool calls: id -> (name, input, result text)."""
    uses, results = {}, {}

    def walk(v):
        if isinstance(v, dict):
            if v.get("type") == "tool_use" and "id" in v:
                uses[v["id"]] = (v.get("name"), v.get("input"))
            if v.get("type") == "tool_result" and "tool_use_id" in v:
                results[v["tool_use_id"]] = text_of(v.get("content"))
            for x in v.values():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)

    walk(history)
    return {i: (n, inp, results.get(i)) for i, (n, inp) in uses.items()}


def judge_off(report, wire):
    off = report["off"]
    names = sorted({t.get("function", {}).get("name") for r in Model.off_seen for t in r.get("tools", [])})
    off["tools"] = names
    check(Model.off_seen and "cards_ui" in names and "cards_block" not in names,
          "blocks off: the model sees cards_ui and no cards_block", json.dumps(names))
    sent = frames(wire.read_bytes()) if wire.exists() else []
    init = next((f for f in sent if f.get("method") == "initialize"), None)
    off["host_initialize"] = init
    check(init and init.get("params", {}).get("config") == {"blocks": False},
          "blocks off: the host passes the manifest's default, config {\"blocks\": false}", json.dumps(init)[:400])
    check(off.get("rpc_exit") == 0, "rpc (blocks off): exits 0 after shutdown", str(off.get("rpc_exit")))


def judge(report, wire):
    # The tools as the model sees them.
    offered = {t["function"]["name"]: t["function"] for r in Model.seen for t in r.get("tools", []) if "function" in t}
    check(report["tool_name"] == "cards_ui", "the model sees cards_ui", str(report["tool_name"]))
    fn = offered.get("cards_ui")
    if fn:
        report["tool_as_sent"] = {"description_chars": len(fn.get("description", "")),
                                  "parameters_bytes": len(json.dumps(fn.get("parameters"), separators=(",", ":")))}
        check(fn.get("parameters") == json.loads((ROOT / "crates/cards-spec/schema.json").read_text()),
              "its parameters are schema.json")
    fb = offered.get("cards_block")
    check(fb is not None, "blocks on: the model sees cards_block", json.dumps(sorted(offered)))
    if fb:
        report["block_tool_as_sent"] = {"description_chars": len(fb.get("description", "")),
                                        "parameters_bytes": len(json.dumps(fb.get("parameters"), separators=(",", ":")))}
        check(fb.get("parameters") == json.loads((ROOT / "crates/cards-spec/schema-blocks.json").read_text()),
              "its parameters are schema-blocks.json")
        check(fb.get("description") == (ROOT / "crates/cards-ext/src/description-block.md").read_text().rstrip("\n"),
              "its description is §2.6's, as the extension declares it", repr(fb.get("description"))[:300])
    by_id = {e.get("tool_id"): r for e, r in zip(report.get("result_events", []), report.get("results", []))}
    ids = [call_id(r, k) for r in range(len(SCRIPT)) for k in range(len(SCRIPT[r][1]))]
    missing = [i for i in ids if i not in by_id]
    check(not missing, f"{len(ids)} results came back, one per scripted call", f"missing {missing}; got {sorted(by_id)}")
    report["subagent_result"] = by_id.get(f"call_{len(SCRIPT)}")
    results = [by_id.get(call_id(r)) for r in range(len(CARD_ROUNDS))]
    # What the model saw, call by call: the cards.
    for (what, _), got, (name, golden) in zip(CARD_ROUNDS, results, [("a1", GOLDEN["a1_plan_with_choices"]),
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
    # The blocks.
    nb = len(CARD_ROUNDS)
    block = lambda r, k=0: by_id.get(call_id(nb + r, k))
    bound = []
    for r, (what, calls) in enumerate(BLOCK_ROUNDS):
        for k, (t, inp) in enumerate(calls):
            if t != "block" or inp is B_BAD["input"] or r == len(BLOCK_ROUNDS) - 1:
                continue
            case = next(c for c in (B_PUT, B_TICK, B_8K, B_16) if c["input"] is inp)
            got = block(r, k)
            check(got == case["result"] and len(got.encode()) <= 64,
                  f"{what}: the result is the golden receipt, one line of at most 64 bytes", f"got {got!r}, want {case['result']!r}")
            bound.append(call_id(nb + r, k))
    read = block(1, 0)
    check(isinstance(read, str) and "The smoke test's notes" in read, "the read beside the tick ran", repr(read)[:300])
    # Both results of the tick's message came back in one round: the next request carries both.
    after = [m for req in Model.seen for m in [req.get("messages", [])]
             if sum(1 for x in m if x.get("role") == "assistant" and x.get("tool_calls")) == nb + 2]
    first = after[0] if after else []
    answered = {x.get("tool_call_id") for x in first if x.get("role") == "tool"}
    check({call_id(nb + 1, 0), call_id(nb + 1, 1)} <= answered,
          "the tick rode the read: both results came back in one round", json.dumps(sorted(answered))[:300])
    bad = block(4)
    check(isinstance(bad, str) and f"cards_block: {B_BAD['error']}" in bad and not bad.startswith("ok "),
          "block: the model reads §1.6's refusal line, no receipt", repr(bad))
    report["block_refusal_as_seen"] = bad
    last = block(len(BLOCK_ROUNDS) - 1)
    check(isinstance(last, str) and RATE_LINE in last, f"block: past 120 ops a minute, the rate line ({RATE_LINE})", repr(last))
    # The transcript the desk reads: every block call that got a receipt binds
    # it (the stored input, normalized by the oracle, gives the stored result).
    pairs = tool_pairs(report.get("history", {}))
    stored = {i: p for i, p in pairs.items() if p[0] in ("cards_block", "cards:block")}
    report["history_block_calls"] = {i: {"name": n, "result": res} for i, (n, _, res) in stored.items()}
    binds = [i for i, (_, inp, res) in stored.items()
             if isinstance(inp, dict) and res == oracle_blocks.result(oracle_blocks.normalize(inp))]
    check(sorted(binds) == sorted(bound) and len(stored) == sum(1 for _, c in BLOCK_ROUNDS for t, _ in c if t == "block"),
          f"history: the {len(bound)} receipts are stored with their calls and bind them; the refused calls don't",
          json.dumps({"binds": sorted(binds), "want": sorted(bound), "stored": sorted(stored)})[:800])
    hist = json.dumps(report.get("history", {}), ensure_ascii=False)
    check(oracle.envelope(oracle_card(results[0]) or "?", A1["doc"], False) in hist,
          "history: the stored result's line 1 is the envelope of a1's doc", "")
    # The answer reaches the model as an ordinary user message.
    answer = report.get("answer")
    got = [m for r in Model.seen for m in r.get("messages", []) if m.get("role") == "user" and m.get("content") == answer]
    check(answer and got, "the answer message reached the model, two lines as sent", repr(answer))
    stored = [m for m in report.get("history_after", {}).get("messages", []) if m.get("role") == "user" and m.get("content") == answer]
    check(answer and len(stored) == 1, "history: the answer is stored once, as the user's own string message", repr(stored)[:300])
    # The frames the host really sent the extension.
    sent = frames(wire.read_bytes()) if wire.exists() else []
    init = next((f for f in sent if f.get("method") == "initialize"), None)
    calls = [f for f in sent if f.get("method") == "tool.call"]
    report["host_initialize"] = init
    report["host_tool_call_keys"] = [sorted(c.get("params", {}).keys()) for c in calls]
    report["host_tool_call_first"] = {k: v for k, v in (calls[0]["params"] if calls else {}).items() if k != "input"}
    check(init and init.get("params", {}).get("extension_protocol_version") == 1, "host: initialize, protocol 1", json.dumps(init)[:400])
    check(init and init.get("params", {}).get("config") == {"blocks": "true"},
          "host: blocks on, the environment's value as a string, config {\"blocks\": \"true\"}", json.dumps(init)[:400])
    chat_calls = [c for c in calls if isinstance(c["params"].get("session_id"), str)]
    want = [t for _, c in SCRIPT for t, _ in c if t != "read"]
    check([c["params"].get("name") for c in chat_calls] == want,
          f"host: one tool.call per chat call, {want.count('ui')} named ui and {want.count('block')} named block",
          json.dumps([c["params"].get("name") for c in calls]))
    sent_inputs = [c["params"].get("input") for c in chat_calls if c["params"].get("name") == "block"]
    script_inputs = [i for _, c in BLOCK_ROUNDS for t, i in c if t == "block"]
    check(sent_inputs == script_inputs, "host: every block input is the model's, untouched (the 8 KiB one too)")
    # The worker: what dev sends for it, and what it was told.
    worker_tools = sorted({t.get("function", {}).get("name") for r in Model.worker_seen for t in r.get("tools", [])})
    report["worker_tools"] = worker_tools
    worker_calls = [c for c in calls if not isinstance(c["params"].get("session_id"), str)]
    report["host_worker_tool_call"] = [{k: v for k, v in c["params"].items() if k != "input"} for c in worker_calls]
    worker_saw = [m.get("content") for r in Model.worker_seen for m in r.get("messages", []) if m.get("role") == "tool"]
    report["worker_saw"] = worker_saw
    if not Model.worker_seen:
        check(False, "worker: the chat started one on the stub model", str(report.get("subagent_result"))[:600])
    else:
        lines = {"cards_ui": "cards_ui: only the main chat can show cards", "cards_block": "cards_block: only the main chat has blocks"}
        for tool, line in lines.items():
            if tool not in worker_tools:
                check(True, f"worker: it is not even offered {tool}", json.dumps(worker_tools))
            else:
                check(any(isinstance(s, str) and line in s for s in worker_saw),
                      f"worker: its {tool} call is refused with the worker line", json.dumps(worker_saw)[:600])
        offered_both = sum(1 for t in lines if t in worker_tools)
        check(len(worker_calls) == offered_both
              and all(c["params"].get("session_id") is None and ("caller" not in c["params"] or "parent" in c["params"]["caller"])
                      for c in worker_calls),
              "host: the worker's calls have no session, and a caller with a parent or none",
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
