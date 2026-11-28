"""Builders for session-wire events (SynapsCLI `session/wire.rs`), in the
exact shapes the daemon sends, for the tests."""


class Wire:
    """One session's events, `(t, ev)`, with a clock that ticks per event."""

    def __init__(self, form="whole"):
        """`form`: how calls come (turns.py): `whole` (one `tool_use`),
        `streamed` (`tool_use_start`, then the arguments in `tool_use_delta`
        pieces, as an OpenAI-compatible stream sends them) or `both`."""
        self.events, self.t, self.n, self.form = [], 0.0, 0, form

    def ev(self, ev):
        self.t += 0.1
        self.events.append((round(self.t, 3), ev))
        return self

    def stream(self, kind, tag, **kw):
        return self.ev({"ev": "stream", "event": {"kind": kind, kind: tag, **kw}})

    def start(self, text="Fix it.", trigger="user"):
        return self.ev({"ev": "turn_started", "turn_baseline": 0, "trigger": trigger, "user_text": text})

    def usage(self, inp=1000, out=50, read=0, write=0, model="m"):
        return self.stream("session", "usage", input_tokens=inp, output_tokens=out, cache_read_input_tokens=read,
                           cache_creation_input_tokens=write, cache_creation_5m=None, cache_creation_1h=None, model=model)

    def round(self, calls=(), text="", usage=None, start=True):
        """One response: `calls` are (name, input) pairs. The tool ids."""
        if start:
            self.stream("llm", "response_start")
        if text:
            self.stream("llm", "text", text=text)
        ids = []
        for name, inp in calls:
            self.n += 1
            ids.append(f"toolu_{self.n:03d}")
            self.call(name, ids[-1], inp)
        self.usage(**(usage or {}))
        return ids

    def call(self, name, tool_id, inp):
        if self.form in ("streamed", "both"):
            import json

            self.stream("llm", "tool_use_start", tool_name=name, tool_id=tool_id)
            raw = json.dumps(inp)
            for i in range(0, len(raw), 7):  # in pieces, as a stream cuts it
                self.stream("llm", "tool_use_delta", tool_id=tool_id, delta=raw[i:i + 7])
        if self.form in ("whole", "both"):
            self.stream("llm", "tool_use", tool_name=name, tool_id=tool_id, input=inp)

    def results(self, ids, text="ok"):
        for i in ids:
            self.stream("llm", "tool_result", tool_id=i, result=text if isinstance(text, str) else text[i])
        return self

    def steer(self, text):
        self.ev({"ev": "steered", "text": text, "delivered": True})
        return self.stream("agent", "steering_delivered", message=text)

    def done(self):
        self.stream("session", "done")
        return self.ev({"ev": "idle"})


BLOCK = {"ops": [{"op": "patch", "id": "plan", "state": {"fix": "done"}}]}
PUT = {"ops": [{"op": "put", "id": "plan", "kind": "plan", "title": "Fix", "body": {"steps": [{"id": "fix", "label": "Fix"}]}}]}
