"""Turns and rounds (docs/blocks.md §2.8), read from one session's events on
the daemon's session wire (SynapsCLI `0a71ea0d`, `crates/agent-engine/src/
session/wire.rs` 869-1030: `WireSessionEvent`, `WireLlmEvent`,
`WireSessionStreamEvent`, `WireAgentEvent`).

**A turn** is one run from its `turn_started` to the session's `idle`,
steers included. It is **complete** only at its `idle`: a `session: done`
with no `idle` after it (the stream cut off) leaves it `incomplete`. Its
outcome is `done` at the `idle` unless an `error` or `aborted` came first.
Its `trigger` (`session/types.rs` 1631-1639) says who started it: `user`
is a user's message, and `queued_auto` is the user's own steer that missed
the last drain and started the next turn (`actor.rs` 3391-3395). Both are
user-initiated and count. `event_auto`, `plugin_command`, `compaction` and
`driver_auto` are event-started turns: reported apart, outside every
denominator.

**A round** is one provider response: one `session: usage` event. The
engine sends exactly one per request (`runtime/api.rs` 3444, "THE
invariant"), so a round is counted when its usage comes. Its calls and text
are the `llm` events since its `response_start`; a `response_reset`
(a retried stream) drops what the round had drawn, since the retry streams
it again. **A response whose usage never came** (a stream that failed, was
cut, or was retried into an error) is no round: it would count as zero
tokens. It is kept apart, in the turn's `unmeasured`, and a turn with one
isn't `measured`: the report keeps it out of an eligible run (report.py).

**A call** comes in either of the wire's two forms (`wire.rs` 980-982):
`tool_use_start` (its name and id) then `tool_use_delta`s (its arguments as
text, in pieces), or a whole `tool_use` with its parsed `input`. A provider
may send either or both: an OpenAI-compatible stream sends only the first.
A call is one per id; its input is the `tool_use`'s when one comes, else
its deltas joined and parsed (kept as the raw text if they don't parse).

Each turn keeps its **timeline**, in wire order: the user's messages (the
opening text, then each steer when the engine delivers it,
`agent: steering_delivered`), the rounds (placed where each began), and
the tool results. The closing-round classifier reads it (classify.py).
"""

import json
from dataclasses import dataclass, field

USER_TRIGGERS = ("user", "queued_auto")
EVENT_TRIGGERS = ("event_auto", "plugin_command", "compaction", "driver_auto")
BLOCK_TOOL = "cards_block"


@dataclass
class Call:
    tool_id: str
    name: str
    input: object
    raw: str = ""  # the deltas so far, when the call came as a stream


@dataclass
class Round:
    index: int  # in its turn, from 0
    t_start: float | None = None
    t_end: float | None = None
    calls: list = field(default_factory=list)
    text: str = ""
    usage: dict | None = None
    resets: int = 0

    @property
    def input_side(self):
        """The reported input of the response: uncached input, cache reads
        and cache writes, all of it read by the provider."""
        u = self.usage or {}
        return u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0) + u.get("cache_creation_input_tokens", 0)

    @property
    def output(self):
        return (self.usage or {}).get("output_tokens", 0)

    def block_calls(self):
        return [c for c in self.calls if c.name == BLOCK_TOOL]


@dataclass
class Result:
    tool_id: str
    name: str | None
    text: str


@dataclass
class Turn:
    trigger: str
    user_text: str | None
    t_start: float | None = None
    t_end: float | None = None
    rounds: list = field(default_factory=list)
    # In wire order: ("user", text, is_steer), ("round", Round), ("result", Result).
    timeline: list = field(default_factory=list)
    steers: list = field(default_factory=list)  # (text, delivered) as `steered` reported them
    outcome: str = "incomplete"  # done, error, aborted (at the `idle`), incomplete (no `idle` seen)
    unmeasured: list = field(default_factory=list)  # responses whose usage never came (module docs)
    saw_done: bool = False  # a `session: done` came (diagnostic: the `idle` decides)

    @property
    def measured(self):
        """Complete (its `idle`, no error, not aborted), and every response
        it streamed has its usage."""
        return self.outcome == "done" and self.t_end is not None and not self.unmeasured

    @property
    def user_initiated(self):
        return self.trigger in USER_TRIGGERS

    def results(self):
        return [x[1] for x in self.timeline if x[0] == "result"]

    def calls(self):
        return [c for r in self.rounds for c in r.calls]

    def block_calls(self):
        return [c for r in self.rounds for c in r.block_calls()]


def turns(events):
    """The turns of one session's `events`: each a `(t, ev)` pair, `t` the
    harness's clock in seconds when the frame came and `ev` the frame's
    session event (`{"ev": …}`), in wire order. Events outside a turn are
    ignored."""
    out, turn, names, calls = [], None, {}, {}

    def close():
        nonlocal turn
        if turn is not None:
            # A response whose usage never came is no round (module docs).
            turn.unmeasured = [r for r in turn.rounds if r.usage is None]
            if turn.unmeasured:
                turn.rounds = [r for r in turn.rounds if r.usage is not None]
                turn.timeline = [x for x in turn.timeline if x[0] != "round" or x[1].usage is not None]
                for i, r in enumerate(turn.rounds):
                    r.index = i
            out.append(turn)
        turn = None

    for t, ev in events:
        kind = ev.get("ev")
        if kind == "turn_started":
            if turn is not None:
                close()  # a turn with no `idle`: incomplete
            turn = Turn(trigger=ev.get("trigger"), user_text=ev.get("user_text"), t_start=t)
            if turn.user_text is not None:
                turn.timeline.append(("user", turn.user_text, False))
            continue
        if turn is None:
            continue
        if kind == "idle":
            turn.t_end = t
            if turn.outcome == "incomplete":
                turn.outcome = "done"
            close()
            continue
        if kind == "aborted":
            turn.outcome = "aborted"
            continue
        if kind == "steered":
            turn.steers.append((ev.get("text"), bool(ev.get("delivered"))))
            continue
        if kind != "stream":
            continue
        s = ev.get("event") or {}
        if s.get("kind") == "llm":
            llm = s.get("llm")
            if llm == "response_start":
                _new_round(turn, t)
            elif llm == "response_reset":
                if turn.rounds and turn.rounds[-1].usage is None:
                    r = turn.rounds[-1]
                    for c in r.calls:
                        calls.pop(c.tool_id, None)
                    r.calls, r.text, r.resets = [], "", r.resets + 1
            elif llm == "text":
                _round(turn, t).text += s.get("text", "")
            elif llm in ("tool_use_start", "tool_use"):
                tid = s.get("tool_id")
                names[tid] = s.get("tool_name")
                call = calls.get(tid)
                if call is None:
                    call = calls[tid] = Call(tid, s.get("tool_name"), None)
                    _round(turn, t).calls.append(call)
                if llm == "tool_use":
                    call.input, call.raw = s.get("input"), None
            elif llm == "tool_use_delta":
                call = calls.get(s.get("tool_id"))
                if call is not None and call.raw is not None:
                    call.raw += s.get("delta", "")
                    try:
                        call.input = json.loads(call.raw)
                    except json.JSONDecodeError:
                        call.input = call.raw
            elif llm == "tool_result":
                turn.timeline.append(("result", Result(s.get("tool_id"), names.get(s.get("tool_id")), s.get("result") or "")))
        elif s.get("kind") == "session":
            sess = s.get("session")
            if sess == "usage":
                # The latest round without its usage; a usage with none is
                # a request that streamed nothing: its own round.
                r = next((r for r in reversed(turn.rounds) if r.usage is None), None) or _new_round(turn, t)
                r.usage = {k: s.get(k) or 0 for k in ("input_tokens", "output_tokens", "cache_read_input_tokens",
                                                       "cache_creation_input_tokens")}
                r.usage["model"] = s.get("model")
                r.t_end = t
            elif sess == "done":
                turn.saw_done = True
            elif sess == "error":
                turn.outcome = "error"
        elif s.get("kind") == "agent" and s.get("agent") == "steering_delivered":
            turn.timeline.append(("user", s.get("message", ""), True))
    close()
    return out


def _round(turn, t):
    """The round streaming now: the last one begun, if nothing (a result, a
    user's message) has come since it began; else a new one, for a provider
    that sends no `response_start`."""
    if turn.timeline and turn.timeline[-1][0] == "round":
        return turn.timeline[-1][1]
    return _new_round(turn, t)


def _new_round(turn, t):
    r = Round(index=len(turn.rounds), t_start=t)
    turn.rounds.append(r)
    turn.timeline.append(("round", r))
    return r


def calls(events):
    """Every call of `events`, turns or not, in order: `(Call, result text or
    None)`. The harness's view of what the agent did (the card it showed,
    the block it put)."""
    out, by, results = [], {}, {}
    for _, ev in events:
        s = ev.get("event") or {}
        if s.get("kind") != "llm":
            continue
        llm, tid = s.get("llm"), s.get("tool_id")
        if llm in ("tool_use_start", "tool_use"):
            if tid not in by:
                by[tid] = Call(tid, s.get("tool_name"), None)
                out.append(by[tid])
            if llm == "tool_use":
                by[tid].input, by[tid].raw = s.get("input"), None
        elif llm == "tool_use_delta" and tid in by and by[tid].raw is not None:
            by[tid].raw += s.get("delta", "")
            try:
                by[tid].input = json.loads(by[tid].raw)
            except json.JSONDecodeError:
                by[tid].input = by[tid].raw
        elif llm == "tool_result":
            results[tid] = s.get("result") or ""
    return [(c, results.get(c.tool_id)) for c in out]
