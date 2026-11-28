"""The harness's own parts, offline: the run order, the turns a task's steps
make, the desk seam (the stand-in, and a real run's refusal of it), the
providers' request shapes and the tap, spend and the budget guard, and the
report's gate (pooled and 2 of 3 repeats)."""

import argparse
import contextlib
import io
import json
import tempfile
import threading
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import run
from b5 import calibrate, count, daemon, desk, estimate, harness, providers, report, spend, taskset, turns
from b5.calibrate import compact
from wire import BLOCK, PUT, Wire

FIXTURES = Path(__file__).resolve().parent / "fixtures"

TASKS = taskset.load()


class Plan(unittest.TestCase):
    def test_both_arms_of_each_task_back_to_back_three_times_alternating(self):
        order, not_run = harness.plan(TASKS, 3)
        runnable = [t for t in TASKS if not t["needs"]]
        self.assertEqual(len(order), len(runnable) * 2 * 3)
        self.assertEqual({t["id"] for t in not_run}, {"shelf-handback-plan", "ledger-handback-diff", "ledger-handback-note"})
        for k in range(0, len(order), 2):
            (r1, t1, a1), (r2, t2, a2) = order[k], order[k + 1]
            self.assertEqual((r1, t1["id"]), (r2, t2["id"]))
            self.assertEqual({a1, a2}, {"on", "off"})
        firsts = [order[k][2] for k in range(0, len(order), 2)]
        n = len(runnable)
        self.assertEqual(firsts[:n].count("on"), (n + 1) // 2, "the arm that goes first alternates by task")
        self.assertNotEqual(firsts[:n], firsts[n:2 * n], "and by repeat")

    def test_steers_and_dismissals_belong_to_the_turn_before_them(self):
        g = harness.groups([{"say": "a"}, {"dismiss": "first_block", "after_round": 2}, {"steer": "b", "after_round": 3},
                            {"answer": {}}, {"steer": "c", "after_round": 1}])
        self.assertEqual([[next(iter(s)) for s in x] for x in g], [["say", "dismiss", "steer"], ["answer", "steer"]])


class Calls(unittest.TestCase):
    def test_the_card_and_the_block_the_harness_acts_on(self):
        for form in ("whole", "streamed"):
            w = Wire(form).start()
            ids = w.round([("cards_block", PUT), ("cards_ui", {"question": "Q?"})])
            w.results(ids, {ids[0]: "ok 1 ops (blocks/1 #0)", ids[1]: "Card cabc123 is showing (cards/1 #1): Q?"})
            w.round([("cards_block", BLOCK)])
            w.done()
            self.assertEqual(harness._last_card(w.events), ({"question": "Q?"}, "Card cabc123 is showing (cards/1 #1): Q?"), form)

    def test_a_refused_card_is_not_one_to_answer(self):
        w = Wire().start()
        w.results(w.round([("cards_ui", {"question": ""})]), "Tool execution failed: cards_ui: question: required")
        self.assertIsNone(harness._last_card(w.done().events))

    def test_a_card_closed_since_is_not_one_to_answer(self):
        w = Wire().start()
        w.results(w.round([("cards_ui", {"question": "Q?"})]), "Card cabc123 is showing (cards/1 #1): Q?")
        w.results(w.round([("cards_ui", {"close": "cabc123"})]), "Card cabc123 closed (cards/1 #2)")
        self.assertIsNone(harness._last_card(w.done().events))
        w = Wire().start()
        w.results(w.round([("cards_ui", {"question": "Q?"})]), "Card cabc123 is showing (cards/1 #1): Q?")
        w.results(w.round([("cards_ui", {"close": "cother99"})]), "Card cother99 closed (cards/1 #2)")
        self.assertIsNotNone(harness._last_card(w.done().events), "another card's close leaves it showing")


class CardAnswer(unittest.TestCase):
    """An answer the policy can't give (cards-eval: no option is the pick,
    or two are) is an interaction failure: nothing is sent, and the step
    and the report say so."""

    def test_an_interaction_failure_sends_nothing_and_is_reported(self):
        asked, orig = [], harness.helper
        harness.helper = lambda cards_eval, cmd, payload: asked.append(payload) or {"error": "no option is ['Hide them']", "interaction": True}
        try:
            w = Wire().start()
            w.results(w.round([("cards_ui", {"question": "Zeros?", "options": ["Omit them", "Keep them"]})]),
                      "Card cabc123 is showing (cards/1 #1): Zeros?")
            s = _Session([])
            s.events = w.done().events
            rec = {"steps": [], "sent": []}
            left = harness.Runner(_Iso([]), "m", _Desk(), Path("/nonexistent")).turn(s, "j1", {"answer": {"pick": ["Hide them"]}}, [], rec)
        finally:
            harness.helper = orig
        self.assertEqual((left, rec["sent"]), ([], []))
        self.assertEqual(asked[0]["pick"], ["Hide them"])
        self.assertEqual(rec["steps"], [{"step": "answer", "skipped": "no option is ['Hide them']", "interaction": True}])
        r = dict(task="t", category="interactive", arm="on", repeat=1, events=[], fold={"lines": []}, judge={"pass": False}, **rec)
        self.assertEqual(report.build([r], Chars(), 900, [], {"mode": "test"})["also"]["interaction_failures"],
                         [{"task": "t", "arm": "on", "repeat": 1, "why": "no option is ['Hide them']"}])


class HandBack(unittest.TestCase):
    """The hand-back, both arms (taskset.py's `hand_back`): on, the desk
    takes the agent's live block of the kind; with none, or off, the user
    sends the plain message."""

    HB = {"block": {"kind": "plan"}, "edit": {"append_step": "Also add `queue`."}, "say": "Go ahead.", "message": "Go ahead, and add `queue`."}

    def setUp(self):
        self.helper, self.boards = harness.helper, []
        harness.helper = lambda cards_eval, cmd, payload: self.boards.pop(0)
        self.grace, harness.QUEUED_GRACE = harness.QUEUED_GRACE, 0

    def tearDown(self):
        harness.helper, harness.QUEUED_GRACE = self.helper, self.grace

    def turn(self, arm, starter, board=None):
        if board is not None:
            self.boards.append(board)
        turn = [{"ev": "turn_started", "trigger": "user", "user_text": "x"}, {"ev": "idle"}]
        s, d = _Session([turn]), _Desk()
        sent, hands = [], []
        d.send = lambda conv, carry, body, history: sent.append(body) or {"text": body, "notices": []}
        d.hand_back = lambda conv, spec, history: hands.append(spec) or {"text": "Go ahead.\n[edited]", "notices": []}
        r = harness.Runner(_Iso([]), "m", d, Path("/nonexistent"))
        r.arm = arm
        rec = {"steps": [], "sent": []}
        r.turn(s, "j1", starter, [], rec)
        return sent, hands, rec

    def test_on_the_desk_hands_back_the_latest_live_block_of_the_kind(self):
        board = {"blocks": [{"id": "old", "kind": "plan", "dismissed": False, "pending": False},
                            {"id": "notes", "kind": "note", "dismissed": False, "pending": False},
                            {"id": "plan2", "kind": "plan", "dismissed": False, "pending": False}]}
        sent, hands, rec = self.turn("on", {"hand_back": self.HB}, board)
        self.assertEqual((sent, hands), ([], [{"block": {"id": "plan2"}, "edit": self.HB["edit"], "say": "Go ahead."}]))
        self.assertEqual(rec["steps"], [{"step": "hand_back", "as": "block", "block": "plan2"}])

    def test_on_with_no_such_block_the_plain_message_goes_instead(self):
        sent, hands, rec = self.turn("on", {"hand_back": self.HB}, {"blocks": [{"id": "n", "kind": "note", "dismissed": False, "pending": False}]})
        self.assertEqual((sent, hands), (["Go ahead, and add `queue`."], []))
        self.assertEqual(rec["steps"], [{"step": "hand_back", "as": "message", "said_instead": "no live plan block"}])

    def test_off_the_opening_asks_for_the_reply_and_the_edit_is_the_message(self):
        sent, _, rec = self.turn("off", {"say": "Plan it in a plan block.", "say_off": "Plan it."})
        self.assertEqual(sent, ["Plan it."])
        sent, hands, rec = self.turn("off", {"hand_back": self.HB})
        self.assertEqual((sent, hands, rec["steps"]), (["Go ahead, and add `queue`."], [], [{"step": "hand_back", "as": "message"}]))
        sent, _, _ = self.turn("on", {"say": "Plan it in a plan block.", "say_off": "Plan it."})
        self.assertEqual(sent, ["Plan it in a plan block."])

    def test_the_stand_in_hands_back_for_the_dry_run(self):
        out = desk.StandIn().hand_back("c1", {"block": {"id": "plan"}, "edit": {"keep": "all"}, "say": "Apply it."}, [])
        self.assertTrue(out["text"].startswith("Apply it.\n[stand-in hand-back of block plan: "))


class FinalText(unittest.TestCase):
    def test_the_answer_is_the_last_response_with_text_not_the_narration(self):
        # A tool-using question: "I'll run the tests." rides the call, the
        # answer comes alone. The judge reads the answer (taskset.py).
        w = Wire().start("How many tests ran? Just the number.")
        w.results(w.round([("bash", {"command": "python3 -m unittest"})], text="I'll run the tests."))
        w.round(text="12")
        w.round([("cards_block", BLOCK)])  # a closing call with no text
        events = w.done().events
        self.assertEqual(harness.final_text(events), "12")
        task = next(t for t in TASKS if t["id"] == "ledger-q-tests")
        self.assertTrue(taskset.judge(task, text=harness.final_text(events))["pass"])
        self.assertEqual(harness.final_text(Wire().start("x").done().events), "")


class Record(unittest.TestCase):
    """Review #14, P3 13: the run's record names the harness, the binary,
    the adapter and the configuration; latency is two named intervals."""

    def test_the_desk_s_handshake_is_kept(self):
        class Adapter(desk.Desk):
            def request(self, req):
                return {"ok": True, "protocol": desk.PROTOCOL, "impl": "synaps-desk D6b", "commit": "0123abc"}

        d = desk.for_real_run(Adapter())
        self.assertEqual(d.handshake, {"ok": True, "protocol": desk.PROTOCOL, "impl": "synaps-desk D6b", "commit": "0123abc"})

    def test_a_binary_is_named_by_its_bytes_not_only_its_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "synaps"
            exe.write_text("#!/bin/sh\necho synaps 0.12.0\n")
            exe.chmod(0o755)
            one = daemon.binary(exe)
            exe.write_text("#!/bin/sh\necho synaps 0.12.0 \n")
            two = daemon.binary(exe)
        self.assertEqual((one["version"], two["version"]), ("synaps 0.12.0", "synaps 0.12.0"))
        self.assertNotEqual(one["sha256"], two["sha256"])

    def test_the_config_is_recorded_without_credentials(self):
        a = argparse.Namespace(dry_run=False, provider="openai_compatible", repeats=3, desk="/bin/true --x", proxy="o200k")
        prov = providers.OpenAICompatible("m/qwen", "http://gw:8000/v1", "sk-secret")
        d = desk.StandIn()
        d.hello()
        c = run.config(a, prov, d, endpoint=False)
        self.assertEqual((c["provider"], c["base_url"], c["model"], c["proxy"]), ("openai_compatible", "http://gw:8000/v1", "qwen", "o200k"))
        self.assertEqual(c["desk"]["handshake"]["impl"], desk.STAND_IN)
        self.assertEqual(c["desk"]["executable"]["path"], "/bin/true")
        self.assertIn("tiktoken", c)
        self.assertNotIn("sk-secret", json.dumps(c))

    def test_latency_is_the_tap_s_request_time_and_the_wire_s_interval_named_apart(self):
        recs = [{"task": "a", "arm": "on", "repeat": 1, "fold": {"lines": []}, "judge": {"pass": True},
                 "requests": [{"t_sent": 10.0, "t_done": 12.5}, {"t_sent": 20.0, "t_done": 21.0}, {"request": {}}]},
                {"task": "a", "arm": "off", "repeat": 1, "fold": {"lines": []}, "judge": {"pass": True}, "requests": []}]
        rows = {("a", "on"): [row("a", "on", 1)], ("a", "off"): [row("a", "off", 1)]}
        orig = report.per_turn
        report.per_turn = lambda rec, counter: (rows[(rec["task"], rec["arm"])], [])
        try:
            also = report.build(recs, _Counter(), 900, [], {"mode": "test"})["also"]
        finally:
            report.per_turn = orig
        self.assertNotIn("latency_per_round", also)
        self.assertEqual(also["request_latency_per_round"]["on"], {"median": 1.75, "p90": 2.5, "requests": 2})
        self.assertEqual(also["request_latency_per_round"]["off"]["requests"], 0)
        self.assertEqual(also["wire_interval_per_round"]["on"]["rounds"], 1)


class BlockOnly(unittest.TestCase):
    def test_a_turn_row_counts_its_block_only_rounds(self):
        w = Wire().start()
        w.results(w.round([("read", {})]))
        w.results(w.round([("cards_block", BLOCK)]))
        w.results(w.round([("bash", {}), ("cards_block", BLOCK)]))
        w.round(text="Fixed.")
        (r,), _ = report.per_turn({"task": "t", "arm": "on", "repeat": 1, "events": w.done().events, "fold": {"lines": []}}, Chars())
        self.assertEqual((r["block_only_rounds"], r["closing"]["rounds"]), (1, 0))

    def test_every_block_only_round_is_reported_beside_the_closing_ones(self):
        rows = [dict(row("a", "on", 1, closing=1), block_only_rounds=3), dict(row("b", "on", 1), block_only_rounds=0), row("a", "off", 1)]
        recs = [{"task": t, "arm": a, "repeat": 1, "fold": {"lines": []}, "judge": {"pass": True}} for t, a in (("a", "on"), ("b", "on"), ("a", "off"))]
        orig = report.per_turn
        report.per_turn = lambda rec, counter: ([r for r in rows if (r["task"], r["arm"]) == (rec["task"], rec["arm"])], [])
        try:
            also = report.build(recs, _Counter(), 900, [], {"mode": "test"})["also"]
        finally:
            report.per_turn = orig
        self.assertEqual(also["block_only_rounds"], {"rounds": 3, "closing": 1, "turns_with": 1, "turns": 2})


class _Desk(desk.StandIn):
    def __init__(self, refuse=None):
        super().__init__()
        self.clicked, self.refuse = [], refuse

    def dismiss(self, conversation, block, history):
        self.clicked.append(block)
        return {"ok": False, "refused": self.refuse} if self.refuse else super().dismiss(conversation, block, history)


class Dismissal(unittest.TestCase):
    """A dismissal clicks the board's first live block (harness.py's "A
    dismissal"): `cards-eval fold` of the history then, never the first
    put the agent tried."""

    def setUp(self):
        self.helper, self.boards = harness.helper, []
        harness.helper = lambda cards_eval, cmd, payload: self.boards.pop(0)

    def tearDown(self):
        harness.helper = self.helper

    def fire(self, board, d=None):
        # The agent's first put was refused; a later one, of `notes`, applied.
        w = Wire().start()
        w.results(w.round([("cards_block", PUT)]), "Tool execution failed: cards_block: ops[0]: …")
        note = {"ops": [{"op": "put", "id": "notes", "kind": "note", "body": {"text": "x"}}]}
        w.results(w.round([("cards_block", note)]), "ok 1 ops (blocks/1 #1)")
        s = _Session([])
        s.events = w.events
        d = d or _Desk()
        self.boards.append(board)
        rec = {"steps": []}
        harness.Runner(_Iso([]), "m", d, Path("/nonexistent")).fire(s, "j1", {"dismiss": "first_block", "after_round": 2}, rec, 2)
        return d, rec["steps"]

    def test_it_clicks_the_first_live_block_not_a_refused_put(self):
        d, steps = self.fire({"blocks": [{"id": "notes", "kind": "note", "dismissed": False, "pending": False}]})
        self.assertEqual(d.clicked, ["notes"])
        self.assertEqual((steps[0]["id"], steps[0]["outcome"]), ("notes", "accepted"))

    def test_a_removed_or_dismissed_block_isnt_clicked(self):
        d, steps = self.fire({"blocks": []})
        self.assertEqual(d.clicked, [], "the only block was removed: nothing to click")
        self.assertEqual((steps[0]["outcome"], steps[0]["skipped"]), ("skipped", "no live block"))
        d, steps = self.fire({"blocks": [{"id": "plan", "dismissed": True, "pending": False},
                                         {"id": "draft", "dismissed": False, "pending": True},
                                         {"id": "notes", "dismissed": False, "pending": False}]})
        self.assertEqual(d.clicked, ["notes"])

    def test_a_refused_click_is_recorded_as_refused(self):
        d, steps = self.fire({"blocks": [{"id": "notes", "dismissed": False, "pending": False}]}, _Desk(refuse="no such block"))
        self.assertEqual((d.clicked, steps[0]["outcome"]), (["notes"], "refused"))

    def test_a_probe_is_delivered_only_when_the_agent_received_its_notice(self):
        # Review #14 round 2, R2-3: what the harness sent is no evidence.
        line = "Also: keep it short.\n[block notes v3 dismissed]"
        def rec(steps, sent=(), history=None, arm="on"):
            return {"task": "t", "arm": arm, "steps": steps, "history": history,
                    "sent": [{"carry": "steer", "text": x, "notices": n, "ticket": 1} for x, n in sent]}
        def user(*texts):
            return [{"role": "user", "content": x} for x in texts]
        ok = [{"step": "dismiss", "id": "notes", "outcome": "accepted"}]
        three = [["notes", "dismissed", 3]]
        cases = {
            "delivered": rec(ok, [(line, three)], user("Fix it.", line)),
            # Shady's: the adapter returned the notice and a ticket, but the
            # message never reached the agent.
            "lost": rec(ok, [(line, three)], user("Fix it.")),
            # A different version in the text than the desk's.
            "other version": rec(ok, [(line.replace("v3", "v2"), three)], user("Fix it.", line.replace("v3", "v2"))),
            # A stale one: the history gave notes v3 before this message.
            "stale": rec(ok, [(line, three)], user("[block notes v3 dismissed]", line)),
            # Received in a message's text part, as an engine may merge it.
            "a part": rec(ok, [(line, three)], [{"role": "user", "content": [{"type": "tool_result", "content": "ok"},
                                                                              {"type": "text", "text": line}]}]),
            "never sent": rec(ok, [("Also: keep it short.", [])], user("Also: keep it short.")),
            "refused": rec([{"step": "dismiss", "id": "notes", "outcome": "refused"}]),
            "off": rec([{"step": "dismiss", "outcome": "skipped", "skipped": "no live block"}], arm="off"),
        }
        got = {k: report.notice_reached(r, r["steps"]) for k, r in cases.items() if k not in ("refused", "off")}
        self.assertEqual(got, {"delivered": "delivered", "lost": "sent", "other version": "sent", "stale": "sent",
                               "a part": "delivered", "never sent": "clicked"})
        self.assertEqual(report.notice_probes(list(cases.values())),
                         {"t": {"runs": 7, "clicked": 6, "sent": 5, "delivered": 2, "lost": 3, "refused": 1, "skipped": 0}})

    def test_a_send_the_history_doesnt_have_is_settled_as_lost(self):
        settled = []
        d = _Desk()
        d.settle = lambda conv, history, lost=(): settled.append(list(lost)) or {"ok": True}
        d.send = lambda conv, carry, body, history: {"text": body + "\n[block plan v1 dismissed]", "notices": [["plan", "dismissed", 1]], "ticket": 7}
        s = _Session([[{"ev": "turn_started", "trigger": "user", "user_text": "Q?"}, {"ev": "idle"}]])
        rec = {"steps": [], "sent": []}
        grace, harness.QUEUED_GRACE = harness.QUEUED_GRACE, 0
        try:
            harness.Runner(_Iso([]), "m", d, Path("/nonexistent")).turn(s, "j1", {"say": "Fix it."}, [], rec)
        finally:
            harness.QUEUED_GRACE = grace
        self.assertEqual((settled, rec["sent"][0].get("lost")), ([[7]], True), "the history has only Q? and A.")


class Received(unittest.TestCase):
    """What reached the agent (desk.py's docs): a user message of the
    history that is the text, whole or as one of its text parts."""

    def test_a_message_or_one_of_its_text_parts(self):
        notice = "[cards: b1 v2 dismissed]"
        history = [{"role": "user", "content": "Start."}, {"role": "assistant", "content": "Go on."},
                   {"role": "user", "content": [{"type": "text", "text": notice}, {"type": "text", "text": " Go on. "},
                                                {"type": "image", "source": {}}]},
                   {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "Keep going."}]}]
        self.assertEqual(desk.received_at(history, "Start."), 0)
        self.assertEqual(desk.received_at(history, "Go on."), 2, "one text part of a message of several")
        self.assertEqual(desk.received_at(history, notice + " Go on."), 2, "the message whole")
        self.assertIsNone(desk.received_at(history, "Go on"), "a part's prefix is no part")
        self.assertIsNone(desk.received_at(history, "Keep going."), "a tool result is no user message")
        self.assertIsNone(desk.received_at(history, "   "))
        self.assertFalse(desk.received(history[:2], "Go on."), "an assistant's text never reached the agent from the user")


class Seam(unittest.TestCase):
    def test_the_stand_in_rides_notices_as_d6b_sends_them(self):
        d = desk.StandIn()
        hist = [{"role": "user", "content": "Earlier.\n[block old v4 dismissed]"}]
        d.dismiss("c1", "plan", hist)
        d.request({"op": "show", "conversation": "c1", "id": "notes", "history": hist})
        self.assertEqual(d.send("c1", "answer", "[card cx] Q → A", hist)["text"], "[card cx] Q → A", "a card answer carries nothing")
        self.assertEqual(d.send("c1", "submit", "  ", hist)["notices"], [], "nothing rides an empty body")
        out = d.send("c1", "steer", "Go on.", hist)
        self.assertEqual(out["text"], "Go on.\n[block plan v5 dismissed]\n[block notes v6 shown]")
        for line in out["text"].split("\n")[1:]:
            self.assertRegex(line, desk.NOTICE)
        self.assertEqual(d.send("c1", "submit", "Next.", hist)["text"], "Next.", "each notice rides once")

    def test_a_real_run_refuses_the_stand_in_and_a_desk_without_d6b(self):
        with self.assertRaises(RuntimeError):
            desk.for_real_run(desk.StandIn())

        class Old(desk.Desk):
            def request(self, req):
                return {"ok": True, "protocol": desk.PROTOCOL, "impl": "synaps-desk D6a"}

        with self.assertRaises(RuntimeError):
            desk.for_real_run(Old())

        class New(Old):
            def request(self, req):
                return {"ok": True, "protocol": desk.PROTOCOL, "impl": "synaps-desk D6b+D7"}

        self.assertEqual(desk.for_real_run(New()).impl, "synaps-desk D6b+D7")


class Shapes(unittest.TestCase):
    BODY = {"system": "S", "tools": [{"name": "cards_block", "description": "d", "input_schema": {"type": "object"}}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "Probe."}]},
                         {"role": "assistant", "content": [{"type": "text", "text": "Working."},
                                                           {"type": "tool_use", "id": "t1", "name": "cards_block", "input": BLOCK}]},
                         {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"},
                                                      {"type": "text", "text": "Continue."}]}]}

    def test_chat_and_back_is_the_same_request(self):
        chat = providers.to_chat(self.BODY, "m")
        self.assertEqual([m["role"] for m in chat["messages"]], ["system", "user", "assistant", "tool", "user"])
        self.assertEqual(json.loads(chat["messages"][2]["tool_calls"][0]["function"]["arguments"]), BLOCK)
        self.assertEqual(providers.from_chat(chat), self.BODY)


class Tap(unittest.TestCase):
    def test_it_passes_a_request_through_unchanged_and_records_it(self):
        got = []

        class Up(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                got.append((self.path, self.rfile.read(int(self.headers["content-length"])), self.headers.get("x-api-key")))
                body = b'data: {"a": 1}\n\n'
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        up = ThreadingHTTPServer(("127.0.0.1", 0), Up)
        threading.Thread(target=up.serve_forever, daemon=True).start()
        tap = providers.Tap(f"http://127.0.0.1:{up.server_address[1]}")
        try:
            raw = json.dumps({"model": "m", "messages": [{"role": "user", "content": "hé"}]}, ensure_ascii=False).encode()
            req = urllib.request.Request(tap.url + "/v1/messages", data=raw, method="POST",
                                         headers={"content-type": "application/json", "x-api-key": "k"})
            with urllib.request.urlopen(req, timeout=10) as r:
                self.assertEqual(r.read(), b'data: {"a": 1}\n\n')
            self.assertEqual(got, [("/v1/messages", raw, "k")])
            (rec,) = tap.take()
            self.assertEqual((rec["path"], rec["status"], rec["request"]["messages"][0]["content"]), ("/v1/messages", 200, "hé"))
            self.assertLessEqual(rec["t_sent"], rec["t_done"])
            self.assertEqual(tap.take(), [])
        finally:
            tap.close()
            up.shutdown()
            up.server_close()


class Spend(unittest.TestCase):
    def test_list_prices_as_synapscli_bills_them(self):
        u = {"input_tokens": 1_000_000, "output_tokens": 1_000_000, "cache_read_input_tokens": 1_000_000,
             "cache_creation_input_tokens": 1_000_000}
        self.assertAlmostEqual(spend.cost("claude-sonnet-5", u), 2 + 10 + 0.2 + 2.5)
        self.assertAlmostEqual(spend.cost("anthropic/claude-opus-5-5-20261101", u), 4 + 20 + 0.2 + 5)
        self.assertAlmostEqual(spend.cost("claude-fable-5-2", u), 8 + 40 + 0.2 + 10)
        self.assertIsNone(spend.base("claude-sonnet-5-5x"))
        with self.assertRaises(KeyError):
            spend.Budget(10, "gpt-unknown")

    def test_the_guard_stops_before_a_run_that_could_pass_the_budget(self):
        b = spend.Budget(1.0, "claude-haiku-4-5")
        self.assertTrue(b.charge({"output_tokens": 100_000}))  # $0.50
        b.run_done()
        self.assertTrue(b.room_for_another(), "0.50 spent, 0.50 the largest run: another can reach 1.00, not pass it")
        self.assertTrue(b.charge({"output_tokens": 20_000}))  # $0.60
        b.run_done()
        self.assertFalse(b.room_for_another(), "0.60 spent, 0.50 the largest run: another could pass 1.00")
        self.assertFalse(b.charge({"output_tokens": 100_000}), "past the budget mid-turn")

    def test_every_charge_is_written_down_and_a_probe_past_the_budget_stops_the_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            ledger = Path(tmp) / "paid.jsonl"
            b = spend.Budget(1.0, "claude-haiku-4-5", ledger=ledger)
            probe = b.paid("probe")
            probe({"input_tokens": 400_000})  # $0.40
            probe({"input_tokens": 500_000})  # $0.90
            with self.assertRaises(spend.OverBudget):
                probe({"input_tokens": 200_000})  # $1.10: paid already, and the run stops
            lines = [json.loads(x) for x in ledger.read_text().splitlines()]
        self.assertEqual([x["what"] for x in lines], ["probe"] * 3)
        self.assertEqual([x["spent"] for x in lines], [0.4, 0.9, 1.1])

    def test_a_paid_probe_is_charged_before_its_count_comes_back(self):
        # A provider with no endpoint: the calibration's one-token completions
        # cost money, so a tiny budget stops it a few probes in.
        sent = []

        class Up(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                sent.append(json.loads(self.rfile.read(int(self.headers["content-length"]))))
                body = json.dumps({"usage": {"prompt_tokens": 250_000, "completion_tokens": 1,
                                             "prompt_tokens_details": {"cached_tokens": 50_000}}}).encode()
                self.send_response(200)
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        up = ThreadingHTTPServer(("127.0.0.1", 0), Up)
        threading.Thread(target=up.serve_forever, daemon=True).start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                b = spend.Budget(1.0, "claude-haiku-4-5", ledger=Path(tmp) / "paid.jsonl")
                prov = providers.OpenAICompatible("claude-haiku-4-5", f"http://127.0.0.1:{up.server_address[1]}", on_usage=b.paid("probe"))
                calls = [{"ops": [{"op": "remove", "id": "x" * (1 + k)}]} for k in range(210)]
                with self.assertRaises(spend.OverBudget):
                    calibrate.calibrate(calls, [], prov.count, len, "chars")
                paid = Path(tmp, "paid.jsonl").read_text().splitlines()
        finally:
            up.shutdown()
            up.server_close()
        # Each probe: 200k uncached + 50k cache read + 1 out = $0.205005; the 5th passes $1.
        self.assertEqual((len(sent), len(paid)), (5, 5))
        self.assertAlmostEqual(b.spent, 5 * (0.2 + 0.005 + 0.000005), places=9)
        self.assertEqual(sent[0]["max_tokens"], 1)

    def test_a_forced_call_is_read_and_paid_on_both_providers_and_the_scripted_model(self):
        # The output control's requests (calibrate.py): paid, with an
        # endpoint too, so each goes to the ledger before it returns.
        sent = []

        class Up(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                req = json.loads(self.rfile.read(int(self.headers["content-length"])))
                sent.append((self.path, req))
                if self.path == "/v1/messages":
                    body = {"model": "claude-haiku-4-5-20261001", "content": [{"type": "tool_use", "id": "t", "name": "cards_block",
                                                                              "input": {"ops": []}}],
                            "stop_reason": "tool_use", "usage": {"input_tokens": 200_000, "output_tokens": 40}}
                else:
                    body = {"model": "m", "choices": [{"message": {"tool_calls": [{"function": {"name": "cards_block", "arguments": '{"ops":[]}'}}]},
                                                       "finish_reason": "tool_calls"}],
                            "usage": {"prompt_tokens": 200_000, "completion_tokens": 40}}
                raw = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("content-length", str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)

        up = ThreadingHTTPServer(("127.0.0.1", 0), Up)
        threading.Thread(target=up.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{up.server_address[1]}"
        body = calibrate.forced({"ops": []}, [{"name": "cards_block", "description": "", "input_schema": {}}])
        try:
            with tempfile.TemporaryDirectory() as tmp:
                b = spend.Budget(1.0, "claude-haiku-4-5", ledger=Path(tmp) / "paid.jsonl")
                a = providers.Anthropic("claude-haiku-4-5", "k", base=base, on_usage=b.paid("probe"))
                o = providers.OpenAICompatible("claude-haiku-4-5", base, on_usage=b.paid("probe"))
                self.assertEqual(a.force(body), _called(40, {"ops": []}))
                self.assertEqual(o.force(body), _called(40, {"ops": []}) | {"stop": "tool_calls"})
                paid = [json.loads(x) for x in Path(tmp, "paid.jsonl").read_text().splitlines()]
        finally:
            up.shutdown()
            up.server_close()
        self.assertEqual([x["what"] for x in paid], ["probe", "probe"])
        self.assertAlmostEqual(b.spent, 2 * (0.2 + 40 * 5e-6), places=9)
        (pa, ra), (po, ro) = sent
        self.assertEqual((pa, ra["tool_choice"], ra["max_tokens"]), ("/v1/messages", {"type": "tool", "name": "cards_block", "disable_parallel_tool_use": True}, 4096))
        self.assertEqual((po, ro["tool_choice"], ro["parallel_tool_calls"], ro["stream"]),
                         ("/chat/completions", {"type": "function", "function": {"name": "cards_block"}}, False, False))
        self.assertEqual(a.models, {"claude-haiku-4-5-20261001"})
        # The dry run's scripted model calls it as asked, its output priced as any call's.
        from b5 import scripted
        m = scripted.Scripted()
        try:
            c = {"ops": [{"op": "patch", "id": "plan", "state": {"fix": "done"}}]}
            r = providers.OpenAICompatible("stub", m.url).force(calibrate.forced(c, []))
        finally:
            m.close()
        self.assertEqual(r, _called(scripted.tokens(json.dumps(c, ensure_ascii=False)) + 4, c) | {"stop": "tool_calls"})
        self.assertEqual(calibrate.alone(r), [], "one call and nothing else")

    def test_the_estimate_counts_the_whole_matrix(self):
        # B6 needs the D8 hand-backs too (report.required): the run to
        # approve is the whole matrix's.
        t = estimate.table(TASKS)
        e = t["rows"]["expected_warm"]
        self.assertEqual((e["tasks"], e["runs"], e["turns"]), (45, 270, 318))
        self.assertEqual(e["runs"], sum(report.required().values()))
        for m in spend.CANDIDATES:
            self.assertLess(e["usd"][m], t["rows"]["expected_cold"]["usd"][m])
            self.assertLess(t["rows"]["expected_cold"]["usd"][m], t["rows"]["long_cold"]["usd"][m])


class Estimate(unittest.TestCase):
    """Review #14, P3 12: scenarios, not a budget; the cache warm and cold;
    the context carried from turn to turn; said-instead steers; the paid
    probes apart."""

    def setUp(self):
        self.S = spend.Shape(rounds=2, out=100, tool_in=0, user_in=10)

    def test_a_cold_prefix_is_written_by_the_first_request(self):
        warm = spend.session_usage(1000, [self.S], warm=True)
        cold = spend.session_usage(1000, [self.S], warm=False)
        # Request 1 reads 1000 (warm) or writes 1010 (cold); request 2 reads 1010, writes 100.
        self.assertEqual((warm["cache_read_input_tokens"], warm["cache_creation_input_tokens"]), (2010, 110))
        self.assertEqual((cold["cache_read_input_tokens"], cold["cache_creation_input_tokens"]), (1010, 1110))

    def test_a_later_turn_starts_from_the_history_the_last_left(self):
        u = spend.session_usage(1000, [self.S, self.S], warm=True)
        # Turn 2's first request reads what turn 1's last one sent (1000 + 10 + 100) from the
        # cache, not the prefix alone; it writes turn 1's last reply and its own message.
        self.assertEqual(u["cache_read_input_tokens"], 2010 + 1110 + 1220)
        self.assertEqual(u["cache_creation_input_tokens"], 110 + 110 + 100)
        self.assertEqual(u["output_tokens"], 400)

    def test_steers_said_instead_add_turns_in_the_long_scenario(self):
        steered = next(t for t in TASKS if any("steer" in s for s in t["turns"]))
        self.assertEqual(spend.turn_kinds(steered), ["coding"])
        self.assertEqual(spend.turn_kinds(steered, steers_said_instead=True), ["coding", "steer"])
        card = next(t for t in TASKS if t["category"] == "interactive" and not t["needs"])
        self.assertEqual(spend.turn_kinds(card), ["card", "coding"])
        t = estimate.table(TASKS)
        self.assertEqual(t["rows"]["long_cold"]["turns"] - t["rows"]["expected_cold"]["turns"], 4 * 2 * 3)

    def test_the_table_names_scenarios_and_the_probes_not_a_budget(self):
        t = estimate.table(TASKS)
        self.assertNotIn("the budget)", t["markdown"])
        for part in ("Expected, warm prefix", "Expected, cold prefix", "Long scenario", "Paid probes", "not a cap", "+15%"):
            self.assertIn(part, t["markdown"])
        for m in spend.CANDIDATES:
            self.assertGreater(t["control_usd"][m], 0, "the output control is paid, with an endpoint too")
            self.assertGreater(t["probes_usd"][m], t["control_usd"][m])

    def test_the_paid_probes_are_the_requests_calibrate_makes(self):
        # Review #14 round 2, R2-8: the estimate priced 288 × 2 proxy probes;
        # the calibration makes 288 + 3 (a shared baseline, three times).
        # Count what the procedures actually send, and pin the estimate to it.
        calls, _ = calibrate.load()
        n = {"count": 0, "force": 0}

        def count(body):
            n["count"] += 1
            return 100 + len(json.dumps(body))

        def force(body):
            n["force"] += 1
            return _called(10, _forced_input(body))
        f, raw = calibrate.calibrate(calls, [], count, len, "chars")
        calibrate.output_control(calls, [], force, len)
        self.assertEqual((n["count"], raw["requests"], calibrate.proxy_requests(len(calls))), (291, 291, 291))
        self.assertEqual(n["force"], 16)
        self.assertEqual(estimate.probes(calls)["requests"], n["force"])
        self.assertEqual(estimate.probes(calls, endpoint=False)["requests"], n["count"] + 2 + n["force"], "and the definition control's 2")
        t = estimate.table(TASKS)
        self.assertEqual((t["probes_usd"]["claude-sonnet-5-5"], t["control_usd"]["claude-sonnet-5-5"]), (1.56, 0.06))

    def test_the_budget_at_the_margin_covers_the_long_scenario_by_15_percent(self):
        t = estimate.table(TASKS)
        for m in spend.CANDIDATES:
            need = (t["rows"]["long_cold"]["usd"][m] + t["control_usd"][m]) * 1.15
            self.assertGreaterEqual(t["margin_usd"][m], need)
            self.assertLess(t["margin_usd"][m] - 5, need)
            self.assertEqual(t["margin_usd"][m] % 5, 0)
        self.assertEqual(t["margin_usd"]["claude-sonnet-5-5"], 70, "the D8 tasks in, $60 leaves 3%")


def row(task, arm, repeat, out=100, block=0, per_round=(), closing=0, calls=1, ops=4, skipped=0, repairs=0, reputs=0, rounds=6,
        tool_calls=3):
    a = {"block_calls": calls, "block_output": block, "block_output_per_round": list(per_round), "result_input": 0,
         "unseen_ops": 0, "unseen_tokens": 0} if arm == "on" else None
    return {"task": task, "arm": arm, "repeat": repeat, "output": out, "attributed": a, "rounds": rounds, "tool_calls": tool_calls,
            "closing": {"k": closing + 1, "rounds": closing, "input": 0, "complete": True},
            "ops": ops if arm == "on" else 0, "skipped": skipped, "repairs": repairs, "reputs": reputs, "refusals": repairs,
            "skipped_by_reason": {}, "skipped_paths_by_reason": {}, "latency": [0.1], "usage": {"output_tokens": out}}


class Chars:
    """A counter for the tests: a token per character, no framing."""
    kind = "chars"
    result_framing = 0

    def call(self, x):
        return len(compact(x))

    def text(self, s):
        return len(s)

    def describe(self):
        return {"kind": self.kind}


def events_of(history):
    """The session wire of one turn that made `history` (a stored history):
    each assistant message a round with its calls under their own ids,
    each later user text a steer the engine delivered."""
    w = Wire()
    msgs = iter(history)
    w.start(next(msgs)["content"])
    for m in msgs:
        parts = m["content"] if isinstance(m["content"], list) else [{"type": "text", "text": m["content"]}]
        if m["role"] == "assistant":
            w.stream("llm", "response_start")
            for p in parts:
                if p["type"] == "text":
                    w.stream("llm", "text", text=p["text"])
                elif p["type"] == "tool_use":
                    w.call(p["name"], p["id"], p["input"])
            w.usage()
        elif any(p["type"] == "tool_result" for p in parts):
            w.results([p["tool_use_id"] for p in parts], {p["tool_use_id"]: p["content"] for p in parts})
        else:
            w.steer(parts[0]["text"])
    return w.done().events


class Ops(unittest.TestCase):
    """An op is one entry of a call's `ops`, never a log line (count.py's
    `ops`). The fixture is cards-eval's fold of its history, exactly (the
    Rust test `the_eval_op_fixture_is_this_folds_report`): ten ops, twenty
    lines. Eight patches applied with a bad path each (one with two), one
    skipped whole (its only path bad), the last four after the block's
    dismissal."""

    def run_of(self):
        history = json.loads((FIXTURES / "ops-history.json").read_text())
        fold = json.loads((FIXTURES / "ops-fold.json").read_text())
        rec = {"task": "t", "arm": "on", "repeat": 1, "events": events_of(history), "fold": fold}
        (row_,), _ = report.per_turn(rec, Chars())
        return row_, history, fold

    def test_an_op_counts_once_however_many_lines_the_board_logged(self):
        r, _, fold = self.run_of()
        self.assertEqual(len([ln for ln in fold["lines"] if ln["author"] == "agent"]), 20)
        self.assertEqual((r["ops"], r["skipped"]), (10, 1))
        self.assertEqual(r["partly_skipped"], 8)
        self.assertEqual(r["skipped_by_reason"], {"Paths": 1})
        self.assertEqual(r["skipped_paths_by_reason"], {"NotThere": 10}, "path diagnostics: their own table")

    def test_path_diagnostics_dont_dilute_the_skip_gate(self):
        # 1 op of 10 skipped is 10%, over the 5% gate. Read as lines it was
        # 1 of 20, 5%, a pass: more bad paths made the gate look better.
        r, _, _ = self.run_of()
        g = report.rows_of([r], [], 900)["skipped_ops"]
        self.assertEqual((g["skipped"], g["ops"], g["share"], g["pass"]), (1, 10, 0.1, False))

    def test_an_unseen_op_is_charged_once(self):
        r, history, _ = self.run_of()
        late = [p["input"]["ops"][0] for m in history[-9:] if m["role"] == "assistant" for p in m["content"] if p["type"] == "tool_use"]
        self.assertEqual(len(late), 4)
        self.assertEqual(r["attributed"]["unseen_ops"], 4)
        self.assertEqual(r["attributed"]["unseen_tokens"], sum(len(compact(o)) for o in late))

    def test_a_remove_of_several_ids_is_one_op(self):
        lines = [{"tool_id": "t1", "event": 3, "op": 0, "author": "agent", "did": "remove", "outcome": o, "why": w, "dismissed_before": False}
                 for o, w in (("applied", None), ("skipped", "CardOpen"), ("no_change", None))]
        lines += [{"tool_id": "t1", "event": 3, "op": 1, "author": "agent", "did": "remove", "outcome": "skipped", "why": w,
                   "dismissed_before": False} for w in ("CardOpen", "NoBlock")]
        a, b = count.ops(lines, {"t1"})
        self.assertEqual((a["outcome"], a["partly"], a["why"]), ("applied", True, None))
        self.assertEqual((b["outcome"], b["partly"], b["why"]), ("skipped", False, "CardOpen, NoBlock"))


class _Session:
    """A session of the eval's daemon, offline: each message it's sent plays
    the next turn of `script` (session-wire events), one event a `next`."""

    def __init__(self, script):
        self.events, self.prompts, self.meta, self.id = [], [], {"journal_id": "j1"}, "s1"
        self.script, self.queue = list(script), []

    def submit(self, text):
        self.queue += self.script.pop(0)

    steer = submit

    def cancel(self):
        self.queue = [{"ev": "aborted", "context_saved": True}, {"ev": "idle"}]

    on_usage = None

    def next(self, timeout):
        if self.queue:
            ev = self.queue.pop(0)
            self.events.append((float(len(self.events)), ev))
            if self.on_usage is not None and (ev.get("event") or {}).get("session") == "usage":
                self.on_usage(ev["event"])  # as daemon.Session records it

    def until(self, pred, timeout):
        while self.queue:
            self.next(0)
            if pred(self.events[-1][1]):
                return self.events[-1][1]

    def messages(self):
        return [{"role": "user", "content": "Q?"}, {"role": "assistant", "content": "A."}]

    def close(self):
        pass


def _as(cls, obj):
    """`obj` as an instance of its subclass `cls`, its state kept."""
    obj.__class__ = cls
    return obj


class _Iso:
    cards_eval = "cards-eval"

    def __init__(self, outputs):
        self.outputs, self.sessions = list(outputs), []

    def blocks(self, on):
        return ["read"] + (["cards_block"] if on else [])

    def session(self, cwd, model, name):
        out = self.outputs.pop(0)
        turn = [{"ev": "turn_started", "trigger": "user", "user_text": "Q?"},
                {"ev": "stream", "event": {"kind": "llm", "llm": "response_start"}},
                {"ev": "stream", "event": {"kind": "llm", "llm": "text", "text": "21"}},
                {"ev": "stream", "event": {"kind": "session", "session": "usage", "input_tokens": 0, "output_tokens": out}},
                {"ev": "stream", "event": {"kind": "session", "session": "done"}}, {"ev": "idle"}]
        self.sessions.append(_Session([turn]))
        return self.sessions[-1]


class BudgetStop(unittest.TestCase):
    """A run the budget stops (spend.py's soft stop) keeps every run that
    spent, the stopped one marked, in the records and in records.jsonl."""

    def setUp(self):
        self.grace, self.helper = harness.QUEUED_GRACE, harness.helper
        harness.QUEUED_GRACE, harness.helper = 0, lambda *a: {"lines": []}

    def tearDown(self):
        harness.QUEUED_GRACE, harness.helper = self.grace, self.helper

    def test_a_stop_mid_run_keeps_the_runs_before_it_and_the_stopped_one(self):
        task = next(t for t in TASKS if t["id"] == "shelf-q-days")
        with tempfile.TemporaryDirectory() as tmp:
            b = spend.Budget(0.7, "claude-haiku-4-5", ledger=Path(tmp) / "paid.jsonl")
            records = []
            st = run.stamp("real")
            with self.assertRaises(spend.OverBudget):
                # Run 1: $0.20. Run 2's one response, $0.60, passes $0.70: cancelled, stopped.
                run.run_all(argparse.Namespace(repeats=1, provider="anthropic"), [task], _Iso([40_000, 120_000]), "claude-haiku-4-5", desk.StandIn(),
                            Path(tmp), records, tap=type("Tap", (), {"take": lambda self: []})(), budget=b, stamp=st)
            written = [json.loads(x) for x in Path(tmp, "records.jsonl").read_text().splitlines()]
            paid = Path(tmp, "paid.jsonl").read_text().splitlines()
        self.assertEqual([r.get("stopped") for r in records], [None, "over budget"])
        self.assertEqual([r["n"] for r in written], [0, 1], "the stopped run is written too")
        self.assertEqual([(r["mode"], r["experiment"]) for r in written], [("real", st["experiment"])] * 2, "and labelled (R3-2)")
        self.assertEqual((records[0]["judge"]["pass"], records[1]["judge"]), (True, None))
        self.assertEqual(len(paid), 2)
        self.assertAlmostEqual(b.spent, 0.8, places=9)
        rep = report.build(records, qualifying(), 900, [], frozen_meta(stopped="over budget", experiment=st["experiment"]),
                           plan=[(task["id"], a, r) for r in (1, 2, 3) for a in ("on", "off")])
        self.assertEqual(rep["verdict"], "not eligible")
        self.assertIn("1 runs stopped: over budget", rep["not_eligible_because"])

    def test_usage_that_comes_while_the_run_stops_is_charged_too(self):
        # Review #14 round 2, R2-6. Shady's: the first response, $0.20,
        # passes a $0.10 budget; another already in flight brings $0.50
        # while the turn is cancelled and the history read. The record had
        # both ($0.70); the budget and paid.jsonl had only the first.
        class InFlight(_Session):
            def cancel(self):
                self.queue = [{"ev": "stream", "event": {"kind": "session", "session": "usage", "input_tokens": 0,
                                                         "output_tokens": 100_000}},  # $0.50 on Haiku 4.5
                              {"ev": "aborted", "context_saved": True}, {"ev": "idle"}]

            def messages(self):  # the query reads the wire as the daemon's does: what comes first is recorded first
                while self.queue:
                    self.next(0)
                return super().messages()

        task = next(t for t in TASKS if t["id"] == "shelf-q-days")
        with tempfile.TemporaryDirectory() as tmp:
            b = spend.Budget(0.1, "claude-haiku-4-5", ledger=Path(tmp) / "paid.jsonl")
            r = harness.Runner(_Iso([40_000]), "claude-haiku-4-5", desk.StandIn(), Path(tmp), budget=b)  # $0.20
            made = r.iso.session
            r.iso.session = lambda *a: _as(InFlight, made(*a))
            with self.assertRaises(spend.OverBudget) as e:
                r.run(0, 0, task, "off")
            paid = [json.loads(x) for x in Path(tmp, "paid.jsonl").read_text().splitlines()]
        rec = e.exception.record
        used = [ev["event"]["output_tokens"] for _, ev in rec["events"] if (ev.get("event") or {}).get("session") == "usage"]
        self.assertEqual(used, [40_000, 100_000], "the record has both")
        self.assertAlmostEqual(b.spent, 0.70, places=9)
        self.assertEqual([x["usd"] for x in paid], [0.2, 0.5])
        self.assertEqual((rec["stopped"], rec["drained"], rec.get("in_flight_unknown")), ("over budget", True, None))

    def test_a_stopped_run_drains_what_comes_after_the_history_too(self):
        # The daemon may answer the history query before the in-flight
        # response's usage arrives: only the drain to the idle sees it.
        class AnsweredFirst(_Session):
            def cancel(self):
                self.queue = [{"ev": "stream", "event": {"kind": "session", "session": "usage", "input_tokens": 0,
                                                         "output_tokens": 100_000}}, {"ev": "idle"}]
        task = next(t for t in TASKS if t["id"] == "shelf-q-days")
        with tempfile.TemporaryDirectory() as tmp:
            b = spend.Budget(0.1, "claude-haiku-4-5", ledger=Path(tmp) / "paid.jsonl")
            r = harness.Runner(_Iso([40_000]), "claude-haiku-4-5", desk.StandIn(), Path(tmp), budget=b)
            made = r.iso.session
            r.iso.session = lambda *a: _as(AnsweredFirst, made(*a))
            with self.assertRaises(spend.OverBudget) as e:
                r.run(0, 0, task, "off")
        self.assertAlmostEqual(b.spent, 0.70, places=9)
        self.assertTrue(e.exception.record["drained"])

    def test_the_session_charges_each_usage_once_as_it_records_it(self):
        # daemon.Session itself (the stand-ins mirror it): the hook sees
        # every usage frame of this session once, whatever reads it.
        import queue
        s = daemon.Session.__new__(daemon.Session)
        s.q, s.frames, s.events, s.prompts, s.t0, s.id = queue.Queue(), [], [], [], 0.0, "s1"
        usage = {"kind": "session", "session": "usage", "input_tokens": 1, "output_tokens": 2}
        for f in ({"type": "event", "session_id": "s1", "event": {"ev": "stream", "event": usage}},
                  {"type": "event", "session_id": "other", "event": {"ev": "stream", "event": usage}},
                  {"type": "event", "session_id": "s1", "event": {"ev": "stream", "event": {"kind": "llm", "llm": "text", "text": "x"}}},
                  {"type": "event", "session_id": "s1", "event": {"ev": "stream", "event": usage}}):
            s.q.put(json.dumps(f))
        got = []
        s.on_usage = got.append
        for _ in range(4):
            s.next(0)
        self.assertEqual(got, [usage, usage], "this session's two, once each")

    def test_a_stopped_run_whose_session_never_idles_says_so(self):
        s = _Session([])
        s.events = [(0.0, {"ev": "turn_started", "trigger": "user"})]
        self.assertFalse(harness.drain(s, timeout=0))
        s.events.append((1.0, {"ev": "idle"}))
        self.assertTrue(harness.drain(s, timeout=0), "idle already: nothing in flight")

    def test_no_room_for_another_run_stops_the_run_too(self):
        task = next(t for t in TASKS if t["id"] == "shelf-q-days")
        with tempfile.TemporaryDirectory() as tmp:
            b = spend.Budget(0.7, "claude-haiku-4-5")
            records = []
            with self.assertRaises(spend.OverBudget) as e:
                # Run 1 cost $0.40: another as large could pass $0.70.
                run.run_all(argparse.Namespace(repeats=1, provider="anthropic"), [task], _Iso([80_000, 1]), "claude-haiku-4-5", desk.StandIn(),
                            Path(tmp), records, tap=type("Tap", (), {"take": lambda self: []})(), budget=b, stamp=run.stamp("real"))
        self.assertEqual(len(records), 1)
        self.assertIn("no room for run 1 of 2", str(e.exception))


class Gate(unittest.TestCase):
    def rows(self, **kw):
        out = []
        for r in (1, 2, 3):
            for t in ("a", "b", "c"):
                out.append(row(t, "on", r, out=150, block=300, per_round=(40, 40), closing=1 if t == "a" else 0, **kw))
                out.append(row(t, "off", r))
        return out

    def judged(self, worse=()):
        return [{"task": t, "arm": a, "repeat": r, "pass": not (a == "on" and (t, r) in worse)}
                for r in (1, 2, 3) for t in ("a", "b", "c") for a in ("on", "off")]

    def test_every_row_passes_on_a_clean_run(self):
        rows = report.rows_of(self.rows(), self.judged(), 900)
        self.assertEqual({k: v["pass"] for k, v in rows.items()}, {k: True for k in report.GATE})
        self.assertEqual(rows["output_on_minus_off"]["median"], 50)
        self.assertAlmostEqual(rows["closing_rounds"]["share"], 1 / 3)

    def test_a_closing_round_share_over_40_percent_fails(self):
        rows = [dict(r, closing=dict(r["closing"], rounds=1)) if r["task"] in ("a", "b") else r for r in self.rows()]
        self.assertFalse(report.rows_of(rows, self.judged(), 900)["closing_rounds"]["pass"])

    def test_the_closing_share_is_40_percent_at_most(self):
        # 4 of 9 turns with a closing round: 44%, over 40% (and under 50%).
        def share(n):
            k = 0
            out = []
            for r in self.rows():
                if r["arm"] == "on":
                    r = dict(r, closing=dict(r["closing"], rounds=1 if k < n else 0))
                    k += 1
                out.append(r)
            return report.rows_of(out, self.judged(), 900)["closing_rounds"]

        self.assertTrue(share(3)["pass"], "3 of 9: 33%")
        self.assertFalse(share(4)["pass"], "4 of 9: 44%")

    def test_two_closing_rounds_in_one_turn_fail(self):
        rows = self.rows()
        rows[0] = dict(rows[0], closing=dict(rows[0]["closing"], rounds=2))
        self.assertFalse(report.rows_of(rows, self.judged(), 900)["closing_rounds"]["pass"])

    def test_net_output_gives_each_repeat_the_same_weight(self):
        # Shady's: the on-runs' outputs per turn are 1000, 1000 and 0, but the
        # third run took four turns. Pooled by turn that's 2000 / 6 = 333.33;
        # by §2.8's "difference of the repeats' means" it is 666.67.
        rows = [row("a", "on", 1, out=1000), row("a", "on", 2, out=1000)] + [row("a", "on", 3, out=0)] * 4
        rows += [row("a", "off", r, out=0) for r in (1, 2, 3)]
        g = report.rows_of(rows, [], 900)["output_on_minus_off"]
        self.assertAlmostEqual(g["per_task"]["a"], 2000 / 3, places=9)
        self.assertEqual(g["unmatched_turns"], [["a", 3]], "the repeat whose arms took different turns is named")

    def test_a_split_turn_is_no_saving(self):
        # One off turn of 1000; on, the same work in two turns of 700 (a
        # steer said instead). Per turn that read as -300: it's +400.
        rows = [row("a", "off", 1, out=1000), row("a", "on", 1, out=700), row("a", "on", 1, out=700)]
        g = report.rows_of(rows, [], 900)["output_on_minus_off"]
        self.assertEqual(g["per_task"]["a"], 400)
        self.assertEqual(g["run_totals"]["a"], {"on": {"run_output": [1400], "turns": [2]}, "off": {"run_output": [1000], "turns": [1]}})
        # A task the user plans in two turns (a card answer) is per its two.
        two = [dict(r, planned_turns=2) for r in rows]
        self.assertEqual(report.rows_of(two, [], 900)["output_on_minus_off"]["per_task"]["a"], 200)

    def test_a_task_that_passes_off_and_fails_on_fails_the_outcome_row(self):
        r = report.rows_of(self.rows(), self.judged(worse=[("b", 1), ("b", 2)]), 900)
        self.assertEqual(r["task_outcome"]["worse_with_blocks"], ["b"])

    def test_a_definition_over_1000_fails(self):
        self.assertFalse(report.rows_of(self.rows(), self.judged(), 1001)["definition"]["pass"])
        self.assertFalse(report.rows_of(self.rows(), self.judged(), None)["definition"]["pass"])

    def test_turns_with_no_block_call_dont_pull_the_median_down(self):
        rows = self.rows() + [row("d", "on", r, block=0, calls=0) for r in (1, 2, 3)] * 4
        r = report.rows_of(rows, self.judged(), 900)["block_output_per_turn"]
        self.assertEqual((r["median"], r["turns_without_calls"]), (300, 12))

    def test_a_row_passes_only_pooled_and_in_2_of_3_repeats(self):
        # Repeat 1 alone is far over (median 900 per turn); pooled, 300 holds.
        rows = [dict(r, attributed=dict(r["attributed"], block_output=900)) if r["arm"] == "on" and r["repeat"] == 1 and r["task"] != "c"
                else r for r in self.rows()]
        rep = self.build(rows)
        g = rep["gate"]["block_output_per_turn"]
        self.assertEqual((g["pooled"]["pass"], g["repeats_passing"], g["pass"]), (True, [2, 3], True))
        rows = [dict(r, attributed=dict(r["attributed"], block_output=900)) if r["arm"] == "on" and r["repeat"] in (1, 2) and r["task"] != "c"
                else r for r in self.rows()]
        g = self.build(rows)["gate"]["block_output_per_turn"]
        self.assertEqual(g["repeats_passing"], [3])
        self.assertFalse(g["pass"], "one repeat isn't a licence")

    def build(self, rows):
        orig = report.per_turn
        by = {}
        for r in rows:
            by.setdefault((r["task"], r["arm"], r["repeat"]), []).append(r)
        recs = [{"task": t, "arm": a, "repeat": rp, "fold": {"lines": []}, "judge": {"pass": True}} for (t, a, rp) in by]
        report.per_turn = lambda rec, counter: (by[(rec["task"], rec["arm"], rec["repeat"])], [])
        try:
            return report.build(recs, _Counter(), 900, [], {"mode": "test"})
        finally:
            report.per_turn = orig


class _Counter:
    def describe(self):
        return {"kind": "test"}


def thirds(s):
    """A proxy for the tests: a token per 3 characters."""
    return len(s) // 3


def qualifying():
    """A fitted proxy (count.Calibrated): a counter that can flip the default.
    0.9 provider tokens a proxy token, a pair allowance of 9, of which the
    output control finds 6 a call's own framing on output."""
    receipt = thirds(calibrate.RECEIPT)
    pts = [(x, 9.0 + 0.9 * (receipt + x)) for x in range(10, 10 + 3 * calibrate.CALIBRATION_MIN, 3)]
    f = calibrate.fit(pts, "thirds", receipt)
    out = calibrate.output_control(calibrate.load()[0], [], lambda body: _called(6 + 0.9 * thirds(compact(_forced_input(body))), _forced_input(body)),
                                   lambda s: f.ratio * thirds(s))
    return count.Calibrated(f, thirds, out)


def _forced_input(body):
    """The input an output-control request asks the model for."""
    return json.loads(body["messages"][0]["content"][0]["text"][len(calibrate.FORCE):])


def _called(output, call_input):
    """A forced call's response (providers.py's shape): the one call asked
    for and nothing else."""
    return {"output": output, "calls": [{"name": "cards_block", "input": call_input}], "text": "", "other": [],
            "stop": "tool_use", "by_call": True}


EXPERIMENT = "e" * 32  # the fixtures' experiment (run.py's `stamp`)


def frozen_meta(**kw):
    return {"mode": "real", "experiment": EXPERIMENT, "taskset": taskset.FROZEN.read_text().split()[0], "repeats": 3} | kw


class Eligibility(unittest.TestCase):
    """The verdict (report.py's "The verdict"): rows that pass license
    nothing unless the run is §2.8's experiment. Every run here is built
    on the session wire and read by the real parser, as Shady's
    counterexamples were: the frozen set's runnable tasks, each category
    as it plays, every gate row passing."""

    @staticmethod
    def run_events(task, arm, cut=False, silent=False):
        """One run's events, its tool ids, and the user's messages as sent
        (each starting the turn it opens, so the history has them)."""
        say = task["turns"][0]["say"]
        w = Wire().start(say)
        cat, ids, hb = task["category"], [], next((s["hand_back"] for s in task["turns"] if "hand_back" in s), None)
        if silent:  # Shady's: TurnStarted, done, Idle, no response
            return w.done().events, ids
        if cat == "no_tool":
            w.round(text="An answer.")
        elif cat == "short":
            w.results(w.round([("read", {"path": "a"})]))
            w.round(text="An answer.")
        else:
            if hb:
                got = w.round([("cards_block", PUT)] if arm == "on" else [], text="My proposal.")
                ids += got
                w.results(got)
                w.done()
                w.start(Eligibility.HANDED if arm == "on" else hb["message"])
            elif cat == "interactive":
                w.results(w.round([("cards_ui", {"question": "Which?"})]), "Card cabc123 is showing (cards/1 #1): Which?")
                w.round(text="Waiting.")
                w.done()
                w.start(Eligibility.ANSWER)
            for k in range(5):
                calls = [("bash", {"k": k})] + ([("cards_block", BLOCK)] if arm == "on" and k in (0, 4) else [])
                got = w.round(calls)
                ids += got[1:]
                w.results(got)
            if cut:
                w.stream("llm", "response_start").stream("llm", "text", text="Done.")
            else:
                w.round(text="Done.")
        return w.done().events, ids

    HANDED = "Go ahead.\n[block plan edited r1]"
    ANSWER = "[card cabc123] Which? → A\n{\"choice\":\"A\"}"
    STEER = "Also: keep it short."

    def records(self, repeats=(1, 2, 3), drop=0, cut=None, probe=lambda task, repeat: True, tasks=None, silent=None):
        """The frozen set's whole matrix (report.required), every run as it
        would be recorded: its events, steps, what the user sent, and the
        history (the agent's received input)."""
        recs = []
        for r, t, arm in harness.plan([dict(x, needs=[]) for x in (TASKS if tasks is None else tasks)], 3)[0]:
            if r + 1 not in repeats:
                continue
            events, ids = self.run_events(t, arm, cut=(t["id"], arm, r + 1) == cut, silent=(t["id"], arm, r + 1) == silent)
            lines = [{"tool_id": i, "event": n, "op": 0, "author": "agent", "did": "patch", "outcome": "applied",
                      "dismissed_before": False} for n, i in enumerate(ids)]
            steps, sent = [], [{"carry": "submit", "text": t["turns"][0]["say"], "notices": []}]
            hb = next((s["hand_back"] for s in t["turns"] if "hand_back" in s), None)
            if hb and arm == "on":
                steps.append({"step": "hand_back", "as": "block", "block": "plan"})
                sent.append({"carry": "hand_back", "text": self.HANDED, "notices": []})
            elif hb:
                steps.append({"step": "hand_back", "as": "message"})
                sent.append({"carry": "submit", "text": hb["message"], "notices": []})
            elif t["category"] == "interactive":
                steps.append({"step": "answer", "card": "cabc123", "values": {"choice": "A"}})
                sent.append({"carry": "answer", "text": self.ANSWER, "notices": []})
            if any("dismiss" in s for s in t["turns"]):
                if arm == "on" and probe(t["id"], r + 1):
                    steps.append({"step": "dismiss", "id": "plan", "outcome": "accepted"})
                    sent.append({"carry": "steer", "text": self.STEER + "\n[block plan v1 dismissed]", "notices": [["plan", "dismissed", 1]]})
                else:
                    steps.append({"step": "dismiss", "outcome": "skipped", "skipped": "no live block"})
            history = [m for x in sent for m in ({"role": "user", "content": x["text"]}, {"role": "assistant", "content": "…"})]
            recs.append({"task": t["id"], "category": t["category"], "arm": arm, "repeat": r + 1, "events": events,
                         "fold": {"lines": lines}, "judge": {"pass": True}, "steps": steps, "sent": sent, "history": history,
                         "mode": "real", "experiment": EXPERIMENT})
        return recs[:len(recs) - drop]

    WHOLE = [(t["id"], arm, r + 1) for r, t, arm in harness.plan([dict(x, needs=[]) for x in TASKS], 3)[0]]

    def build(self, recs, counter=None, not_run=(), plan=WHOLE, **meta):
        """report.build, with B5b's plan: the whole matrix, D8 included."""
        return report.build(recs, counter or qualifying(), 900, list(not_run), frozen_meta(**meta), plan=plan)

    def test_one_repeat_and_an_uncalibrated_counter_is_not_eligible(self):
        # Shady's first counterexample: it passed the gate.
        rep = self.build(self.records(repeats=(1,)), count.Uncalibrated(thirds, "thirds"), repeats=1)
        self.assertFalse(rep["gate_pass"], "one repeat with an uncalibrated counter is no licence")
        self.assertEqual(rep["verdict"], "not eligible")
        why = " / ".join(rep["not_eligible_because"])
        for part in ("uncalibrated", "1 repeats", "180 of the frozen set's 270 runs missing"):
            self.assertIn(part, why)

    def test_three_repeats_with_an_uncalibrated_counter_are_not_eligible(self):
        rep = self.build(self.records(), count.Uncalibrated(thirds, "thirds"))
        self.assertTrue(rep["rows_pass"])
        self.assertFalse(rep["gate_pass"])
        self.assertEqual(len(rep["not_eligible_because"]), 1)
        self.assertIn("the counter is uncalibrated", rep["not_eligible_because"][0])

    def test_the_d8_tasks_not_run_leave_it_not_eligible(self):
        d8 = [t for t in TASKS if t["needs"]]
        b5a = self.records(tasks=[t for t in TASKS if not t["needs"]])
        for not_run in (d8, []):  # the caller's list is descriptive: the set says what B6 needs
            rep = self.build(b5a, not_run=not_run, plan=None)
            self.assertFalse(rep["gate_pass"])
            self.assertEqual(rep["not_eligible_because"],
                             [f"{len(d8)} tasks not run ({', '.join(t['id'] for t in d8)}): B6 needs them, both arms (B5b)"])
        self.assertIn("Gate: **not eligible**", report.markdown(rep))

    def test_the_matrix_is_the_frozen_sets_not_the_callers(self):
        # Review #14 round 2, R2-2. Shady's: the 252 runs B5a plans, with
        # the plan that makes them, and no `not_run`, passed: it has no D8
        # task. So did a run with a coding task left out of the records and
        # of the plan.
        b5a = [t for t in TASKS if not t["needs"]]
        plan = [(t["id"], arm, r + 1) for r, t, arm in harness.plan(b5a, 3)[0]]
        rep = self.build(self.records(tasks=b5a), plan=plan)
        self.assertEqual(rep["verdict"], "not eligible")
        self.assertIn("the run's plan isn't the frozen set's: it leaves out 18 of its 270 runs", rep["not_eligible_because"])
        self.assertIn("3 tasks not run", " / ".join(rep["not_eligible_because"]))
        coding = next(t["id"] for t in TASKS if t["category"] == "coding")
        fewer = [t for t in TASKS if t["id"] != coding]
        rep = self.build(self.records(tasks=fewer), plan=[(t["id"], arm, r + 1) for r, t, arm in harness.plan([dict(x, needs=[]) for x in fewer], 3)[0]])
        self.assertEqual(rep["verdict"], "not eligible")
        self.assertIn(f"1 tasks of the frozen set not run ({coding})", rep["not_eligible_because"])
        self.assertEqual(sum(report.required().values()), 270)
        self.assertTrue(self.build(self.records(), plan=None)["gate_pass"], "the whole matrix, with no plan given")

    def test_every_run_needs_its_judged_outcome(self):
        # Shady's: every judgment None, and the run passed (the outcome row
        # had nothing to compare, so nothing was worse).
        recs = [dict(r, judge=None) for r in self.records()]
        rep = self.build(recs)
        self.assertEqual(rep["verdict"], "not eligible")
        self.assertIn("270 runs with no judged outcome", rep["not_eligible_because"])
        self.assertFalse(rep["gate"]["task_outcome"]["pass"], "no judgment is no outcome, not an outcome no worse")
        one = self.records()
        one[7] = dict(one[7], judge={"pass": None, "how": "tests"})
        self.assertIn("1 runs with no judged outcome", self.build(one)["not_eligible_because"])

    def test_a_turn_the_provider_never_answered_is_not_measured(self):
        # Shady's: TurnStarted, done, Idle, no response and no usage, in
        # place of one on-arm no-tool turn: it was measured, and passed.
        nt = next(t["id"] for t in TASKS if t["category"] == "no_tool")
        recs = self.records(silent=(nt, "on", 1))
        (silent,) = [r for r in recs if (r["task"], r["arm"], r["repeat"]) == (nt, "on", 1)]
        (turn,) = turns.turns(silent["events"])
        self.assertEqual((turn.outcome, len(turn.rounds), turn.measured), ("done", 0, False))
        rep = self.build(recs)
        self.assertEqual(rep["verdict"], "not eligible")
        self.assertIn("1 runs with a turn not measured", " / ".join(rep["not_eligible_because"]))

    def test_an_interrupted_run_is_incomplete_not_a_smaller_experiment(self):
        # A budget stop after two whole repeats: they pass, 2 of 3.
        rep = self.build(self.records(repeats=(1, 2)))
        self.assertTrue(all(g["repeats_passing"] == [1, 2] for g in rep["gate"].values()))
        self.assertFalse(rep["gate_pass"])
        self.assertIn("90 of the frozen set's 270 runs missing (an interrupted run is incomplete)", rep["not_eligible_because"])
        rep = self.build(self.records(drop=5))
        self.assertFalse(rep["gate_pass"])
        self.assertIn("5 of the frozen set's 270 runs missing", " / ".join(rep["not_eligible_because"]))
        rep = self.build(self.records()[:-1] + [dict(self.records()[-1], stopped="over budget")])
        self.assertFalse(rep["gate_pass"])
        self.assertIn("1 runs stopped: over budget", rep["not_eligible_because"])

    def test_a_run_with_a_turn_not_measured_is_not_eligible(self):
        coding = next(t["id"] for t in TASKS if t["category"] == "coding")
        rep = self.build(self.records(cut=(coding, "on", 2)))
        self.assertFalse(rep["gate_pass"])
        self.assertIn("1 runs with a turn not measured", " / ".join(rep["not_eligible_because"]))

    def test_a_notice_probe_must_reach_the_agent_in_2_of_3_on_runs(self):
        dismissing = sorted(t["id"] for t in TASKS if any("dismiss" in s for s in t["turns"]))
        self.assertEqual(len(dismissing), 4)
        rep = self.build(self.records(probe=lambda task, r: not (task == dismissing[0] and r > 1)))
        self.assertFalse(rep["gate_pass"])
        self.assertEqual(rep["not_eligible_because"], [f"the notice probe didn't reach the agent in 2 of 3 on-runs: {dismissing[0]}"])
        self.assertTrue(self.build(self.records(probe=lambda task, r: not (task == dismissing[0] and r == 3)))["gate_pass"])

    def change(self, recs, kind, fn):
        """`recs` with each on-run of a task with a `kind` step changed by `fn`."""
        by = {x["id"]: x for x in TASKS}
        return [fn(dict(r), by[r["task"]]) if r["arm"] == "on" and any(kind in s for s in by[r["task"]]["turns"]) else r for r in recs]

    def test_on_run_hand_backs_must_travel_as_blocks(self):
        # Review #14 round 2, R2-1. Shady's: all nine on-arm hand-backs fell
        # back to the plain message (no live block), and the run passed.
        def fallback(r, task):
            (hb,) = [s["hand_back"] for s in task["turns"] if "hand_back" in s]
            r["steps"] = [{"step": "hand_back", "as": "message", "said_instead": "no live plan block"}]
            r["sent"] = [r["sent"][0], {"carry": "submit", "text": hb["message"], "notices": []}]
            r["history"] = [{"role": "user", "content": m["text"]} for m in r["sent"]]
            return r
        rep = self.build(self.change(self.records(), "hand_back", fallback))
        self.assertTrue(rep["rows_pass"])
        self.assertEqual(rep["verdict"], "not eligible")
        (why,) = rep["not_eligible_because"]
        self.assertTrue(why.startswith("9 on-run card answers or hand-backs didn't happen as the task says: "), why)
        self.assertIn("r1: hand-back sent as the plain message, a fallback (no live plan block)", why)
        # A refused hand-back: the desk wouldn't take the block, nothing went.
        one = self.records()
        i = next(k for k, r in enumerate(one) if r["arm"] == "on" and r["task"] == "shelf-handback-plan" and r["repeat"] == 2)
        one[i] = dict(one[i], steps=[{"step": "hand_back", "skipped": "the desk refused: no such block", "block": "plan"}], sent=one[i]["sent"][:1])
        self.assertEqual(self.build(one)["not_eligible_because"],
                         ["1 on-run card answers or hand-backs didn't happen as the task says: "
                          "shelf-handback-plan r2: hand-back the desk refused: no such block"])

    def test_every_on_run_card_answer_must_happen(self):
        # Shady's: all fifteen on-arm answers skipped as interaction
        # failures, every code check passing, and the run passed.
        def skipped(r, task):
            r["steps"] = [{"step": "answer", "skipped": "no option labelled A", "interaction": True}]
            r["sent"] = r["sent"][:1]
            r["history"] = r["history"][:2]
            return r
        rep = self.build(self.change(self.records(), "answer", skipped))
        self.assertTrue(rep["rows_pass"])
        self.assertEqual(rep["verdict"], "not eligible")
        self.assertIn("15 on-run card answers or hand-backs didn't happen", rep["not_eligible_because"][0])
        self.assertIn("ledger-card-negative r1: card answer skipped (no option labelled A)", rep["not_eligible_because"][0],
                      "the skip is the reason named, not only what it left unsent")
        self.assertEqual(len(rep["also"]["interaction_failures"]), 15)
        # Sent, but not in the agent's received input: no sign it got there.
        lost = self.change(self.records(), "answer", lambda r, task: dict(r, history=r["history"][:2]) if r["repeat"] == 3 else r)
        self.assertIn("ledger-card-negative r3: 1 answer message(s) not in the agent's history", self.build(lost)["not_eligible_because"][0])

    def test_a_notice_the_agent_never_received_isnt_delivered(self):
        # Shady's: the fixture had no notice reach the agent, yet each probe
        # was delivered three times, from the sent list alone.
        dismissing = sorted(t["id"] for t in TASKS if any("dismiss" in s for s in t["turns"]))
        recs = [dict(r, history=[m for m in r["history"] if "dismissed]" not in m["content"]])
                if r["task"] == dismissing[1] and r["arm"] == "on" and r["repeat"] > 1 else r for r in self.records()]
        rep = self.build(recs)
        self.assertEqual(rep["not_eligible_because"], [f"the notice probe didn't reach the agent in 2 of 3 on-runs: {dismissing[1]}"])
        self.assertEqual(rep["also"]["notice_probes"][dismissing[1]], {"runs": 3, "clicked": 3, "sent": 3, "delivered": 1, "lost": 2,
                                                                       "refused": 0, "skipped": 0})

    def test_pilot_records_never_qualify_whatever_the_meta_says(self):
        # Review #14 round 3, R3-2. Shady's: the qualifying whole matrix,
        # its 22 coding/off/repeat-1 runs a pilot's, under a real run's
        # meta. At a29f248 it passed: the label was only the meta's.
        recs = self.records()
        pilot = [r for r in recs if r["category"] == "coding" and r["arm"] == "off" and r["repeat"] == 1]
        self.assertEqual(len(pilot), 22)
        for r in pilot:
            r["mode"] = "pilot"
        rep = self.build(recs)
        self.assertEqual(rep["verdict"], "not eligible")
        self.assertEqual(rep["not_eligible_because"], ["records not from a real run, never data: 22 pilot"])
        # A record with no label, or of another experiment, is no better.
        for r in pilot:
            del r["mode"]
        self.assertEqual(self.build(recs)["not_eligible_because"], ["records not from a real run, never data: 22 with no mode"])
        for r in pilot:
            r["mode"], r["experiment"] = "real", "p" * 32
        self.assertEqual(self.build(recs)["not_eligible_because"], [f"records of another experiment than this run's {EXPERIMENT[:12]}: 22"])
        for r in pilot:
            r["experiment"] = EXPERIMENT
        self.assertEqual(self.build(recs)["not_eligible_because"], [])
        self.assertIn("the run names no experiment", " / ".join(self.build(recs, experiment=None)["not_eligible_because"]))
        self.assertIn("a pilot's configuration, never data", self.build(recs, config={"pilot": "the coding tasks"})["not_eligible_because"])

    def test_a_dry_run_and_another_task_set_are_never_eligible(self):
        recs = self.records()
        self.assertIn("a dry run, never data", self.build(recs, mode="dry")["not_eligible_because"])
        self.assertIn("not the frozen", " / ".join(self.build(recs, taskset="0" * 64)["not_eligible_because"]))

    def test_the_whole_plan_measured_real_and_calibrated_is_eligible(self):
        recs = self.records()
        rep = self.build(recs)
        self.assertEqual((rep["not_eligible_because"], rep["verdict"], rep["gate_pass"]), ([], "passes", True))
        self.assertIn("Eligible for B6: yes.", report.markdown(rep))
        worse = [dict(r, judge={"pass": False}) if r["arm"] == "on" and r["repeat"] < 3 and r["task"] == recs[0]["task"] else r
                 for r in recs]
        rep = self.build(worse)
        self.assertEqual((rep["eligible_for_b6"], rep["verdict"], rep["gate_pass"]), (True, "fails", False))

    def test_a_real_run_takes_three_repeats(self):
        argv = ["--provider", "anthropic", "--synaps", "/nonexistent/synaps", "--model", "claude-haiku-4-5", "--budget-usd", "5",
                "--approved", "claude-haiku-4-5@5", "--desk", "/nonexistent/desk", "--repeats", "1"]
        with self.assertRaises(SystemExit) as e:
            run.main(argv)
        self.assertIn("3 repeats", str(e.exception))


class Pilot(unittest.TestCase):
    """The depth pilot (run.py's `--pilot`): the coding tasks, blocks off,
    once, on the intended model; never B6 data, and no way round the
    experiment's rules."""

    ARGV = ["--pilot", "--provider", "anthropic", "--synaps", "/nonexistent/synaps", "--model", "claude-sonnet-5-5",
            "--budget-usd", "8", "--approved", "pilot:claude-sonnet-5-5@8"]

    def refused(self, argv):
        with self.assertRaises(SystemExit) as e:
            run.main(argv)
        return str(e.exception)

    def test_it_plans_every_coding_task_blocks_off_once(self):
        order = harness.pilot_plan(TASKS)
        self.assertEqual(len(order), 22)
        self.assertEqual({(r, arm) for r, _, arm in order}, {(0, "off")})
        self.assertEqual([t["id"] for _, t, _ in order], [t["id"] for t in TASKS if t["category"] == "coding"])

    def test_it_takes_its_own_approval_and_relaxes_nothing(self):
        for extra in (["--repeats", "1"], ["--tasks", "ledger-since"], ["--desk", "/x"], ["--d8-stand-in"]):
            self.assertIn("are the experiment's", self.refused(self.ARGV + extra))
        run_approval = [x if x != "pilot:claude-sonnet-5-5@8" else "claude-sonnet-5-5@8" for x in self.ARGV]
        self.assertIn("isn't this pilot's pilot:claude-sonnet-5-5@8", self.refused(run_approval))
        self.assertIn("--approved pilot:MODEL@USD", self.refused([x for x in self.ARGV if not x.startswith("pilot:") and x != "--approved"]))
        self.assertEqual(self.refused(["--pilot", "--dry-run", "--synaps", "/x"]), "2", "a pilot is on a provider")
        # And a pilot's approval is no run's.
        argv = ["--provider", "anthropic", "--synaps", "/x", "--model", "claude-sonnet-5-5", "--budget-usd", "8",
                "--approved", "pilot:claude-sonnet-5-5@8", "--desk", "/x"]
        self.assertIn("isn't this run's", self.refused(argv))

    def test_its_report_is_each_tasks_depth_and_never_eligible(self):
        coding = [t for t in TASKS if t["category"] == "coding"]
        recs = []
        for i, t in enumerate(coding[:-1]):  # the last one never ran
            w = Wire().start("Do it.")
            for k in range(4 + i % 5):  # 5 to 9 rounds
                w.results(w.round([("bash", {"k": k})]))
            w.round(text="Done.")
            recs.append({"task": t["id"], "category": "coding", "arm": "off", "repeat": 1, "events": w.done().events,
                         "judge": {"pass": i % 7 != 0}, "fold": {"lines": []}, "steps": [], "sent": []})
        rep = report.pilot(recs, {"mode": "pilot", "model": "claude-sonnet-5-5", "head": "abc1234", "taskset": "f" * 64})
        s = rep["summary"]
        self.assertEqual((rep["eligible_for_b6"], rep["verdict"], s["tasks"], s["of"], s["missing"]), (False, "pilot", 21, 22, [coding[-1]["id"]]))
        self.assertEqual(sorted(r["rounds"] for r in rep["tasks"])[:2], [5, 5])
        self.assertEqual(s["at_depth"], 21 - len(s["under_depth"]))
        self.assertEqual(s["margins"], sorted(r["rounds"] - 6 for r in rep["tasks"]))
        md = report.pilot_markdown(rep)
        self.assertIn("never B6 data", md)
        self.assertIn("a screen, not the experiment's two of three", md)
        # A pilot's records are never an experiment's.
        self.assertIn("a pilot run, never data", report.build(recs, qualifying(), 900, [], frozen_meta(mode="pilot"))["not_eligible_because"])

    def test_it_runs_off_once_through_the_same_runner_ledger_and_stop(self):
        task = next(t for t in TASKS if t["id"] == "shelf-q-days")
        grace, helper = harness.QUEUED_GRACE, harness.helper
        harness.QUEUED_GRACE, harness.helper = 0, lambda *a: {"lines": []}
        try:
            with tempfile.TemporaryDirectory() as tmp:
                b = spend.Budget(5, "claude-haiku-4-5", ledger=Path(tmp) / "paid.jsonl")
                records = []
                st = run.stamp("pilot")
                run.run_all(argparse.Namespace(repeats=3, provider="anthropic"), [task], _Iso([1000]), "claude-haiku-4-5", desk.StandIn(),
                            Path(tmp), records, tap=type("Tap", (), {"take": lambda self: []})(), budget=b, order=[(0, task, "off")],
                            stamp=st)
                paid = Path(tmp, "paid.jsonl").read_text().splitlines()
                written = [json.loads(x) for x in Path(tmp, "records.jsonl").read_text().splitlines()]
        finally:
            harness.QUEUED_GRACE, harness.helper = grace, helper
        self.assertEqual([(r["task"], r["arm"], r["repeat"]) for r in records], [("shelf-q-days", "off", 1)])
        self.assertEqual(len(paid), 1)
        # Review #14 round 3, R3-2: the raw records say what they are, not
        # only the pilot's report, and no report makes them eligible.
        self.assertEqual([(r["mode"], r["experiment"]) for r in written], [("pilot", st["experiment"])])
        why = report.build(written, qualifying(), 900, [], frozen_meta(experiment=st["experiment"]))["not_eligible_because"]
        self.assertIn("records not from a real run, never data: 1 pilot", why)

    def test_what_it_writes_says_pilot_and_no_report_makes_it_data(self):
        # Review #14 round 3, R3-2: run.pilot end to end (its provider and
        # daemon offline), read back from what it wrote: records.jsonl's
        # every line is the pilot's, and a report that says it is real
        # still can't use them.
        task = next(t for t in TASKS if t["category"] == "coding" and len(t["turns"]) == 1 and set(t["turns"][0]) <= {"say", "say_off"})

        class Iso(_Iso):
            def install(self):
                pass

            def start(self):
                pass

            def stop(self):
                return {"head": "abc1234", "daemon_exit": 0, "runtime_dir_left": False}

        prov = type("Prov", (), {"has_endpoint": True, "base": "http://x", "model": "claude-haiku-4-5"})()
        tap = type("Tap", (), {"take": lambda self: [], "close": lambda self: None})()
        saved = (run.upstream, run.daemon.Isolated, harness.QUEUED_GRACE, harness.helper)
        run.upstream, run.daemon.Isolated = (lambda a, budget: (prov, tap, {}, a.model)), (lambda *a, **k: Iso([1000]))
        harness.QUEUED_GRACE, harness.helper = 0, lambda *a: {"lines": []}
        try:
            with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stdout(io.StringIO()):
                a = argparse.Namespace(repeats=3, tasks=None, desk=None, d8_stand_in=False, model="claude-haiku-4-5", budget_usd=1.0,
                                       approved="pilot:claude-haiku-4-5@1", work=tmp, synaps="/x", provider="anthropic", pilot=True,
                                       dry_run=False, base_url=None)
                self.assertEqual(run.pilot(a, [task], "f" * 64), 0)
                written = [json.loads(x) for x in Path(tmp, "records.jsonl").read_text().splitlines()]
                meta = json.loads(Path(tmp, "pilot.json").read_text())["meta"]
        finally:
            run.upstream, run.daemon.Isolated, harness.QUEUED_GRACE, harness.helper = saved
        self.assertEqual([(r["task"], r["arm"], r["mode"]) for r in written], [(task["id"], "off", "pilot")])
        self.assertEqual((meta["mode"], meta["experiment"]), ("pilot", written[0]["experiment"]))
        why = report.build(written, qualifying(), 900, [], frozen_meta(experiment=meta["experiment"]))["not_eligible_because"]
        self.assertIn("records not from a real run, never data: 1 pilot", why)

    def test_its_estimate_and_budget(self):
        t = estimate.table(TASKS)
        pp = t["pilot"]
        self.assertEqual(pp["tasks"], 22)
        s = pp["usd"]
        self.assertEqual((s["expected_warm"]["claude-sonnet-5-5"], s["expected_cold"]["claude-sonnet-5-5"], s["long_cold"]["claude-sonnet-5-5"]),
                         (1.58, 2.11, 6.10))
        self.assertEqual(pp["budget_usd"]["claude-sonnet-5-5"], 8)
        self.assertIn("The depth pilot (`--pilot`)", t["markdown"])


class SetShape(unittest.TestCase):
    """§2.8's set as the run played it, blocks off (report.py's "The set's
    shape"): 20 coding tasks of ≥ 6 rounds, 10 short of 1–2, 5 with no tool
    call, 5 whose card was answered."""

    PLAYS = {"coding": {"rounds": 6}, "short": {"rounds": 2, "tool_calls": 1}, "no_tool": {"rounds": 1, "tool_calls": 0},
             "interactive": {"rounds": 4}}

    def run_of(self, off=None, on=None, steps=None):
        """Records and turn rows of a whole set, each run as its category
        plays, but for `off[(task, repeat)]` / `on[...]` (row fields) and
        `steps[(task, repeat)]` (an off-run's harness steps)."""
        off, on, steps = off or {}, on or {}, steps or {}
        recs, rows = [], []
        for cat, n in taskset.CATEGORIES.items():
            for i in range(n):
                task = f"{cat}-{i}"
                for rp in (1, 2, 3):
                    for arm, over in (("off", off), ("on", on)):
                        st = [{"step": "answer", "card": "c1"}] if cat == "interactive" else []
                        if arm == "off":
                            st = steps.get((task, rp), st)
                        recs.append({"task": task, "category": cat, "arm": arm, "repeat": rp, "steps": st})
                        rows.append({**row(task, arm, rp), **self.PLAYS[cat], **over.get((task, rp), {})})
        return recs, rows

    def test_a_set_that_plays_its_categories_holds(self):
        s = report.set_shape(*self.run_of())
        self.assertTrue(s["holds"])
        self.assertEqual({c: (s[c]["held"], s[c]["tasks"], s[c]["short_of"]) for c in report.SHAPE},
                         {"coding": (20, 20, []), "short": (10, 10, []), "no_tool": (5, 5, []), "interactive": (5, 5, [])})

    def test_a_coding_task_of_5_rounds_in_2_of_3_off_runs_falls_short(self):
        s = report.set_shape(*self.run_of(off={("coding-3", 1): {"rounds": 5}, ("coding-3", 3): {"rounds": 5}}))
        self.assertEqual((s["coding"]["held"], s["coding"]["short_of"], s["holds"]), (19, ["coding-3"], False))

    def test_a_coding_task_under_6_rounds_is_reported_apart_and_not_as_long(self):
        # Frozen at B5a (review #14, P1 4): long only if its blocks-off runs
        # reached 6 rounds; the rest are reported apart, with their rounds.
        s = report.set_shape(*self.run_of(off={("coding-3", 1): {"rounds": 5}, ("coding-3", 3): {"rounds": 4}}))
        self.assertEqual(s["coding"]["apart"], [{"task": "coding-3", "rounds": [5, 6, 4]}])
        self.assertNotIn("coding-3", s["coding"]["held_tasks"])
        self.assertEqual(report.set_shape(*self.run_of())["coding"]["apart"], [])

    def test_every_row_is_reported_per_bucket_with_the_short_coding_tasks_apart(self):
        recs, rows = self.run_of(off={("coding-3", rp): {"rounds": 5} for rp in (1, 2, 3)})
        by = {}
        for r in rows:
            by.setdefault((r["task"], r["arm"], r["repeat"]), []).append(r)
        orig = report.per_turn
        report.per_turn = lambda rec, counter: (by[(rec["task"], rec["arm"], rec["repeat"])], [])
        try:
            rep = report.build([dict(r, fold={"lines": []}, judge={"pass": True}) for r in recs], _Counter(), 900, [], {"mode": "test"})
        finally:
            report.per_turn = orig
        b = rep["also"]["by_bucket"]
        self.assertEqual(list(b), ["long", "coding_under_6", "short", "no_tool", "interactive"])
        self.assertEqual(b["coding_under_6"]["tasks"], ["coding-3"])
        self.assertEqual(len(b["long"]["tasks"]), 19)
        self.assertNotIn("coding-3", b["long"]["tasks"])
        self.assertEqual(b["coding_under_6"]["rows"]["output_on_minus_off"]["tasks"], 1)
        self.assertNotIn("pass", b["long"]["rows"]["closing_rounds"])
        self.assertIn("`coding-3` (5, 5, 5)", report.markdown(rep))

    def test_one_short_off_run_of_3_still_holds(self):
        s = report.set_shape(*self.run_of(off={("coding-3", 2): {"rounds": 5}}))
        self.assertEqual((s["coding"]["held"], s["holds"]), (20, True))

    def test_it_is_read_with_blocks_off(self):
        # Blocks' own closing round would lift a 5-round task to 6: that isn't the task's size.
        five = {(f"coding-{i}", rp): {"rounds": 5} for i in range(20) for rp in (1, 2, 3)}
        six = {k: {"rounds": 6} for k in five}
        self.assertFalse(report.set_shape(*self.run_of(off=five, on=six))["holds"])
        self.assertTrue(report.set_shape(*self.run_of(off=six, on=five))["holds"])

    def test_rounds_add_up_over_a_tasks_turns(self):
        # A card task's two turns, or a coding task's said-instead steer: one run, 3 + 3 rounds.
        recs, rows = self.run_of()
        rows = [dict(r, rounds=3) for r in rows if r["task"] == "coding-0"] * 2 + [r for r in rows if r["task"] != "coding-0"]
        self.assertEqual(report.set_shape(recs, rows)["coding"]["held"], 20)

    def test_short_is_1_or_2_rounds(self):
        s = report.set_shape(*self.run_of(off={("short-1", rp): {"rounds": 3} for rp in (1, 2)}))
        self.assertEqual((s["short"]["short_of"], s["holds"]), (["short-1"], False))

    def test_a_no_tool_task_that_calls_a_tool_falls_short(self):
        s = report.set_shape(*self.run_of(off={("no_tool-2", rp): {"tool_calls": 1} for rp in (2, 3)}))
        self.assertEqual((s["no_tool"]["short_of"], s["holds"]), (["no_tool-2"], False))

    def test_a_no_tool_run_with_no_turn_falls_short(self):
        recs, rows = self.run_of()
        rows = [r for r in rows if not (r["task"] == "no_tool-0" and r["arm"] == "off")]
        self.assertEqual(report.set_shape(recs, rows)["no_tool"]["short_of"], ["no_tool-0"])

    def test_a_card_task_whose_answer_was_skipped_falls_short(self):
        skip = {("interactive-4", rp): [{"step": "answer", "skipped": "no card showing"}] for rp in (1, 3)}
        s = report.set_shape(*self.run_of(steps=skip))
        self.assertEqual((s["interactive"]["short_of"], s["holds"]), (["interactive-4"], False))
        handed = {("interactive-4", rp): [{"step": "hand_back", "block": "plan"}] for rp in (1, 2, 3)}
        self.assertTrue(report.set_shape(*self.run_of(steps=handed))["holds"])

    def build(self, recs, rows):
        """report.build over these runs, every gate row passing (rows_of is
        Gate's to test): only the shape can fail it."""
        by = {}
        for r in rows:
            by.setdefault((r["task"], r["arm"], r["repeat"]), []).append(r)
        orig = (report.per_turn, report.rows_of, taskset.load)
        report.per_turn = lambda rec, counter: ([dict(x, measured=True) for x in by[(rec["task"], rec["arm"], rec["repeat"])]], [])
        report.rows_of = lambda *a: {k: {"pass": True} for k in report.GATE}
        # This set is the frozen one here: eligibility's matrix is the set's.
        taskset.load = lambda *a: [{"id": t, "needs": [], "turns": []} for t in dict.fromkeys(r["task"] for r in recs)]
        try:
            return report.build([dict(r, fold={"lines": []}, judge={"pass": True}, mode="real", experiment=EXPERIMENT) for r in recs],
                                qualifying(), 900, [], frozen_meta(),
                                plan=[(r["task"], r["arm"], r["repeat"]) for r in recs])
        finally:
            report.per_turn, report.rows_of, taskset.load = orig

    def test_the_gate_passes_only_if_the_shape_holds_and_says_why(self):
        rep = self.build(*self.run_of())
        self.assertTrue(rep["set_shape"]["holds"])
        self.assertTrue(rep["gate_pass"])
        self.assertIn("The set's shape, blocks off: holds: coding 20 of 20 (≥ 6 rounds; ≥ 20), short 10 of 10", report.markdown(rep))
        rep = self.build(*self.run_of(off={("coding-7", rp): {"rounds": 5} for rp in (1, 2, 3)}))
        self.assertTrue(all(g["pass"] for g in rep["gate"].values()))
        self.assertFalse(rep["gate_pass"], "a run that didn't play §2.8's set didn't measure it")
        md = report.markdown(rep)
        self.assertIn("The set's shape, blocks off: **does not hold**: coding 19 of 20 (≥ 6 rounds; ≥ 20), short: `coding-7`, short 10", md)
        self.assertIn("Gate: **not eligible**", md)
        self.assertIn("Eligible for B6: **no**: the set's shape doesn't hold.", md)


if __name__ == "__main__":
    unittest.main()
