"""Providers: the request shapes, the counts §2.8 takes from them, and the
tap that records what the engine sends.

**The neutral form** is Anthropic's Messages shape (`system`, `tools` with
`input_schema`, `messages` of content blocks). The method is written in it
(calibrate.py, count.py). An OpenAI-compatible provider's chat bodies are
converted to it (`from_chat`) and back (`to_chat`).

**Counting a request** (`Provider.count`):
- `anthropic`: the token-count endpoint, `POST /v1/messages/count_tokens`
  (free; exact). Its counts are `endpoint`'s.
- `openai_compatible`: no endpoint; a completion of one output token, and
  its reported input (`prompt_tokens`, which includes the cached part).
  Its counts calibrate the proxy. Each is paid: its usage goes to
  `on_usage` (spend.py's `Budget.paid`) before the count is returned.

**The tap** (`Tap`) sits between the daemon and its provider: the daemon's
base URL points at it (`SYNAPS_ANTHROPIC_BASE_URL`, or the local
provider's `LOCAL_ENDPOINT`), and it passes every request through
unchanged, recording each request's body, when it went and when its last
byte came back. The definition control reads the first request of a run;
latency per round is the tap's.
"""

import json
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ANTHROPIC = "https://api.anthropic.com"
ANTHROPIC_VERSION = "2023-06-01"


# ── The neutral form and OpenAI's chat ─────────────────────────────────────


def _text(content):
    if isinstance(content, str):
        return content
    return "".join(p.get("text", "") for p in content or [] if p.get("type") == "text")


def to_chat(body, model):
    """A neutral body as an OpenAI-compatible chat completion request."""
    msgs = []
    if body.get("system"):
        msgs.append({"role": "system", "content": body["system"] if isinstance(body["system"], str) else _text(body["system"])})
    for m in body["messages"]:
        parts = m["content"] if isinstance(m["content"], list) else [{"type": "text", "text": m["content"]}]
        if m["role"] == "assistant":
            calls = [{"id": p["id"], "type": "function", "function": {"name": p["name"], "arguments": json.dumps(p["input"], separators=(",", ":"), ensure_ascii=False)}}
                     for p in parts if p["type"] == "tool_use"]
            out = {"role": "assistant", "content": _text(parts) or None}
            if calls:
                out["tool_calls"] = calls
            msgs.append(out)
        else:
            for p in parts:
                if p["type"] == "tool_result":
                    msgs.append({"role": "tool", "tool_call_id": p["tool_use_id"], "content": _text(p["content"]) if not isinstance(p["content"], str) else p["content"]})
            text = _text(parts)
            if text:
                msgs.append({"role": "user", "content": text})
    tools = [{"type": "function", "function": {"name": t["name"], "description": t.get("description", ""), "parameters": t.get("input_schema", {})}}
             for t in body.get("tools") or []]
    out = {"model": model, "messages": msgs}
    if tools:
        out["tools"] = tools
    return out


def from_chat(req):
    """An OpenAI-compatible chat request in the neutral form."""
    system, messages = [], []
    for m in req.get("messages", []):
        role, content = m.get("role"), m.get("content")
        if role in ("system", "developer"):
            system.append(_text(content) if not isinstance(content, str) else content)
        elif role == "assistant":
            parts = []
            if content:
                parts.append({"type": "text", "text": _text(content) if not isinstance(content, str) else content})
            for c in m.get("tool_calls") or []:
                f = c.get("function", {})
                try:
                    inp = json.loads(f.get("arguments") or "{}")
                except json.JSONDecodeError:
                    inp = f.get("arguments")
                parts.append({"type": "tool_use", "id": c.get("id"), "name": f.get("name"), "input": inp})
            messages.append({"role": "assistant", "content": parts})
        else:
            part = ({"type": "tool_result", "tool_use_id": m.get("tool_call_id"), "content": _text(content) if not isinstance(content, str) else content}
                    if role == "tool" else {"type": "text", "text": _text(content) if not isinstance(content, str) else content})
            if messages and messages[-1]["role"] == "user":
                messages[-1]["content"].append(part)
            else:
                messages.append({"role": "user", "content": [part]})
    tools = [{"name": t["function"]["name"], "description": t["function"].get("description", ""), "input_schema": t["function"].get("parameters", {})}
             for t in req.get("tools") or [] if t.get("type") == "function"]
    return {"system": "\n\n".join(system), "tools": tools, "messages": messages}


def from_anthropic(req):
    """An Anthropic request (as the engine sent it) in the neutral form: the
    same, cut to what counts. `cache_control` marks are left in: they don't
    change a count."""
    system = req.get("system")
    if isinstance(system, list):
        system = _text(system)
    return {"system": system or "", "tools": req.get("tools") or [], "messages": req.get("messages") or []}


# ── Providers ──────────────────────────────────────────────────────────────


def post(url, body, headers, timeout=120):
    data = json.dumps(body, ensure_ascii=False).encode()
    r = urllib.request.Request(url, data=data, method="POST", headers={"content-type": "application/json", **headers})
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"{url}: HTTP {e.code}: {e.read()[:600]!r}") from None


class Anthropic:
    name = "anthropic"
    has_endpoint = True

    def __init__(self, model, api_key, base=ANTHROPIC):
        self.model, self.key, self.base = model.split("/", 1)[-1], api_key, base.rstrip("/")

    def headers(self):
        return {"x-api-key": self.key, "anthropic-version": ANTHROPIC_VERSION}

    def count(self, body):
        """The token-count endpoint's input tokens for a neutral body."""
        b = {"model": self.model, "messages": body["messages"]}
        if body.get("system"):
            b["system"] = body["system"]
        if body.get("tools"):
            b["tools"] = body["tools"]
        return post(f"{self.base}/v1/messages/count_tokens", b, self.headers())["input_tokens"]


class OpenAICompatible:
    name = "openai_compatible"
    has_endpoint = False

    def __init__(self, model, base, api_key=None, on_usage=None):
        self.model, self.base, self.key = model.split("/", 1)[-1], base.rstrip("/"), api_key
        self.on_usage = on_usage

    def count(self, body):
        """A one-token completion's reported input for a neutral body. The
        completion is paid: its usage goes to `on_usage` first, which may
        stop the run."""
        req = to_chat(body, self.model) | {"max_tokens": 1, "stream": False}
        headers = {"authorization": f"Bearer {self.key}"} if self.key else {}
        u = post(f"{self.base}/chat/completions", req, headers)["usage"]
        if self.on_usage is not None:
            cached = (u.get("prompt_tokens_details") or {}).get("cached_tokens") or 0
            self.on_usage({"input_tokens": u["prompt_tokens"] - cached, "cache_read_input_tokens": cached,
                           "output_tokens": u.get("completion_tokens", 0)})
        return u["prompt_tokens"]


# ── The tap ────────────────────────────────────────────────────────────────


class Tap:
    """A recording pass-through to `upstream` (module docs). `records` holds
    one dict per request, in the order they came: `{"path", "t_sent",
    "t_done", "status", "request": the body as JSON}`. Bodies are never
    changed; headers pass through but for the hop's own. A request is
    recorded when it comes, and `take` waits for those still streaming: a
    client has the last byte before the tap is done writing it."""

    HOP = {"host", "content-length", "connection", "transfer-encoding", "accept-encoding"}

    def __init__(self, upstream):
        self.upstream = upstream.rstrip("/")
        self.records, self.lock = [], threading.Lock()
        tap = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a):
                pass

            def do_GET(self):
                self._pass(None)

            def do_POST(self):
                self._pass(self.rfile.read(int(self.headers.get("content-length") or 0)))

            def _pass(self, raw):
                rec = {"path": self.path, "t_sent": time.monotonic(), "t_done": None, "status": None}
                done = threading.Event()
                with tap.lock:
                    tap.records.append((rec, done))
                try:
                    self._forward(rec, raw)
                finally:
                    rec["t_done"] = time.monotonic()
                    done.set()

            def _forward(self, rec, raw):
                try:
                    rec["request"] = json.loads(raw) if raw else None
                except json.JSONDecodeError:
                    rec["request"] = None
                headers = {k: v for k, v in self.headers.items() if k.lower() not in tap.HOP}
                req = urllib.request.Request(tap.upstream + self.path, data=raw, method=self.command, headers=headers)
                try:
                    resp = urllib.request.urlopen(req, timeout=600)
                except urllib.error.HTTPError as e:
                    resp = e
                with resp:  # the upstream connection, closed once relayed
                    rec["status"] = resp.status if hasattr(resp, "status") else resp.code
                    self.send_response(rec["status"])
                    for k, v in resp.headers.items():
                        if k.lower() not in tap.HOP:
                            self.send_header(k, v)
                    self.send_header("transfer-encoding", "chunked")
                    self.end_headers()
                    while True:
                        chunk = resp.read1(65536) if hasattr(resp, "read1") else resp.read(65536)
                        if not chunk:
                            break
                        self.wfile.write(b"%x\r\n%s\r\n" % (len(chunk), chunk))
                        self.wfile.flush()
                    self.wfile.write(b"0\r\n\r\n")
                    self.wfile.flush()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server.server_address[1]}"

    def take(self, wait=30):
        """The records so far, each finished (or given up on after `wait`
        seconds, its `t_done` None), and forget them."""
        with self.lock:
            out, self.records = self.records, []
        for _, done in out:
            done.wait(wait)
        return [rec for rec, _ in out]

    def close(self):
        self.server.shutdown()
        self.server.server_close()
