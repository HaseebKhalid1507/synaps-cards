"""The harness's own parts, offline: the run order, the turns a task's steps
make, the desk seam (the stand-in, and a real run's refusal of it), the
providers' request shapes and the tap, spend and the budget guard, and the
report's gate (pooled and 2 of 3 repeats)."""

import json
import threading
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import run
from b5 import calibrate, count, desk, estimate, harness, providers, report, spend, taskset
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

    def test_the_estimate_counts_the_runnable_set(self):
        t = estimate.table(TASKS)
        e = t["rows"]["expected"]
        self.assertEqual((e["tasks"], e["runs"], e["turns"]), (40, 240, 270))
        for m in spend.CANDIDATES:
            self.assertLess(e["usd"][m], t["rows"]["high"]["usd"][m])


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


def qualifying():
    """A fitted proxy (count.Calibrated): a counter that can flip the default.
    0.3 tokens a character, a framing of 9."""
    receipt = len(calibrate.RECEIPT)
    pts = [(x, 9.0 + 0.3 * (receipt + x)) for x in range(10, 10 + 3 * calibrate.CALIBRATION_MIN, 3)]
    return count.Calibrated(calibrate.fit(pts, "chars", receipt), len)


def thirds(s):
    """A proxy for the uncalibrated counter: a token per 3 characters."""
    return len(s) // 3


def frozen_meta(**kw):
    return {"mode": "real", "taskset": taskset.FROZEN.read_text().split()[0], "repeats": 3} | kw


class Eligibility(unittest.TestCase):
    """The verdict (report.py's "The verdict"): rows that pass license
    nothing unless the run is §2.8's experiment. Every run here is built
    on the session wire and read by the real parser, as Shady's
    counterexamples were: the frozen set's runnable tasks, each category
    as it plays, every gate row passing."""

    @staticmethod
    def run_events(task, arm, cut=False):
        w = Wire().start(task["turns"][0]["say"])
        cat, ids = task["category"], []
        if cat == "no_tool":
            w.round(text="An answer.")
        elif cat == "short":
            w.results(w.round([("read", {"path": "a"})]))
            w.round(text="An answer.")
        else:
            if cat == "interactive":
                w.results(w.round([("cards_ui", {"question": "Which?"})]), "Card cabc123 is showing (cards/1 #1): Which?")
                w.round(text="Waiting.")
                w.done()
                w.start("[card cabc123] Which? → A")
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

    def records(self, repeats=(1, 2, 3), drop=0, cut=None):
        recs = []
        for r, t, arm in harness.plan(TASKS, 3)[0]:
            if r + 1 not in repeats:
                continue
            events, ids = self.run_events(t, arm, cut=(t["id"], arm, r + 1) == cut)
            lines = [{"tool_id": i, "event": n, "op": 0, "author": "agent", "did": "patch", "outcome": "applied",
                      "dismissed_before": False} for n, i in enumerate(ids)]
            recs.append({"task": t["id"], "category": t["category"], "arm": arm, "repeat": r + 1, "events": events,
                         "fold": {"lines": lines}, "judge": {"pass": True},
                         "steps": [{"step": "answer", "card": "cabc123"}] if t["category"] == "interactive" else []})
        return recs[:len(recs) - drop]

    def build(self, recs, counter=None, not_run=(), **meta):
        plan = [(t["id"], arm, r + 1) for r, t, arm in harness.plan(TASKS, 3)[0]]
        return report.build(recs, counter or qualifying(), 900, list(not_run), frozen_meta(**meta), plan=plan)

    def test_one_repeat_and_an_uncalibrated_counter_is_not_eligible(self):
        # Shady's first counterexample: it passed the gate.
        rep = self.build(self.records(repeats=(1,)), count.Uncalibrated(thirds, "thirds"), repeats=1)
        self.assertFalse(rep["gate_pass"], "one repeat with an uncalibrated counter is no licence")
        self.assertEqual(rep["verdict"], "not eligible")
        why = " / ".join(rep["not_eligible_because"])
        for part in ("uncalibrated", "1 repeats", "240 runs missing"):
            self.assertIn(part, why)

    def test_three_repeats_with_an_uncalibrated_counter_are_not_eligible(self):
        rep = self.build(self.records(), count.Uncalibrated(thirds, "thirds"))
        self.assertTrue(rep["rows_pass"])
        self.assertFalse(rep["gate_pass"])
        self.assertEqual(len(rep["not_eligible_because"]), 1)
        self.assertIn("the counter is uncalibrated", rep["not_eligible_because"][0])

    def test_the_d8_tasks_not_run_leave_it_not_eligible(self):
        d8 = [t for t in TASKS if t["needs"]]
        rep = self.build(self.records(), not_run=d8)
        self.assertFalse(rep["gate_pass"])
        self.assertEqual(len(rep["not_eligible_because"]), 1)
        self.assertIn(f"{len(d8)} tasks not run", rep["not_eligible_because"][0])
        self.assertIn("Gate: **not eligible**", report.markdown(rep))

    def test_an_interrupted_run_is_incomplete_not_a_smaller_experiment(self):
        # A budget stop after two whole repeats: they pass, 2 of 3.
        rep = self.build(self.records(repeats=(1, 2)))
        self.assertTrue(all(g["repeats_passing"] == [1, 2] for g in rep["gate"].values()))
        self.assertFalse(rep["gate_pass"])
        self.assertIn("80 of the plan's 240 runs missing (an interrupted run is incomplete)", rep["not_eligible_because"])
        rep = self.build(self.records(drop=5))
        self.assertFalse(rep["gate_pass"])
        self.assertIn("5 of the plan's 240 runs missing", " / ".join(rep["not_eligible_because"]))
        rep = self.build(self.records()[:-1] + [dict(self.records()[-1], stopped="over budget")])
        self.assertFalse(rep["gate_pass"])
        self.assertIn("1 runs stopped: over budget", rep["not_eligible_because"])

    def test_a_run_with_a_turn_not_measured_is_not_eligible(self):
        coding = next(t["id"] for t in TASKS if t["category"] == "coding")
        rep = self.build(self.records(cut=(coding, "on", 2)))
        self.assertFalse(rep["gate_pass"])
        self.assertIn("1 runs with a turn not measured", " / ".join(rep["not_eligible_because"]))

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
        orig = (report.per_turn, report.rows_of)
        report.per_turn = lambda rec, counter: ([dict(x, measured=True) for x in by[(rec["task"], rec["arm"], rec["repeat"])]], [])
        report.rows_of = lambda *a: {k: {"pass": True} for k in report.GATE}
        try:
            return report.build([dict(r, fold={"lines": []}, judge={"pass": True}) for r in recs], qualifying(), 900, [], frozen_meta(),
                                plan=[(r["task"], r["arm"], r["repeat"]) for r in recs])
        finally:
            report.per_turn, report.rows_of = orig

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
