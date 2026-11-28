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
Nor is **a turn the provider never answered** (no response at all, from
its `turn_started` to its `idle`): every user-initiated turn is a request
to the provider, so one with no round measured nothing, and would pass
for a turn that cost nothing (review #14 round 2, R2-2).

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

**Single-agent** (docs/blocks.md §2.8; review #14 round 4, R4-1). The
agent works alone, in the pilot too: a worker's responses can't be
measured here. A reactive worker's (`subagent_start`, then
`subagent_collect`) usage stays in its own state, never on the session's
stream (`tools/subagent/drive.rs` 114-121; `collect.rs` 194-202 returns
it). A one-shot's (`subagent`) is one aggregate usage after all of its
responses, between its `agent: subagent_start` and `subagent_done`
(`oneshot.rs` 136, 348-359, 470-486), and none at all if it fails
(503-530). So the eval's daemon offers none of `DELEGATION_TOOLS`, the
engine's eight (`tools/registry.rs` 52-59; daemon.py disables them). In
a turn, a usage inside a one-shot's span is the worker's: kept apart in
`worker_usage`, never a round. Outside one, a usage with no response begun
is still its own round (a request that streamed nothing).

`not_single_agent(events, requests)` says why one run's record isn't
single-agent with every paid request measured, empty when it is: a
delegation tool offered by any of its requests, or called; a worker's
event (`subagent_start`, `_update`, `_done`, or `subagent_rows` with a
row); a worker's usage; a tool result in a worker's report form (a JSON
object with `handle_id` and `usage`, whatever call it answers); its
requests not recorded. And any **paid request** (`paid`: a provider
request with `messages`, answered 2xx or not yet answered) without its
usage on the session's stream. The engine sends exactly one usage per
request (above), so the counts are equal unless something paid isn't on
the stream: a worker's request, an attempt the engine retried mid-stream
(`response_reset`, `runtime/api.rs` 2705-2722, 2784), or a usage whose
request the tap never saw. Either way, what the run cost isn't all
measured, and the report keeps it out of an eligible run (report.py).
"""

import collections
import json
from dataclasses import dataclass, field

USER_TRIGGERS = ("user", "queued_auto")
EVENT_TRIGGERS = ("event_auto", "plugin_command", "compaction", "driver_auto")
BLOCK_TOOL = "cards_block"
# The engine's delegation tools (SynapsCLI `0a71ea0d`, `tools/registry.rs`
# 52-59): every way to start, drive or read a worker (module docs).
DELEGATION_TOOLS = ("subagent", "subagent_model_authorize", "subagent_models", "subagent_start", "subagent_status",
                    "subagent_steer", "subagent_collect", "subagent_resume")
WORKER_EVENTS = ("subagent_start", "subagent_update", "subagent_done")  # `wire.rs` 1021-1026
USAGE = ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")


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

    @property
    def block_only(self):
        """A response whose calls are all block calls (classify.py's "A tail
        metric"), wherever it falls in the turn."""
        return bool(self.calls) and all(c.name == BLOCK_TOOL for c in self.calls)


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
    worker_usage: list = field(default_factory=list)  # a one-shot worker's aggregate usages: never rounds (module docs)
    saw_done: bool = False  # a `session: done` came (diagnostic: the `idle` decides)

    @property
    def measured(self):
        """Complete (its `idle`, no error, not aborted), answered (at least
        one response), and every response it streamed has its usage."""
        return self.outcome == "done" and self.t_end is not None and bool(self.rounds) and not self.unmeasured

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
    out, turn, names, calls, workers = [], None, {}, {}, set()

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
            turn, workers = Turn(trigger=ev.get("trigger"), user_text=ev.get("user_text"), t_start=t), set()
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
            if sess == "usage" and workers:  # a one-shot worker's aggregate (module docs, "Single-agent")
                turn.worker_usage.append({k: s.get(k) or 0 for k in USAGE} | {"model": s.get("model")})
            elif sess == "usage":
                # The latest round without its usage; a usage with none is
                # a request that streamed nothing: its own round.
                r = next((r for r in reversed(turn.rounds) if r.usage is None), None) or _new_round(turn, t)
                r.usage = {k: s.get(k) or 0 for k in USAGE}
                r.usage["model"] = s.get("model")
                r.t_end = t
            elif sess == "done":
                turn.saw_done = True
            elif sess == "error":
                turn.outcome = "error"
        elif s.get("kind") == "agent" and s.get("agent") == "steering_delivered":
            turn.timeline.append(("user", s.get("message", ""), True))
        elif s.get("kind") == "agent" and s.get("agent") == "subagent_start":
            workers.add(s.get("subagent_id"))
        elif s.get("kind") == "agent" and s.get("agent") == "subagent_done":
            workers.discard(s.get("subagent_id"))
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


def paid(requests):
    """The paid provider requests of one run's `requests` (module docs,
    "Single-agent"): each with `messages`, answered 2xx or not yet."""
    return [q for q in requests or [] if isinstance(q.get("request"), dict) and "messages" in q["request"]
            and (q.get("status") is None or 200 <= q["status"] < 300)]


def offered(requests):
    """The tool names the paid requests offered the model, in either
    provider's form (Anthropic's `name`, a chat `function`'s)."""
    names = set()
    for q in paid(requests):
        for t in q["request"].get("tools") or []:
            names.add((t.get("function") or {}).get("name") if t.get("type") == "function" else t.get("name"))
    return sorted(n for n in names if n)


def _worker_report(text):
    """Whether a tool result is a worker's report (`collect.rs` 194-202):
    a JSON object with `handle_id` and `usage`."""
    try:
        v = json.loads(text) if text else None
    except json.JSONDecodeError:
        return False
    return isinstance(v, dict) and "handle_id" in v and "usage" in v


def not_single_agent(events, requests):
    """Why one run's record isn't single-agent with every paid request
    measured (module docs, "Single-agent"): empty when it is. `requests`
    is the record's (None if it has none)."""
    if requests is None:
        return ["its provider requests weren't recorded: what it was offered and paid is unknown"]
    why = []
    d = [n for n in offered(requests) if n in DELEGATION_TOOLS]
    if d:
        why.append(f"delegation offered: {', '.join(d)}")
    cs = calls(events)
    called = collections.Counter(c.name for c, _ in cs if c.name in DELEGATION_TOOLS)
    if called:
        why.append("delegation called: " + ", ".join(f"{n} ×{k}" for n, k in sorted(called.items())))
    seen = collections.Counter()
    for _, ev in events:
        s = ev.get("event") or {}
        if ev.get("ev") == "stream" and s.get("kind") == "agent" and s.get("agent") in WORKER_EVENTS:
            seen[s["agent"]] += 1
        elif ev.get("ev") == "subagent_rows" and ev.get("rows"):
            seen["subagent_rows"] += 1
    if seen:
        why.append("a worker's events: " + ", ".join(f"{n} ×{k}" for n, k in sorted(seen.items())))
    wu = [u for x in turns(events) for u in x.worker_usage]
    if wu:
        why.append(f"a worker's usage on the stream, never a round: {len(wu)}, {sum(u['output_tokens'] for u in wu)} output tokens")
    reports = sorted({c.name or "?" for c, r in cs if _worker_report(r)})
    if reports:
        why.append(f"a worker's report in a tool result ({', '.join(reports)})")
    n = len(paid(requests))
    usages = sum(1 for _, ev in events if ev.get("ev") == "stream" and (ev.get("event") or {}).get("session") == "usage")
    if n != usages:
        why.append(f"{n} paid provider requests, {usages} usages on the session's stream: not every request is measured")
    return why
