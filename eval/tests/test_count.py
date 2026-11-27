import json
import random
import unittest

from b5 import calibrate
from b5.calibrate import RECEIPT, compact, fit, pair
from b5.count import Calibrated, Endpoint, Uncalibrated, attribute, definition, prefix_diff
from b5.turns import turns
from wire import BLOCK, PUT, Wire

TOOLS = [{"name": "read", "description": "Read", "input_schema": {}},
         {"name": "cards_block", "description": "Keep blocks", "input_schema": {}}]


def chars(s):
    """A proxy for the tests: one token per character."""
    return len(s)


def fitted(ratio=1.0, intercept=0.0):
    pts = [(x, intercept + ratio * x) for x in range(10, 10 + 3 * calibrate.CALIBRATION_MIN, 3)]
    return fit(pts, "chars", chars(RECEIPT))


class Fitting(unittest.TestCase):
    def test_the_fit_finds_the_ratio_and_the_framing_is_the_intercept_less_the_receipt(self):
        rng = random.Random(1)
        pts = [(x, 31.0 + 1.25 * x + rng.uniform(-2, 2)) for x in (rng.randint(5, 3000) for _ in range(240))]
        f = fit(pts, "chars", chars(RECEIPT))
        self.assertAlmostEqual(f.ratio, 1.25, places=2)
        self.assertAlmostEqual(f.intercept, 31.0, delta=1.0)
        self.assertAlmostEqual(f.framing, f.intercept - f.ratio * len(RECEIPT), places=9)
        self.assertGreater(f.r2, 0.999)
        self.assertLessEqual(f.resid_max, 2.5)
        self.assertLess(f.loo_ratio[0], f.ratio + 1e-12)
        self.assertGreater(f.loo_ratio[1], f.ratio - 1e-12)
        self.assertEqual(f.n, 240)

    def test_the_spread_brackets_the_fit_and_shows_an_influential_pair(self):
        points = [(x, 2.0 * x + 10) for x in range(1, 220)]
        points[-1] = (points[-1][0], points[-1][1] + 400)  # one pair far off the line
        f = calibrate.fit(points, "chars", 0)
        self.assertLessEqual(f.loo_ratio[0], f.ratio)
        self.assertGreaterEqual(f.loo_ratio[1], f.ratio)
        self.assertLess(f.loo_ratio[0], f.loo_ratio[1], "leaving the far pair out moves the ratio")
        self.assertLess(f.loo_ratio[0], 2.0 + 1e-9)
        self.assertLess(f.loo_intercept[0], f.loo_intercept[1])
        self.assertLess(f.r2, 1.0)
        self.assertGreater(f.resid_max, 300)

    def test_fewer_than_200_pairs_are_refused(self):
        with self.assertRaises(ValueError):
            fit([(x, x) for x in range(199)], "chars", 1)

    def test_a_pair_differs_by_the_call_and_its_result_only(self):
        w, wo = pair(PUT, TOOLS)
        self.assertEqual((w["system"], w["tools"]), (wo["system"], wo["tools"]))
        a, b = w["messages"], wo["messages"]
        self.assertEqual(a[0], b[0])
        self.assertEqual([x for x in a[1]["content"] if x["type"] != "tool_use"], b[1]["content"])
        self.assertEqual([x for x in a[2]["content"] if x["type"] != "tool_result"], b[2]["content"])
        use = next(x for x in a[1]["content"] if x["type"] == "tool_use")
        res = next(x for x in a[2]["content"] if x["type"] == "tool_result")
        self.assertEqual((use["name"], use["input"], res["tool_use_id"], res["content"]), ("cards_block", PUT, use["id"], RECEIPT))
        # The call's own result comes first in its message, as providers require.
        self.assertEqual(a[2]["content"][0]["type"], "tool_result")

    def test_the_calibration_recovers_a_providers_tokens(self):
        # A provider that counts 1.5 tokens a character, plus 9 for each
        # call's and result's wrappers.
        def count(body):
            n = 0
            for m in body["messages"]:
                for part in m["content"]:
                    if part["type"] == "text":
                        n += 1.5 * len(part["text"])
                    elif part["type"] == "tool_use":
                        n += 1.5 * len(compact(part["input"])) + 6
                    elif part["type"] == "tool_result":
                        n += 1.5 * len(part["content"]) + 3
            return n

        calls = [{"ops": [{"op": "remove", "id": "x" * (1 + k % 23)}] * (1 + k % 16)} for k in range(210)]
        f, raw = calibrate.calibrate(calls, TOOLS, count, chars, "chars")
        self.assertAlmostEqual(f.ratio, 1.5, places=9)
        self.assertAlmostEqual(f.framing, 9.0, places=6)
        self.assertEqual(len(raw["pairs"]), 210)
        c = Calibrated(f, chars)
        self.assertAlmostEqual(c.call(BLOCK), 9 + 1.5 * len(compact(BLOCK)), places=6)


class Counting(unittest.TestCase):
    def turn(self):
        w = Wire().start()
        w.results(w.round([("read", {}), ("cards_block", PUT)]))  # 2 rounds after its result
        w.results(w.round([("bash", {})]))
        w.results(w.round([("cards_block", BLOCK)]), "ok 1 ops (blocks/1 #abc)")  # 1 round after
        w.round(text="Fixed.")
        (t,) = turns(w.done().events)
        return t

    def test_a_turns_block_output_and_per_round(self):
        c = Calibrated(fitted(ratio=2.0, intercept=10.0 + 2.0 * len(RECEIPT)), chars)
        a = attribute(self.turn(), c)
        put, tick = 10 + 2 * len(compact(PUT)), 10 + 2 * len(compact(BLOCK))
        self.assertAlmostEqual(a["block_output"], put + tick, places=6)
        self.assertEqual(len(a["block_output_per_round"]), 2)
        self.assertAlmostEqual(a["block_output_per_round"][0], put, places=6)
        self.assertEqual((a["block_calls"], a["counter"]), (2, "calibrated"))

    def test_each_block_result_is_input_once_per_round_after_it(self):
        c = Calibrated(fitted(), chars)
        a = attribute(self.turn(), c)
        self.assertAlmostEqual(a["result_input"], len("ok") * 3 + len("ok 1 ops (blocks/1 #abc)") * 1, places=6)

    def test_a_result_no_round_read_costs_no_input(self):
        # The turn ended (cancelled) right after the block result: nothing re-read it.
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.results(w.round([("cards_block", BLOCK)]), "ok 1 ops (blocks/1 #abc)")
        w.ev({"ev": "aborted", "context_saved": True}).ev({"ev": "idle"})
        (t,) = turns(w.events)
        self.assertEqual(attribute(t, Calibrated(fitted(), chars))["result_input"], 0)

    def test_ops_of_another_turn_arent_this_turns_unseen_patches(self):
        t = self.turn()
        other = {"tool_id": "toolu_999", "op": 0, "author": "agent", "did": "patch", "dismissed_before": True}
        a = attribute(t, Calibrated(fitted(), chars), [other])
        self.assertEqual((a["unseen_ops"], a["unseen_tokens"]), (0, 0))

    def test_ops_on_dismissed_blocks_by_count_and_their_own_tokens(self):
        t = self.turn()
        tick_id = t.block_calls()[1].tool_id
        lines = [
            {"tool_id": tick_id, "op": 0, "author": "agent", "did": "patch", "dismissed_before": True},
            {"tool_id": t.block_calls()[0].tool_id, "op": 0, "author": "agent", "did": "put", "dismissed_before": False},
            {"tool_id": None, "op": None, "author": "user", "did": "notice", "dismissed_before": True},
        ]
        a = attribute(t, Calibrated(fitted(), chars), lines)
        self.assertEqual(a["unseen_ops"], 1)
        self.assertAlmostEqual(a["unseen_tokens"], len(compact(BLOCK["ops"][0])), places=6)

    def test_an_endpoint_call_is_its_pair_less_the_receipt(self):
        asked = []

        def count(body):
            asked.append(1)
            return sum(len(json.dumps(m)) for m in body["messages"])

        e = Endpoint(count, TOOLS)
        w, wo = pair(PUT, TOOLS)
        tw, two = calibrate.text_probe(RECEIPT, TOOLS)
        want = (count(w) - count(wo)) - (count(tw) - count(two))
        asked.clear()
        self.assertEqual(e.call(PUT), want)
        n = len(asked)
        self.assertEqual(e.call(PUT), want)
        self.assertEqual(len(asked), n, "a call counted once is remembered")
        self.assertEqual(e.kind, "endpoint")

    def test_an_uncalibrated_count_says_so_and_cant_flip_the_default(self):
        u = Uncalibrated(chars, "chars")
        self.assertEqual(u.call(BLOCK), 8 + len(compact(BLOCK)))
        self.assertEqual((u.describe()["kind"], u.can_flip_default), ("uncalibrated", False))
        self.assertTrue(Calibrated(fitted(), chars).can_flip_default)


class Definition(unittest.TestCase):
    PREFIX = {"system": "sys", "tools": TOOLS, "messages": [{"role": "user", "content": "Fix it."}, {"role": "assistant", "content": "x"}]}

    def test_the_definition_is_the_prefix_with_the_block_tool_less_without_it(self):
        seen = []

        def count(body):
            seen.append(body)
            return 100 * len(body["tools"]) + len(body["messages"])

        self.assertEqual(definition(count, self.PREFIX), 100)
        self.assertEqual([len(b["messages"]) for b in seen], [1, 1], "the first message only")
        self.assertEqual([[t["name"] for t in b["tools"]] for b in seen], [["read", "cards_block"], ["read"]])

    def test_a_prefix_without_the_block_tool_is_refused(self):
        with self.assertRaises(ValueError):
            definition(lambda b: 0, dict(self.PREFIX, tools=TOOLS[:1]))

    def test_what_else_differs_between_the_arms_first_requests_is_named(self):
        off = dict(self.PREFIX, tools=TOOLS[:1])
        self.assertEqual(prefix_diff(self.PREFIX, off), [])
        self.assertEqual(prefix_diff(self.PREFIX, dict(off, system="other")), ["system"])


if __name__ == "__main__":
    unittest.main()
