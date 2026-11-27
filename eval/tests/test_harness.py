"""The harness's own parts, offline: the run order, the turns a task's steps
make, the desk seam (the stand-in, and a real run's refusal of it), the
providers' request shapes and the tap, spend and the budget guard, and the
report's gate (pooled and 2 of 3 repeats)."""

import json
import threading
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from b5 import desk, estimate, harness, providers, report, spend, taskset
from wire import BLOCK, PUT, Wire

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
            self.assertEqual(harness._first_block(w.events), "plan", form)
            self.assertEqual(harness._last_card(w.events), ({"question": "Q?"}, "Card cabc123 is showing (cards/1 #1): Q?"), form)

    def test_a_refused_card_is_not_one_to_answer(self):
        w = Wire().start()
        w.results(w.round([("cards_ui", {"question": ""})]), "Tool execution failed: cards_ui: question: required")
        self.assertIsNone(harness._last_card(w.done().events))


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

    def test_the_estimate_counts_the_runnable_set(self):
        t = estimate.table(TASKS)
        e = t["rows"]["expected"]
        self.assertEqual((e["tasks"], e["runs"], e["turns"]), (40, 240, 270))
        for m in spend.CANDIDATES:
            self.assertLess(e["usd"][m], t["rows"]["high"]["usd"][m])


def row(task, arm, repeat, out=100, block=0, per_round=(), closing=0, calls=1, ops=4, skipped=0, repairs=0, reputs=0):
    a = {"block_calls": calls, "block_output": block, "block_output_per_round": list(per_round), "result_input": 0,
         "unseen_ops": 0, "unseen_tokens": 0} if arm == "on" else None
    return {"task": task, "arm": arm, "repeat": repeat, "output": out, "attributed": a,
            "closing": {"k": closing + 1, "rounds": closing, "input": 0, "complete": True},
            "ops": ops if arm == "on" else 0, "skipped": skipped, "repairs": repairs, "reputs": reputs, "refusals": repairs,
            "skipped_by_reason": {}, "skipped_paths_by_reason": {}, "latency": [0.1], "usage": {"output_tokens": out}}


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


if __name__ == "__main__":
    unittest.main()
