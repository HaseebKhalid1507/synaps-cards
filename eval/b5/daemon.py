"""The eval's own daemon, isolated as scripts/smoke.py isolates its one
(its module docs; `main` 348-445, `Daemon` 264-345), and its sessions:

- everything runs in a throwaway Synaps home under the run's work
  directory: `HOME`, `SYNAPS_BASE_DIR`, and a short `SYNAPS_RUNTIME_DIR` of
  its own, so nothing of the user's is read or written and no running
  daemon is touched;
- the harness talks only to the socket it saw appear in its own runtime
  directory while the daemon it started is alive, and over it sends nothing
  past `hello` unless the daemon's `welcome` names that daemon's pid;
- every daemon client command carries that runtime directory and
  `SYNAPS_DAEMON_AUTOSPAWN=0`, so none can find or start another daemon;
- its sessions are owned: they end when the harness's connection does.

The plugin is installed as a user installs it: this checkout cloned into
`<home>/plugins/cards` at its HEAD, then `scripts/setup.sh`. The harness's
helper, `cards-eval`, is built in the same clone. Blocks go on and off as
§1.1 says: `extension.cards.blocks` in the home's config, then
`synaps daemon reload-extension cards`.
"""

import json
import os
import queue
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROTOCOL = 3  # the daemon's session wire (SynapsCLI `session/wire.rs`, PROTOCOL_VERSION)


def lines_of(stream):
    q = queue.Queue()

    def pump():
        try:
            for line in stream:
                q.put(line)
        except (OSError, ValueError):
            pass
        q.put(None)

    threading.Thread(target=pump, daemon=True).start()
    return q


class Isolated:
    """The daemon (module docs). `env` adds the provider's variables."""

    def __init__(self, synaps, work, env=None):
        self.synaps = str(Path(synaps).resolve())
        self.work = Path(work)
        self.home = self.work / "home"
        self.base = self.home / ".synaps-cli"
        self.cwd = self.work / "cwd"
        self.extra = dict(env or {})
        self.daemon, self.sock, self.sessions = None, None, []
        self.record = {"synaps": self.synaps}

    def install(self):
        for d in (self.base, self.cwd):
            d.mkdir(parents=True, exist_ok=True)
        self.run_dir = Path(tempfile.mkdtemp(prefix="b5-", dir="/tmp"))  # the socket path must stay short
        v = subprocess.run([self.synaps, "--version"], capture_output=True, text=True)
        self.record["synaps_version"] = v.stdout.strip()
        self.version = self.record["synaps_version"].split()[-1] if self.record["synaps_version"] else "?"
        plugin = self.base / "plugins" / "cards"
        head = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
        if not plugin.exists():
            subprocess.run(["git", "clone", "-q", str(ROOT), str(plugin)], check=True)
        subprocess.run(["git", "-C", str(plugin), "checkout", "-q", head], check=True)
        self.record["head"] = head
        setup = subprocess.run(["bash", "scripts/setup.sh"], cwd=plugin, capture_output=True, text=True)
        if setup.returncode != 0:
            raise RuntimeError(f"scripts/setup.sh failed: {(setup.stdout + setup.stderr)[-1500:]}")
        build = subprocess.run(["cargo", "build", "--release", "--locked", "-q", "--target-dir", "target", "-p", "cards-eval"], cwd=plugin,
                               capture_output=True, text=True)
        if build.returncode != 0:
            raise RuntimeError(f"building cards-eval failed: {build.stderr[-1500:]}")
        self.cards_eval = str(plugin / "target" / "release" / "cards-eval")
        self.config = self.base / "config"
        self.config_was = self.config.read_text() if self.config.exists() else ""
        return self

    def env(self):
        return {"HOME": str(self.home), "PATH": os.environ["PATH"], "USER": os.environ.get("USER", "b5"), "LANG": "C.UTF-8",
                "TERM": "dumb", "SYNAPS_BASE_DIR": str(self.base), "SYNAPS_RUNTIME_DIR": str(self.run_dir),
                "SYNAPS_DAEMON_AUTOSPAWN": "0", "RUST_LOG": "warn",
                # The agent's Python writes no bytecode: a stale .pyc (trusted by
                # its source's whole-second mtime and size) would make a fast
                # same-length edit invisible to the next test run, by chance.
                "PYTHONDONTWRITEBYTECODE": "1", **self.extra}

    def start(self):
        self.dlog = open(self.work / "daemon.log", "w")
        self.daemon = subprocess.Popen([self.synaps, "daemon", "--foreground", "--idle-exit", "900"], env=self.env(), cwd=self.cwd,
                                       stdin=subprocess.DEVNULL, stdout=self.dlog, stderr=subprocess.STDOUT)
        for _ in range(300):
            if any(self.run_dir.glob("*.sock")) or self.daemon.poll() is not None:
                break
            time.sleep(0.1)
        sock = self.run_dir / "daemon.sock"
        if not (sock.exists() and self.daemon.poll() is None):
            raise RuntimeError(f"the daemon made no socket in its own runtime dir: {sorted(map(str, self.run_dir.glob('*')))}")
        self.sock = sock
        self.record["daemon_pid"] = self.daemon.pid
        return self

    def blocks(self, on):
        """Turn blocks on or off (§1.1): the config, then the reload. The
        extension's tools as the reload reports them."""
        self.config.write_text(self.config_was + ("\nextension.cards.blocks = true\n" if on else ""))
        r = subprocess.run([self.synaps, "daemon", "reload-extension", "cards", "--json"], env=self.env(), cwd=self.cwd,
                           capture_output=True, text=True, timeout=120)
        try:
            out = json.loads(r.stdout)
        except json.JSONDecodeError:
            raise RuntimeError(f"reload-extension: {r.stdout[-400:]} {r.stderr[-400:]}") from None
        tools = sorted((out.get("report") or {}).get("tools") or [])
        want = ["cards:block", "cards:ui"] if on else ["cards:ui"]
        if r.returncode != 0 or out.get("ok") is not True or tools != want:
            raise RuntimeError(f"reload-extension, blocks {'on' if on else 'off'}: {json.dumps(out)[:800]}")
        return tools

    def session(self, cwd, model, name):
        if self.daemon.poll() is not None:
            raise RuntimeError("the daemon started here has exited")
        s = Session(self.sock, cwd, name, self.version, self.daemon.pid, model)
        self.sessions.append(s)
        return s

    def stop(self):
        for s in self.sessions:
            s.close()
        stop = None
        if self.daemon and self.daemon.poll() is None:
            stop = subprocess.run([self.synaps, "daemon", "stop"], env=self.env(), cwd=self.cwd, capture_output=True, text=True, timeout=60).returncode
        if self.daemon:
            try:
                self.daemon.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.daemon.terminate()
                self.daemon.wait(timeout=10)
            self.dlog.close()
        self.config.write_text(self.config_was)
        shutil.rmtree(self.run_dir, ignore_errors=True)
        self.record.update({"daemon_stop": stop, "daemon_exit": self.daemon.returncode if self.daemon else None,
                            "runtime_dir_left": self.run_dir.exists()})
        return self.record


class Session:
    """One owned scratch session of the eval's daemon, on its session wire.
    `events` is this session's `(t, ev)` pairs, in wire order; `prompts`
    every prompt the daemon raised, each denied."""

    def __init__(self, sock, cwd, name, version, pid, model):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(str(sock))
        self.q = lines_of(self.sock.makefile("r", encoding="utf-8"))
        self.frames, self.events, self.prompts, self.qid, self.t0 = [], [], [], 0, time.monotonic()
        self.send({"type": "hello", "protocol_version": PROTOCOL, "cwd": str(cwd), "client_version": version, "env": None,
                   "client": {"kind": "attach", "terminal": None, "instance": str(uuid.uuid4()), "name": name}})
        self.welcome = self.next(30)
        if self.welcome.get("type") != "welcome" or self.welcome.get("pid") != pid:
            self.close()
            raise RuntimeError(f"not the daemon started here (pid {pid}): {json.dumps(self.welcome)[:400]}")
        if "owned_sessions" not in self.welcome.get("features", []):
            self.close()
            raise RuntimeError(f"the daemon has no owned sessions: {self.welcome.get('features')}")
        self.send({"type": "attach", "attach": "create", "mode": "mirror", "owned": {},
                   "config": {"cwd": str(cwd), "model_override": model}})
        f = self.until(lambda f: f.get("type") in ("attached", "error", "refused", "bye"), 120)
        if f.get("type") != "attached":
            raise RuntimeError(f"create: {json.dumps(f)[:600]}")
        self.meta = f["meta"]
        self.id = self.meta["id"]

    def send(self, frame):
        self.sock.sendall((json.dumps(frame, ensure_ascii=False) + "\n").encode())

    def cmd(self, cmd):
        self.send({"type": "cmd", "session_id": self.id, "cmd": cmd})

    def submit(self, text):
        self.cmd({"cmd": "submit", "text": text, "attachments": []})

    def steer(self, text):
        self.cmd({"cmd": "steer", "text": text})

    def cancel(self):
        self.cmd({"cmd": "cancel"})

    def next(self, timeout):
        """The next frame, recorded; this session's events kept as `(t, ev)`.
        None on a timeout."""
        try:
            line = self.q.get(timeout=timeout)
        except queue.Empty:
            return None
        if line is None:
            raise RuntimeError("the daemon's stream closed")
        try:
            f = json.loads(line)
        except json.JSONDecodeError:
            return {}
        self.frames.append(f)
        if f.get("type") == "event" and f.get("session_id") == getattr(self, "id", None):
            ev = f.get("event") or {}
            self.events.append((round(time.monotonic() - self.t0, 3), ev))
            if ev.get("ev") == "prompt":  # nothing here asks: record it, deny it
                self.prompts.append(ev)
                self.cmd({"cmd": "answer", "prompt_id": ev.get("request", {}).get("id"), "value": None})
        return f

    def until(self, pred, timeout):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            f = self.next(max(0.05, end - time.monotonic()))
            if f is not None and pred(f):
                return f
        raise RuntimeError(f"timed out after {timeout}s; last frames: {json.dumps(self.frames[-3:])[:1500]}")

    def messages(self):
        """The session's history as the daemon holds it (`SessionQuery::Messages`)."""
        self.qid += 1
        qid = self.qid
        self.cmd({"cmd": "query", "id": qid, "query": {"query": "messages"}})
        f = self.until(lambda f: f.get("type") == "event" and f.get("session_id") == self.id
                       and (f.get("event") or {}).get("ev") == "query_result" and f["event"].get("id") == qid, 60)
        return f["event"].get("value")

    def close(self):
        try:
            self.send({"type": "bye"})
        except OSError:
            pass
        self.sock.close()
