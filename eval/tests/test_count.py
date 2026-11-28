import json
import random
import unittest

from b5 import calibrate, providers
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

    def test_an_uncalibrated_count_says_so_and_cant_flip_the_default(self):
        u = Uncalibrated(chars, "chars")
        self.assertEqual(u.call(BLOCK), 8 + len(compact(BLOCK)))
        self.assertEqual((u.describe()["kind"], u.can_flip_default), ("uncalibrated", False))
        self.assertTrue(Calibrated(fitted(intercept=9.0 + len(RECEIPT)), chars, control(6)).can_flip_default)


def endpoint_count(wobble=lambda args: 0, drift=None):
    """A token-count endpoint: a token a character of text and of a call's
    compact arguments, 7 for a call's wrapper (plus `wobble(args)`: the
    framing a serialization adds) and 4 for a result's. `drift`, a list,
    makes each count of the bare probe one more than the last."""
    def count(body):
        n = len(body.get("system") or "")
        for m in body["messages"]:
            for part in m["content"]:
                if part["type"] == "text":
                    n += len(part["text"])
                elif part["type"] == "tool_use":
                    a = compact(part["input"])
                    n += len(a) + 7 + wobble(a)
                elif part["type"] == "tool_result":
                    n += len(part["content"]) + 4
        if drift is not None and len(body["messages"]) == 3 and len(body["messages"][1]["content"]) == 1:
            drift.append(1)
            n += len(drift)
        return n
    return count


CALLS = [{"ops": [{"op": "remove", "id": "x" * (1 + k % 23)}] * (1 + k % 16)} for k in range(210)]


def forced_model(framing=7, wobble=lambda args: 0, no_call=()):
    """A provider's forced call (calibrate.py's output control): it calls
    the tool with the input asked for, and reports its output as the
    arguments' characters plus `framing` (plus `wobble`). `no_call`: the
    request numbers that make no call."""
    seen = []

    def force(body):
        seen.append(body)
        got = json.loads(body["messages"][0]["content"][0]["text"][len(calibrate.FORCE):])
        if len(seen) - 1 in no_call:
            return 3, None
        a = compact(got)
        return len(a) + framing + wobble(a), got
    force.seen = seen
    return force


def control(framing, spread=0):
    """An output control's summary, as calibrate.output_control gives it."""
    return {"framing": framing, "requests": 16, "framing_quartiles": [framing - spread, framing, framing + spread],
            "framing_range": [framing - spread, framing + spread], "no_call": 0, "exact": 16}


class EndpointCounting(unittest.TestCase):
    """With an endpoint (§2.8, "Framing"): a call is its arguments' count
    plus one allowance, calibrated once over the set and applied to every
    call (calibrate.py's `endpoint`)."""

    def test_a_call_is_its_arguments_count_plus_the_one_allowance(self):
        # A call's own framing is 9, or 10 when its arguments' length is a
        # multiple of 5 (the wobble): the allowance is the median, 9, and
        # every call gets it, not its own.
        count = endpoint_count(wobble=lambda a: int(len(a) % 5 == 0))
        framing, raw = calibrate.endpoint(CALLS, TOOLS, count)
        self.assertEqual(framing, 9)
        e = Endpoint(count, TOOLS, framing, raw)
        odd = next(c for c in CALLS if len(compact(c)) % 5 == 0)  # its own framing is 10
        self.assertEqual(e.call(odd), (len(compact(odd)) + 1) + 9)
        w, wo = pair(odd, TOOLS)
        self.assertNotEqual(e.call(odd), count(w) - count(wo) - (len(RECEIPT) + 1), "not its own pair")
        self.assertEqual(e.describe()["framing"], 9)

    def test_the_calibration_keeps_every_raw_difference_and_its_request_count(self):
        framing, raw = calibrate.endpoint(CALLS, TOOLS, endpoint_count())
        self.assertEqual(len(raw["pairs"]), 210)
        p = raw["pairs"][0]
        self.assertEqual((p["diff"] - p["args"] - raw["receipt"], p["framing"]), (9, 9))
        self.assertEqual(raw["requests"], 2 * 210 + 3 + 2, "a with and an args count per call, 3 baselines, the receipt and its base")
        self.assertEqual(raw["baselines"], [raw["baselines"][0]] * 3)
        self.assertTrue(Endpoint(endpoint_count(), TOOLS, framing, raw, output=control(6)).can_flip_default)

    def test_a_spread_framing_or_a_moving_baseline_cant_flip_the_default(self):
        count = endpoint_count(wobble=lambda a: len(a) % 20)
        framing, raw = calibrate.endpoint(CALLS, TOOLS, count)
        e = Endpoint(count, TOOLS, framing, raw)
        self.assertFalse(e.can_flip_default)
        self.assertIn("interquartile", e.describe()["why_not"])
        framing, raw = calibrate.endpoint(CALLS, TOOLS, endpoint_count(drift=[]))
        self.assertIn("the baseline moved", Endpoint(None, TOOLS, framing, raw).describe()["why_not"])
        self.assertFalse(Endpoint(None, TOOLS, 9).can_flip_default, "an allowance with no calibration behind it")


class OutputControl(unittest.TestCase):
    """The two framings (review #14 round 2, R2-4; count.py's "The two
    framings"): the pair's allowance is a call's wrapper and its result's;
    block-call output takes only the call's, measured on output."""

    def test_the_calls_framing_is_measured_on_output_and_the_results_is_input(self):
        # endpoint_count: a call's wrapper is 7 as input, its result's 4.
        # The model writes a call's wrapper in 7 output tokens.
        count = endpoint_count()
        framing, raw = calibrate.endpoint(CALLS, TOOLS, count)
        self.assertEqual(framing, 9, "the pair's allowance: both wrappers, less the text probes' newlines")
        e = Endpoint(count, TOOLS, framing, raw)
        force = forced_model(framing=7)
        e.output = calibrate.output_control(CALLS, TOOLS, force, e.text)
        self.assertEqual(e.output["requests"], 16)
        self.assertTrue(all(b["tool_choice"] == {"type": "tool", "name": "cards_block"} for b in force.seen))
        self.assertEqual((e.call_framing, e.result_framing), (6, 3))
        self.assertTrue(e.can_flip_default, e.describe().get("why_not"))
        # A call is what the model wrote for it: its arguments and its own
        # wrapper. Its result's wrapper isn't block-call output.
        self.assertEqual(e.call(BLOCK), len(compact(BLOCK)) + 7)
        d = e.describe()
        self.assertEqual((d["framing"], d["call_framing"], d["result_framing"]), (9, 6, 3))

    def test_the_results_framing_is_input_once_per_round_after_it_never_block_output(self):
        e = Endpoint(endpoint_count(), TOOLS, 9, None, output=control(6))
        t = Counting().turn()
        a = attribute(t, e)
        self.assertEqual(a["block_output"], (len(compact(PUT)) + 1 + 6) + (len(compact(BLOCK)) + 1 + 6))
        self.assertEqual(a["result_framing_input"], 3 * 3 + 3 * 1, "2 block results: 3 rounds after the first, 1 after the second")
        self.assertEqual(a["result_input"], (len("ok") + 1) * 3 + (len("ok 1 ops (blocks/1 #abc)") + 1) * 1)

    def test_with_no_output_control_the_allowance_is_output_and_cant_flip_the_default(self):
        framing, raw = calibrate.endpoint(CALLS, TOOLS, endpoint_count())
        e = Endpoint(endpoint_count(), TOOLS, framing, raw)
        self.assertEqual((e.call_framing, e.result_framing), (9, 0), "the conservative reading")
        self.assertFalse(e.can_flip_default)
        self.assertIn("no output control", e.describe()["why_not"])
        c = Calibrated(fitted(intercept=9.0 + len(RECEIPT)), chars)
        self.assertEqual((c.call_framing, c.can_flip_default), (9.0, False))
        self.assertIn("no output control", c.describe()["why_not"])

    def test_an_unsound_output_control_cant_flip_the_default(self):
        allowance = 9
        self.assertEqual(calibrate.output_problems(control(6), allowance), [])
        self.assertIn("interquartile", calibrate.output_problems(control(6, spread=3), allowance)[0])
        self.assertIn("output framing 70.00 outside", calibrate.output_problems(control(70), allowance)[0])
        self.assertIn("the result's framing -6.00", calibrate.output_problems(control(15), allowance)[0])
        out = calibrate.output_control(CALLS, TOOLS, forced_model(no_call=(0, 5)), lambda s: len(s))
        self.assertEqual((out["no_call"], out["requests"]), (2, 16))
        self.assertIn("2 of the output control's 16 requests made no call", calibrate.output_problems(out, allowance)[0])
        # The proxy's counter, the same way.
        f = fitted(intercept=9.0 + len(RECEIPT))
        c = Calibrated(f, chars, calibrate.output_control(CALLS, TOOLS, forced_model(framing=6), lambda s: f.ratio * chars(s)))
        self.assertEqual((c.call_framing, c.result_framing, c.can_flip_default), (6.0, 3.0, True))
        self.assertEqual(c.call(BLOCK), 6 + len(compact(BLOCK)))

    def test_its_calls_spread_over_the_sets_sizes(self):
        calls, _ = calibrate.load()
        picked = calibrate.output_calls(calls)
        sizes = [len(compact(c).encode()) for c in picked]
        self.assertEqual(len(picked), calibrate.OUTPUT_CALLS)
        self.assertEqual(sizes, sorted(sizes))
        self.assertLessEqual(sizes[-1], calibrate.OUTPUT_MAX_BYTES)
        self.assertLess(sizes[0] * 10, sizes[-1], "short and long calls both")


class Qualifying(unittest.TestCase):
    """A fitted proxy flips the default only if its fit is sound by the
    bounds fixed before any data (calibrate.py's `QUALIFY`)."""

    def test_a_sound_fit_qualifies(self):
        self.assertEqual(calibrate.problems(fitted(ratio=1.2, intercept=9 + 1.2 * len(RECEIPT))), [])

    def test_a_pathological_slope_or_framing_doesnt(self):
        self.assertIn("ratio 3.0000 outside", calibrate.problems(fitted(ratio=3.0, intercept=9 + 3 * len(RECEIPT)))[0])
        self.assertIn("framing -37.00 outside", calibrate.problems(fitted())[0])
        self.assertFalse(Calibrated(fitted(), chars).can_flip_default)
        self.assertIn("framing -37.00", Calibrated(fitted(), chars).describe()["why_not"])

    def test_a_loose_fit_or_one_pair_that_moves_it_doesnt(self):
        rng = random.Random(3)
        loose = [(x, 9 + len(RECEIPT) + x + rng.uniform(-300, 300)) for x in range(10, 610, 3)]
        self.assertTrue(any(p.startswith("r²") for p in calibrate.problems(fit(loose, "chars", len(RECEIPT)))))
        lever = [(x, 9 + len(RECEIPT) + x) for x in range(10, 220)] + [(5000, 9 + len(RECEIPT) + 5000 * 1.3)]
        self.assertTrue(any("left-one-out" in p for p in calibrate.problems(fit(lever, "chars", len(RECEIPT)))))

    def test_a_moving_baseline_or_a_second_model_doesnt(self):
        f = fitted(intercept=9.0 + len(RECEIPT))
        f.baselines, f.models = (100, 100, 101), ("m-1", "m-2")
        self.assertEqual(calibrate.problems(f), ["the baseline moved: [100, 100, 101]", "more than one model answered: ['m-1', 'm-2']"])

    def test_the_proxys_calibration_counts_its_baseline_three_times(self):
        f, raw = calibrate.calibrate(CALLS, TOOLS, endpoint_count(), chars, "chars")
        self.assertEqual((raw["requests"], len(raw["baselines"]), f.baselines), (213, 3, tuple(raw["baselines"])))

    def test_the_artifact_names_what_it_ran_against_and_no_secret(self):
        c = calibrate.config(providers.Anthropic("claude-haiku-4-5", "sk-secret"), TOOLS)
        self.assertEqual((c["provider"], c["model"], c["count_url"]), ("anthropic", "claude-haiku-4-5",
                                                                       "https://api.anthropic.com/v1/messages/count_tokens"))
        self.assertEqual(c["probe"]["receipt"], RECEIPT)
        self.assertEqual(len(c["tools_sha256"]), 64)
        self.assertEqual(len(c["calibration_set_sha256"]), 64)
        self.assertNotIn("sk-secret", json.dumps(c))


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
